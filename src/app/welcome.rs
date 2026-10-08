// Bienvenida la primera vez: cuatro pasos cortos con lo esencial de Forge. Se puede
// volver a ver desde ⌘K → "Bienvenida".
use super::*;
use forge_core::i18n::{Lang, lang};

/// (icono, título, texto) de cada paso.
fn steps() -> [(&'static str, &'static str, &'static str); 4] {
    [
        (
            icon::HAND_WAVING,
            tr!("Bienvenido a Forge"),
            tr!(
                "Cada proyecto con sus terminales, procesos, agentes de IA y un Guard que revisa tus cambios. Abre una carpeta para empezar (o arrástrala a la ventana)."
            ),
        ),
        (
            icon::MAGNIFYING_GLASS,
            tr!("⌘K para todo"),
            tr!(
                "Escribe lo que quieres hacer: abrir un proyecto, dividir la terminal, iniciar un proceso, abrir Claude… ⌘D divide, ⌘T abre una terminal, ⌘F busca en ella y ⌃Tab vuelve al proyecto anterior."
            ),
        ),
        (
            icon::ROBOT,
            tr!("Agentes con memoria"),
            tr!(
                "Claude Code y Codex abiertos desde Forge comparten la Memoria y las Ideas del proyecto (⌘⇧M, ⌘⇧I): recuerdan decisiones, anotan pendientes y los tachan. Te avisan cuando terminan o te necesitan."
            ),
        ),
        (
            icon::SHIELD_CHECK,
            tr!("Guard y Git"),
            tr!(
                "Antes de un commit o un push, Guard revisa tamaño, secretos y tus tests (⌘G). Instala sus hooks para que proteja también fuera de Forge. El panel de Git (⌘⇧G) prepara, confirma y sube tus cambios."
            ),
        ),
    ]
}

impl App {
    pub(super) fn welcome_ui(&mut self, ctx: &egui::Context, cmds: &mut Vec<UiCmd>) {
        let Some(step) = self.welcome else { return };
        let steps = steps();
        let (glyph, title, text) = steps[step];
        let last = step + 1 == steps.len();
        let mut next = None;
        let modal = egui::Modal::new(egui::Id::new("welcome"))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::SEPARATOR))
                    .corner_radius(14.0)
                    .inner_margin(28.0),
            )
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(glyph).size(40.0).color(theme::ACCENT));
                    ui.add_space(6.0);
                    ui.label(RichText::new(title).size(20.0).strong());
                    ui.add_space(4.0);
                    ui.label(RichText::new(text).size(13.5).color(theme::TEXT_2));
                });
                ui.add_space(14.0);
                if step == 0 {
                    ui.vertical_centered(|ui| {
                        ui.horizontal(|ui| {
                            ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                            for (l, label) in [(Lang::En, "English"), (Lang::Es, "Español")] {
                                if ui.selectable_label(lang() == l, label).clicked() {
                                    cmds.push(UiCmd::SetLang(l));
                                }
                            }
                            if theme::primary(
                                ui,
                                format!("{}  {}", icon::FOLDER_PLUS, tr!("Abrir proyecto…")),
                            )
                            .clicked()
                            {
                                cmds.push(UiCmd::OpenFolder);
                                next = Some(None);
                            }
                        });
                    });
                    ui.add_space(10.0);
                }
                ui.horizontal(|ui| {
                    // Puntos de progreso.
                    for k in 0..steps.len() {
                        let color = if k == step {
                            theme::ACCENT
                        } else {
                            theme::SEPARATOR
                        };
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.5, color);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if last {
                            tr!("Empezar")
                        } else {
                            tr!("Siguiente")
                        };
                        if theme::primary(ui, label).clicked() {
                            next = Some((!last).then_some(step + 1));
                        }
                        if step > 0 && theme::secondary(ui, tr!("Anterior")).clicked() {
                            next = Some(Some(step - 1));
                        }
                        if !last
                            && ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(tr!("Saltar")).color(theme::TEXT_3),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                        {
                            next = Some(None);
                        }
                    });
                });
            });
        if modal.should_close() {
            next = Some(None);
        }
        if let Some(next) = next {
            self.welcome = next;
            if next.is_none() {
                self.db(|s| s.set_setting("welcomed", "1"));
            }
        }
    }
}
