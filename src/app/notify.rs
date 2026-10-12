// Notificaciones de macOS para los avisos de actividad, solo con Forge en segundo plano
// (dentro de la app basta el punto de la barra lateral). Una por aviso hasta que se entra
// al proyecto: un agente que espera y redibuja su pantalla no vuelve a notificar. Un clic
// en la notificación abre ese proyecto.
use super::*;
use crate::workspace::{Alert, Attention};
use forge_core::store::Store;
use serde::{Deserialize, Serialize};

/// Sonidos del sistema (Glass, Ping, Basso…).
const SOUNDS: &str = "/System/Library/Sounds";

/// Cómo avisa un tipo de aviso.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct AlertPrefs {
    /// Notificación de macOS.
    pub(super) notify: bool,
    /// Sonido del sistema ("" = ninguno).
    pub(super) sound: String,
    /// Avisar también con Forge al frente.
    pub(super) foreground: bool,
}

/// Los avisos: no molestar, volumen y cada tipo. Se guardan en la base de datos (`alerts`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Alerts {
    pub(super) quiet: bool,
    /// 0 a 1.
    pub(super) volume: f32,
    pub(super) kinds: HashMap<Alert, AlertPrefs>,
}

impl Default for Alerts {
    fn default() -> Self {
        Self {
            quiet: false,
            volume: 0.6,
            kinds: HashMap::new(),
        }
    }
}

impl Alerts {
    /// Lo de cada tipo, o su valor por defecto.
    pub(super) fn get(&self, alert: Alert) -> AlertPrefs {
        self.kinds.get(&alert).cloned().unwrap_or_else(|| {
            let (sound, foreground) = match alert {
                Alert::Done => ("Glass", false),
                // Un agente parado esperando tu permiso: también con Forge al frente.
                Alert::Approval => ("Ping", true),
                Alert::Failed => ("Basso", false),
                Alert::Blocked => ("Funk", false),
                Alert::Limit => ("Sosumi", false),
                Alert::Plan => ("Pop", false),
            };
            AlertPrefs {
                notify: true,
                sound: sound.into(),
                foreground,
            }
        })
    }

    pub(super) fn load(store: Option<&Store>) -> Self {
        let setting = |k: &str| store.and_then(|s| s.setting(k).ok().flatten());
        match setting("alerts").and_then(|json| serde_json::from_str(&json).ok()) {
            Some(alerts) => alerts,
            // Antes había un solo interruptor: apagado es «No molestar».
            None => Self {
                quiet: setting("notifications").as_deref() == Some("off"),
                ..Self::default()
            },
        }
    }

    pub(super) fn save(&self, store: Option<&Store>) -> Result<(), String> {
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        store.map_or(Ok(()), |s| s.set_setting("alerts", &json))
    }
}

/// Nombre del aviso en Ajustes y para qué es.
pub(super) fn alert_text(alert: Alert) -> (&'static str, &'static str) {
    match alert {
        Alert::Done => (
            tr!("Agente terminó"),
            tr!("Un agente acabó su turno y te deja la respuesta."),
        ),
        Alert::Approval => (
            tr!("Pide aprobación"),
            tr!("Un agente está parado esperando tu respuesta o tu permiso para seguir."),
        ),
        Alert::Failed => (
            tr!("Proceso falló"),
            tr!("Un proceso administrado (servidor, watcher…) terminó con error."),
        ),
        Alert::Blocked => (
            tr!("Guard bloqueó"),
            tr!("Project Guard encontró algo que impide el commit o el push."),
        ),
        Alert::Limit => (
            tr!("Límite de la cuenta"),
            tr!("La cuenta de un agente llegó a su límite de uso; Forge ofrece seguir con otra."),
        ),
        Alert::Plan => (
            tr!("Plan creado"),
            tr!("Un agente creó un plan por fases en Planes."),
        ),
    }
}

/// Sonidos del sistema disponibles, por nombre.
pub(super) fn sounds() -> &'static [String] {
    static SOUND_NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    SOUND_NAMES.get_or_init(|| {
        let mut names: Vec<String> = std::fs::read_dir(SOUNDS)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                (path.extension()? == "aiff")
                    .then(|| path.file_stem()?.to_str().map(String::from))?
            })
            .collect();
        names.sort();
        names
    })
}

/// Suena sin esperar (afplay respeta el volumen; la notificación va muda).
pub(super) fn play(sound: &str, volume: f32) {
    let path = Path::new(SOUNDS).join(format!("{sound}.aiff"));
    let volume = format!("{:.2}", volume.clamp(0.0, 1.0));
    std::thread::spawn(move || {
        let _ = std::process::Command::new("/usr/bin/afplay")
            .arg("-v")
            .arg(volume)
            .arg(path)
            .status();
    });
}

