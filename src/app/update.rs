// Actualización dentro de la app, como Warp: pastilla "Actualizar Forge" en la barra
// superior → descarga en segundo plano → "Reiniciar para actualizar" sustituye la app y la
// vuelve a abrir. Se busca al abrir Forge, cada pocas horas mientras está abierto y a mano
// (⌘K o Ajustes).
use super::*;
use forge_core::updates::{self, Release};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Update {
    Available(Release),
    Downloading,
    Ready(PathBuf),
    Failed(Release, String),
    /// Búsqueda pedida a mano, en curso.
    Checking,
    /// Búsqueda a mano sin novedades: la pastilla lo dice unos segundos.
    UpToDate(Instant),
}

/// Cada cuánto se vuelve a mirar con Forge abierto.
const CHECK_EVERY: Duration = Duration::from_secs(4 * 3600);

impl App {
    /// Cada fotograma (solo la app real tiene `next_update_check`): ¿toca mirar otra vez?
    pub(super) fn update_tick(&mut self, ctx: &egui::Context) {
        if self
            .next_update_check
            .is_some_and(|at| Instant::now() >= at)
        {
            self.next_update_check = Some(Instant::now() + CHECK_EVERY);
            self.check_updates(ctx, false);
        }
    }

    /// En segundo plano: ¿hay una versión de Forge más nueva publicada? A mano (`manual`),
    /// la pastilla dice "Buscando…" y, si no hay nada, "Forge está al día".
    pub(super) fn check_updates(&mut self, ctx: &egui::Context, manual: bool) {
        // Descargando o lista para reiniciar: no se pisa ese estado.
        if matches!(
            self.update_state(),
            Some(Update::Downloading | Update::Ready(_))
        ) {
            return;
        }
        if manual {
            *self.update.lock().unwrap() = Some(Update::Checking);
        }
        let (slot, ctx) = (self.update.clone(), ctx.clone());
        std::thread::spawn(move || {
            let newer = updates::latest(env!("CARGO_PKG_REPOSITORY"))
                .filter(|r| updates::is_newer(&r.tag, env!("CARGO_PKG_VERSION")));
            let mut state = slot.lock().unwrap();
            match newer {
                Some(release) => *state = Some(Update::Available(release)),
                None if manual => *state = Some(Update::UpToDate(Instant::now())),
                None => {}
            }
            drop(state);
            ctx.request_repaint();
            // Para que "Forge está al día" se vaya solo.
            ctx.request_repaint_after(Duration::from_secs(5));
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
            Some(Update::Downloading | Update::Checking | Update::UpToDate(_)) | None => {}
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
        Update::Checking => (tr!("Buscando…"), tr!("Buscando una versión nueva").into()),
        Update::UpToDate(at) if at.elapsed() < Duration::from_secs(4) => {
            let text = format!("{}  {}", icon::CHECK, tr!("Forge está al día"));
            let hint = tr!(
                "Tienes la última versión ({v})",
                v = env!("CARGO_PKG_VERSION")
            );
            ui.label(RichText::new(text).size(12.0).color(theme::GREEN))
                .on_hover_text(hint);
            ui.add_space(6.0);
            return;
        }
        Update::UpToDate(_) => return,
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
