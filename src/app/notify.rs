// Notificaciones del sistema (macOS y Linux) para los avisos de actividad, solo con Forge en segundo plano
// (dentro de la app basta el punto de la barra lateral). Una por aviso hasta que se entra
// al proyecto: un agente que espera y redibuja su pantalla no vuelve a notificar. Un clic
// en la notificación abre ese proyecto.
use super::*;
use crate::workspace::{Alert, Attention};
use forge_core::store::Store;
use serde::{Deserialize, Serialize};

/// Sonidos del sistema (Glass, Ping, Basso…).
const SOUNDS: &str = "/System/Library/Sounds";
/// Linux: el tema de sonidos freedesktop (viene con Ubuntu, Fedora…).
const LINUX_SOUNDS: &str = "/usr/share/sounds/freedesktop/stereo";
/// Windows: los sonidos del sistema.
const WINDOWS_SOUNDS: &str = r"C:\Windows\Media";

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

/// Carpeta de sonidos de Windows: `%SystemRoot%\\Media` (C:\\Windows si falta).
fn windows_media(system_root: Option<String>) -> String {
    system_root
        .filter(|r| !r.is_empty())
        .map_or_else(|| WINDOWS_SOUNDS.to_string(), |r| format!("{r}\\Media"))
}

/// Archivo de un sonido por su nombre.
/// En Linux, los de macOS (los valores por defecto) pasan a su parecido de freedesktop.
fn sound_path(name: &str, os: &str) -> PathBuf {
    if os == "macos" {
        return Path::new(SOUNDS).join(format!("{name}.aiff"));
    }
    if os == "windows" {
        let windows = match name {
            "Glass" => "Windows Notify System Generic",
            "Ping" => "Windows Notify Messaging",
            "Basso" => "Windows Critical Stop",
            "Pop" => "Windows Message Nudge",
            "Sosumi" => "Windows Exclamation",
            "Funk" | "Hero" | "Morse" | "Purr" | "Submarine" | "Tink" | "Blow" | "Bottle"
            | "Frog" => "Windows Notify",
            other => other,
        };
        // Texto, no Path::join: en macOS (tests) la `\\` no es separador.
        let media = windows_media(std::env::var("SystemRoot").ok());
        return PathBuf::from(format!("{media}\\{windows}.wav"));
    }
    let linux = match name {
        "Glass" => "complete",
        "Ping" => "message-new-instant",
        "Basso" => "dialog-warning",
        "Pop" => "message",
        "Sosumi" => "suspend-error",
        "Funk" | "Hero" | "Morse" | "Purr" | "Submarine" | "Tink" | "Blow" | "Bottle" | "Frog" => {
            "bell"
        }
        other => other,
    };
    Path::new(LINUX_SOUNDS).join(format!("{linux}.oga"))
}

/// Sonidos del sistema disponibles, por nombre.
pub(super) fn sounds() -> &'static [String] {
    static SOUND_NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    SOUND_NAMES.get_or_init(|| {
        let (dir, ext) = if cfg!(target_os = "macos") {
            (SOUNDS.to_string(), "aiff")
        } else if cfg!(windows) {
            (windows_media(std::env::var("SystemRoot").ok()), "wav")
        } else {
            (LINUX_SOUNDS.to_string(), "oga")
        };
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                (path.extension()? == ext).then(|| path.file_stem()?.to_str().map(String::from))?
            })
            .collect();
        names.sort();
        names
    })
}

/// Suena sin esperar: afplay en macOS, paplay (PulseAudio/PipeWire, también en WSLg) en
/// Linux (los dos respetan el volumen) y PlaySound en Windows. La notificación va muda.
// ponytail: en Windows suena al volumen del sistema (PlaySound no tiene volumen propio).
pub(super) fn play(sound: &str, volume: f32) {
    let path = sound_path(sound, std::env::consts::OS);
    let volume = volume.clamp(0.0, 1.0);
    std::thread::spawn(move || {
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = std::process::Command::new("/usr/bin/afplay");
            c.arg("-v").arg(format!("{volume:.2}"));
            c
        } else if cfg!(windows) {
            #[cfg(windows)]
            {
                use windows_sys::Win32::Media::Audio::{
                    PlaySoundW, SND_FILENAME, SND_NODEFAULT, SND_SYNC,
                };
                let wide: Vec<u16> = path
                    .to_string_lossy()
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                // SAFETY: `wide` termina en 0 y vive hasta que PlaySound vuelve (SND_SYNC).
                unsafe {
                    PlaySoundW(
                        wide.as_ptr(),
                        std::ptr::null_mut(),
                        SND_FILENAME | SND_SYNC | SND_NODEFAULT,
                    )
                };
            }
            return;
        } else {
            let mut c = std::process::Command::new("paplay");
            c.arg(format!("--volume={}", (volume * 65536.0) as u32));
            c
        };
        let _ = cmd.arg(path).status();
    });
}

