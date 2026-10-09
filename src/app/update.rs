// Actualización dentro de la app, como Warp: pastilla "Actualizar Forge" en la barra
// superior → descarga en segundo plano → "Reiniciar para actualizar" sustituye la app y la
// vuelve a abrir.
use super::*;
use forge_core::updates::{self, Release};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Update {
    Available(Release),
    Downloading,
    Ready(PathBuf),
    Failed(Release, String),
}

impl App {
    /// Una vez al día, en segundo plano: ¿hay una versión de Forge más nueva publicada?
    pub(super) fn check_updates(&mut self, ctx: &egui::Context) {
        let last: i64 = self
            .db(|s| s.setting("update_checked"))
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if store::now() - last < 86_400 {
            return;
        }
        self.db(|s| s.set_setting("update_checked", &store::now().to_string()));
        let (slot, ctx) = (self.update.clone(), ctx.clone());
        std::thread::spawn(move || {
            if let Some(release) = updates::latest(env!("CARGO_PKG_REPOSITORY"))
                && updates::is_newer(&release.tag, env!("CARGO_PKG_VERSION"))
            {
                *slot.lock().unwrap() = Some(Update::Available(release));
                ctx.request_repaint();
            }
        });
    }

    pub(super) fn update_state(&self) -> Option<Update> {
        self.update.lock().ok().and_then(|u| u.clone())
    }

    /// Clic en la pastilla: descargar, o reiniciar con la versión ya descargada.
    pub(super) fn run_update(&mut self, ctx: &egui::Context) {
        match self.update_state() {
            Some(Update::Available(release) | Update::Failed(release, _)) => {
                // Sin .dmg con huella o fuera de un .app (cargo run): la página de la versión.
                let (Some(dmg), Some(checksum), Some(_)) = (
                    release.dmg.clone(),
                    release.checksum.clone(),
                    updates::current_app(),
                ) else {
                    let _ = std::process::Command::new("open")
                        .arg(&release.page)
                        .spawn();
                    return;
                };
                *self.update.lock().unwrap() = Some(Update::Downloading);
                let (slot, ctx) = (self.update.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let state = match updates::download(&dmg, &checksum) {
                        Ok(staged) => Update::Ready(staged),
                        Err(e) => Update::Failed(release, e),
                    };
                    *slot.lock().unwrap() = Some(state);
                    ctx.request_repaint();
                });
            }
            Some(Update::Ready(staged)) => {
                let Some(app) = updates::current_app() else {
                    return;
                };
                let script = updates::relaunch_script(std::process::id(), &staged, &app);
                let spawned = std::process::Command::new("/bin/sh")
                    .args(["-c", &script])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                match spawned {
                    // Al cerrar se guarda la sesión (on_exit) y el script hace el resto.
                    Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Some(Update::Downloading) | None => {}
        }
    }
}

/// Pastilla con borde de color en la barra superior (va en un layout de derecha a izquierda).
pub(super) fn update_pill(ui: &mut egui::Ui, state: &Option<Update>, cmds: &mut Vec<UiCmd>) {
    let Some(state) = state else { return };
    let (text, hover): (&str, String) = match state {
        Update::Available(r) => (
            tr!("Actualizar Forge"),
            tr!("Forge {tag} disponible: descargarla", tag = r.tag),
        ),
        Update::Downloading => (
            tr!("Descargando…"),
            tr!("Descargando la versión nueva").into(),
        ),
        Update::Ready(_) => (
            tr!("Reiniciar para actualizar"),
            tr!("Forge se cerrará y se abrirá con la versión nueva").into(),
        ),
        Update::Failed(_, e) => (
            tr!("Reintentar actualización"),
            tr!("No se pudo descargar: {e}", e = e),
        ),
    };
    let button = egui::Button::new(RichText::new(text).size(12.0).color(theme::ACCENT))
        .fill(Color32::TRANSPARENT)
        .stroke(egui::Stroke::new(1.0, theme::ACCENT))
        .corner_radius(13.0)
        .min_size(Vec2::new(0.0, 26.0));
    if ui.add(button).on_hover_text(hover).clicked() {
        cmds.push(UiCmd::Update);
    }
    ui.add_space(6.0);
}
