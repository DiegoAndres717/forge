// Autorización de comandos peligrosos (§15.2): las terminales de Forge dejan la solicitud
// en `approvals/` y esperan; aquí se vigila esa carpeta y se pregunta al usuario.
use super::*;
use forge_core::danger::{self, Request};

impl App {
    /// Vigila la carpeta de solicitudes en segundo plano (repinta al cambiar).
    pub(super) fn watch_approvals(&mut self, ctx: &egui::Context, dir: PathBuf) {
        self.approvals_dir = Some(dir.clone());
        let pending = self.approvals.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            loop {
                let now = danger::pending(&dir);
                let changed = {
                    let Ok(mut current) = pending.lock() else {
                        return;
                    };
                    let changed = *current != now;
                    *current = now;
                    changed
                };
                if changed {
                    ctx.request_repaint();
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
    }

    /// Diálogo para la primera solicitud pendiente.
    pub(super) fn approvals_ui(&mut self, ctx: &egui::Context) {
        let Some(request) = self.approvals.lock().ok().and_then(|p| p.first().cloned()) else {
            return;
        };
        let Some(dir) = self.approvals_dir.clone() else {
            return;
        };
        // Una sola vez por solicitud: traer Forge al frente y avisar en el Dock.
        if self.approval_seen.as_deref() != Some(request.id.as_str()) {
            self.approval_seen = Some(request.id.clone());
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Critical,
            ));
        }
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::new(("approval", &request.id)))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::SEPARATOR))
                    .corner_radius(12.0)
                    .inner_margin(18.0),
            )
            .show(ctx, |ui| {
                ui.set_width(460.0);
                Self::request_ui(ui, &request, &mut decision);
            });
        // Esc o clic fuera: lo seguro es rechazar.
        if modal.should_close() && decision.is_none() {
            decision = Some(false);
        }
        if let Some(allow) = decision {
            if let Err(e) = danger::answer(&dir, &request.id, allow) {
                self.error = Some(format!("no se pudo responder la autorización: {e}"));
            }
            if let Ok(mut p) = self.approvals.lock() {
                p.retain(|r| r.id != request.id);
            }
        }
    }

    fn request_ui(ui: &mut egui::Ui, request: &Request, decision: &mut Option<bool>) {
        let who = request.origin.as_deref().unwrap_or("Una terminal de Forge");
        ui.label(
            RichText::new(format!(
                "{}  {who} quiere ejecutar un comando peligroso",
                icon::WARNING
            ))
            .size(15.0)
            .strong()
            .color(theme::ORANGE),
        );
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(theme::FIELD)
            .corner_radius(6.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(&request.command)
                        .monospace()
                        .color(theme::TEXT),
                );
            });
        ui.add_space(4.0);
        ui.label(RichText::new(capitalize(&request.reason)).color(theme::TEXT_2));
        ui.label(
            RichText::new(format!("En {}", tilde(&request.cwd)))
                .size(11.5)
                .color(theme::TEXT_3),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, "Denegar").clicked() {
                *decision = Some(false);
            }
            if theme::secondary(ui, "Permitir una vez").clicked() {
                *decision = Some(true);
            }
        });
    }
}