/// Un comando peligroso espera tu permiso: (sonido, notificación). Usa el aviso «Pide
/// permiso» de Ajustes; suena también con Forge al frente y notifica si estás en otra app.
pub(super) fn approval_alert(
    alerts: &Alerts,
    notifications: bool,
    focused: bool,
) -> (Option<String>, bool) {
    if !notifications || alerts.quiet {
        return (None, false);
    }
    let prefs = alerts.get(Alert::Approval);
    if focused && !prefs.foreground {
        return (None, false);
    }
    let sound = (!prefs.sound.is_empty()).then_some(prefs.sound);
    (sound, prefs.notify && !focused)
}

impl App {
    /// Avisa (sonido y notificación) de una solicitud de autorización nueva.
    pub(super) fn alert_approval(
        &mut self,
        ctx: &egui::Context,
        request: &forge_core::danger::Request,
    ) {
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let (sound, show) = approval_alert(&self.alerts, self.notifications, focused);
        if let Some(sound) = sound {
            play(&sound, self.alerts.volume);
        }
        if show {
            let who = request.origin.clone().unwrap_or_else(|| "Forge".into());
            let path = request
                .project
                .clone()
                .unwrap_or_else(|| request.cwd.clone());
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text = tr!(
                "{who} pide permiso: {cmd}",
                who = who,
                cmd = request.command
            );
            let (clicked, ctx) = (self.notification_click.clone(), ctx.clone());
            notify(name, path, text, clicked, ctx);
        }
    }

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
                    // Mucho contexto: una franja en esa terminal, sin notificación.
                    if event.kind == "compact" {
                        if let Some(panel) = event.panel {
                            ws.compact_offer(panel);
                        }
                        continue;
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

/// Linux: notificación de escritorio (D-Bus); un clic en ella abre el proyecto.
// ponytail: sin sonido propio en Linux (la lista de sonidos sale vacía).
#[cfg(target_os = "linux")]
fn notify(
    project: String,
    path: PathBuf,
    text: String,
    clicked: Arc<Mutex<Option<PathBuf>>>,
    ctx: egui::Context,
) {
    if crate::is_wsl() {
        // status() espera a PowerShell: sin procesos zombi y sin frenar la interfaz.
        std::thread::spawn(move || {
            let _ = windows_toast(&project, &text)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
        return;
    }
    std::thread::spawn(move || {
        let Ok(handle) = notify_rust::Notification::new()
            .appname("Forge")
            .summary(&format!("Forge · {project}"))
            .body(&text)
            .action("default", tr!("Abrir"))
            .show()
        else {
            return;
        };
        handle.wait_for_action(|action| {
            if action == "default" {
                *clicked.lock().unwrap() = Some(path);
                ctx.request_repaint();
            }
        });
    });
}

/// Aviso nativo de Windows con powershell.exe, en Windows y desde WSL (donde las
/// notificaciones de Linux no llegan a Windows). El texto va en variables (WSLENV las pasa a
/// Windows desde WSL), nunca en el script.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
fn windows_toast(project: &str, text: &str) -> std::process::Command {
    // ponytail: con el id de app de PowerShell (Windows descarta los de apps sin registrar);
    // sin clic que abra el proyecto.
    const SCRIPT: &str = r#"
$m = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
$t = $m::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$n = $t.GetElementsByTagName('text')
$n.Item(0).AppendChild($t.CreateTextNode($env:FORGE_TOAST_TITLE)) > $null
$n.Item(1).AppendChild($t.CreateTextNode($env:FORGE_TOAST_BODY)) > $null
$id = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
$m::CreateToastNotifier($id).Show([Windows.UI.Notifications.ToastNotification]::new($t))
"#;
    let wslenv = with_toast_vars(&std::env::var("WSLENV").unwrap_or_default());
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("FORGE_TOAST_TITLE", format!("Forge · {project}"))
        .env("FORGE_TOAST_BODY", text)
        .env("WSLENV", wslenv);
    cmd
}

/// WSLENV con las variables del aviso añadidas.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
fn with_toast_vars(wslenv: &str) -> String {
    let vars = "FORGE_TOAST_TITLE:FORGE_TOAST_BODY";
    if wslenv.is_empty() {
        vars.to_string()
    } else {
        format!("{wslenv}:{vars}")
    }
}

/// Windows: el mismo aviso nativo que desde WSL (powershell.exe: PowerShell 7 no carga las
/// clases de notificación de Windows). Sin clic que abra el proyecto.
#[cfg(windows)]
fn notify(
    project: String,
    _path: PathBuf,
    text: String,
    _clicked: Arc<Mutex<Option<PathBuf>>>,
    _ctx: egui::Context,
) {
    // status() espera a PowerShell: sin procesos colgando y sin frenar la interfaz.
    std::thread::spawn(move || {
        let _ = windows_toast(&project, &text)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn notify(_: String, _: PathBuf, _: String, _: Arc<Mutex<Option<PathBuf>>>, _: egui::Context) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approval_request_rings_and_notifies() {
        let alerts = Alerts::default();
        // Fuera de Forge: sonido y notificación (si no, el usuario no se entera).
        assert_eq!(
            approval_alert(&alerts, true, false),
            (Some("Ping".to_string()), true)
        );
        // Con Forge al frente: el sonido igual (pide permiso), sin notificación.
        assert_eq!(
            approval_alert(&alerts, true, true),
            (Some("Ping".to_string()), false)
        );
        // No molestar o avisos apagados: nada.
        let quiet = Alerts {
            quiet: true,
            ..Alerts::default()
        };
        assert_eq!(approval_alert(&quiet, true, false), (None, false));
        assert_eq!(approval_alert(&alerts, false, false), (None, false));
    }

    #[test]
    fn linux_plays_the_freedesktop_sound_closest_to_each_macos_one() {
        let linux = Path::new("/usr/share/sounds/freedesktop/stereo");
        assert_eq!(sound_path("Glass", "linux"), linux.join("complete.oga"));
        assert_eq!(
            sound_path("Ping", "linux"),
            linux.join("message-new-instant.oga")
        );
        // Un sonido de Linux elegido en Ajustes se usa tal cual.
        assert_eq!(sound_path("bell", "linux"), linux.join("bell.oga"));
        assert_eq!(
            sound_path("Glass", "macos"),
            Path::new("/System/Library/Sounds/Glass.aiff")
        );
        // Windows: los .wav de C:\\Windows\\Media.
        assert_eq!(
            sound_path("Ping", "windows"),
            Path::new(r"C:\Windows\Media\Windows Notify Messaging.wav")
        );
        assert_eq!(
            sound_path("Basso", "windows"),
            Path::new(r"C:\Windows\Media\Windows Critical Stop.wav")
        );
        assert_eq!(
            sound_path("tada", "windows"),
            Path::new(r"C:\Windows\Media\tada.wav")
        );
        // Windows instalado en otra unidad.
        assert_eq!(windows_media(Some(r"D:\Win".into())), r"D:\Win\Media");
    }

    #[test]
    fn wslenv_gets_the_toast_variables_without_a_stray_colon() {
        assert_eq!(with_toast_vars(""), "FORGE_TOAST_TITLE:FORGE_TOAST_BODY");
        assert_eq!(
            with_toast_vars("USERPROFILE/p"),
            "USERPROFILE/p:FORGE_TOAST_TITLE:FORGE_TOAST_BODY"
        );
    }

    #[test]
    fn wsl_toasts_pass_the_text_outside_the_script() {
        let cmd = windows_toast("app'; rm -rf ~", "listo $(whoami)");
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter()
                .all(|a| !a.contains("whoami") && !a.contains("rm -rf"))
        );
        let env: HashMap<_, _> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            env["FORGE_TOAST_TITLE"].as_deref(),
            Some("Forge · app'; rm -rf ~")
        );
        assert_eq!(env["FORGE_TOAST_BODY"].as_deref(), Some("listo $(whoami)"));
        assert!(
            env["WSLENV"]
                .as_deref()
                .unwrap()
                .contains("FORGE_TOAST_TITLE")
        );
    }

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
        // Los sonidos son los de macOS (/System/Library/Sounds).
        if cfg!(target_os = "macos") {
            assert!(sounds().iter().any(|s| s == "Glass"));
        }
    }
}