impl App {
    pub(super) fn notify_tick(&mut self, ctx: &egui::Context) {
        self.notification_clicked(ctx);
        // Eventos de los agentes (hooks), cada medio segundo.
        if self
            .events_checked
            .is_none_or(|t| t.elapsed() > Duration::from_millis(500))
            && let Some(dir) = crate::terminal::SHELL_ENV
                .get()
                .and_then(|e| e.get("FORGE_EVENTS"))
        {
            self.events_checked = Some(Instant::now());
            for event in forge_core::events::drain(Path::new(dir)) {
                if let Some(ws) = self
                    .workspaces
                    .iter_mut()
                    .find(|w| w.project.path == event.project)
                {
                    if event.kind == "limit"
                        && let Some(panel) = event.panel
                    {
                        ws.limit_reached(panel);
                    }
                    ws.agent_event(Alert::from_event(&event.kind), event.text);
                }
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if !self.notifications || self.alerts.quiet {
            return;
        }
        for ws in &mut self.workspaces {
            let Some(attention) = ws.attention() else {
                continue;
            };
            let prefs = self.alerts.get(attention.alert());
            // Con Forge al frente basta el punto de la barra lateral, salvo que se pida.
            if ws.notified.contains(&attention) || (focused && !prefs.foreground) {
                continue;
            }
            ws.notified.push(attention.clone());
            if !prefs.sound.is_empty() {
                play(&prefs.sound, self.alerts.volume);
            }
            if !prefs.notify {
                continue;
            }
            let text = match &attention {
                Attention::Agent(name) => tr!("{name} terminó o espera respuesta", name = name),
                Attention::Failed(name) => tr!("El proceso {name} falló", name = name),
                Attention::Blocked => tr!("Guard bloqueó la validación").to_string(),
                Attention::Event(_, text) => text.clone(),
            };
            let (clicked, ctx) = (self.notification_click.clone(), ctx.clone());
            notify(
                ws.project.name(),
                ws.project.path.clone(),
                text,
                clicked,
                ctx,
            );
        }
    }

    /// Clic en una notificación: Forge al frente y en ese proyecto.
    fn notification_clicked(&mut self, ctx: &egui::Context) {
        if let Some(path) = self
            .notification_click
            .lock()
            .ok()
            .and_then(|mut c| c.take())
            && let Some(i) = self.workspaces.iter().position(|w| w.project.path == path)
        {
            self.active = Some(i);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }
}

/// Notificación del sistema; un clic deja el proyecto en `clicked` y redibuja.
#[cfg(target_os = "macos")]
fn notify(
    project: String,
    path: PathBuf,
    text: String,
    clicked: Arc<Mutex<Option<PathBuf>>>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let _ = mac_notification_sys::set_application("dev.forge.app");
        let response = mac_notification_sys::Notification::new()
            .title("Forge")
            .subtitle(&project)
            .message(&text)
            .wait_for_click(true)
            .send();
        if matches!(
            response,
            Ok(mac_notification_sys::NotificationResponse::Click)
        ) {
            *clicked.lock().unwrap() = Some(path);
            ctx.request_repaint();
        }
    });
}

// ponytail: fuera de macOS aún sin notificaciones (vendrán con notify-rust).
#[cfg(not(target_os = "macos"))]
fn notify(_: String, _: PathBuf, _: String, _: Arc<Mutex<Option<PathBuf>>>, _: egui::Context) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alerts_keep_defaults_and_old_switch() {
        let store = Store::in_memory().unwrap();
        // El interruptor de antes apagado: «No molestar».
        store.set_setting("notifications", "off").unwrap();
        let mut alerts = Alerts::load(Some(&store));
        assert!(alerts.quiet);
        assert_eq!(alerts.get(Alert::Approval).sound, "Ping");
        assert!(alerts.get(Alert::Approval).foreground);
        alerts.quiet = false;
        alerts.kinds.insert(
            Alert::Done,
            AlertPrefs {
                notify: false,
                sound: String::new(),
                foreground: false,
            },
        );
        alerts.save(Some(&store)).unwrap();
        let loaded = Alerts::load(Some(&store));
        assert_eq!(loaded, alerts);
        assert!(!loaded.get(Alert::Done).notify);
        assert_eq!(loaded.get(Alert::Failed).sound, "Basso");
        assert!(sounds().iter().any(|s| s == "Glass"));
    }
}
