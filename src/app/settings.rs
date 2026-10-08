// Ventana de Ajustes (⌘,): idioma, notificaciones, terminal y atajos al proyecto actual.
use super::*;
use forge_core::i18n::{Lang, lang};

impl App {
    pub(super) fn settings_ui(&mut self, ctx: &egui::Context, cmds: &mut Vec<UiCmd>) {
        if !self.settings_open {
            return;
        }
        let mut changed = false;
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::SEPARATOR))
                    .corner_radius(12.0)
                    .inner_margin(20.0),
            )
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::GEAR).size(18.0).color(theme::TEXT_2));
                    ui.label(RichText::new(tr!("Ajustes")).size(16.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, icon::X, tr!("Cerrar (Esc)")).clicked() {
                            self.settings_open = false;
                        }
                    });
                });
                ui.add_space(8.0);

                theme::section(ui, tr!("General"));
                theme::card(ui, |ui| {
                    egui::Grid::new("settings-general").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                        ui.label(tr!("Idioma"));
                        ui.horizontal(|ui| {
                            for (l, label) in [(Lang::En, "English"), (Lang::Es, "Español")] {
                                if ui.selectable_label(lang() == l, label).clicked() {
                                    cmds.push(UiCmd::SetLang(l));
                                }
                            }
                        });
                        ui.end_row();
                        ui.label(tr!("Notificaciones"));
                        let mut on = self.notifications_enabled;
                        if ui
                            .checkbox(&mut on, tr!("Avisar cuando un agente termina, un proceso falla o Guard bloquea"))
                            .changed()
                        {
                            self.notifications_enabled = on;
                            self.db(|s| s.set_setting("notifications", if on { "on" } else { "off" }));
                        }
                        ui.end_row();
                    });
                });

                ui.add_space(6.0);
                theme::section(ui, tr!("Terminal"));
                theme::card(ui, |ui| {
                    egui::Grid::new("settings-terminal").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                        ui.label(tr!("Tamaño de letra"));
                        changed |= ui
                            .add(egui::Slider::new(&mut self.settings.font_size, 9.0..=28.0).step_by(1.0)
                                    .fixed_decimals(0)
                                    .trailing_fill(true).suffix(" pt"))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .changed();
                        ui.end_row();
                        ui.label(tr!("Tecla Option"));
                        changed |= ui
                            .checkbox(&mut self.settings.option_as_meta, tr!("Como Meta (atajos de terminal: ⌥B, ⌥F…)"))
                            .changed();
                        ui.end_row();
                    });
                    ui.label(
                        RichText::new(tr!("Fuentes propias y más opciones: ~/.config/forge/config.toml"))
                            .size(11.0)
                            .color(theme::TEXT_3),
                    );
                });

                // Tokens de Claude abierto desde Forge (subagentes incluidos), 30 días.
                let usage = self
                    .db(|s| s.agent_usage(None, store::now() - 30 * 86_400))
                    .unwrap_or_default();
                if !usage.is_empty() {
                    ui.add_space(6.0);
                    theme::section(ui, tr!("Uso de modelos (30 días)"));
                    theme::card(ui, |ui| {
                        let total: u64 = usage.iter().map(|u| u.total()).sum::<u64>().max(1);
                        egui::Grid::new("settings-usage").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
                            for u in &usage {
                                ui.label(RichText::new(&u.model).strong());
                                ui.label(format!(
                                    "{} tokens · {}%",
                                    short_tokens(u.total()),
                                    u.total() * 100 / total
                                ));
                                ui.label(
                                    RichText::new(tr!(
                                        "{sub} de {turns} turnos por subagentes",
                                        sub = u.subagent_turns,
                                        turns = u.turns
                                    ))
                                    .size(11.5)
                                    .color(theme::TEXT_3),
                                );
                                ui.end_row();
                            }
                        });
                        ui.label(
                            RichText::new(tr!("Claude Code abierto desde Forge delega búsquedas en Haiku y revisiones en Sonnet; aquí ves a dónde van los tokens."))
                                .size(11.0)
                                .color(theme::TEXT_3),
                        );
                    });
                }

                if let Some(i) = self.active {
                    let ws = &self.workspaces[i];
                    ui.add_space(6.0);
                    theme::section(ui, &tr!("Proyecto: {name}", name = ws.project.name()));
                    theme::card(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for file in ["project.toml", "rules.toml", "agents.toml", "routing.toml"] {
                                if theme::secondary(ui, format!("{}  {file}", icon::FILE_TEXT)).clicked() {
                                    cmds.push(UiCmd::EditFile(file));
                                }
                            }
                        });
                        let installed = matches!(
                            forge_core::hooks::status(&ws.project.path),
                            Ok(s) if s.iter().all(|(_, state)| matches!(state, forge_core::hooks::HookState::Installed))
                        );
                        ui.horizontal(|ui| {
                            ui.label(if installed {
                                tr!("Hooks de Git instalados")
                            } else {
                                tr!("Hooks de Git no instalados")
                            });
                            if installed {
                                if theme::secondary(ui, tr!("Quitar")).clicked() {
                                    cmds.push(UiCmd::HooksUninstall);
                                }
                            } else if theme::primary(ui, tr!("Instalar")).clicked() {
                                cmds.push(UiCmd::HooksInstall);
                            }
                        });
                    });
                }
            });
        if modal.should_close() {
            self.settings_open = false;
        }
        if changed {
            self.settings.font_size = self.settings.font_size.round();
            if let Err(e) = crate::save_settings(&self.settings) {
                self.error = Some(e);
            }
        }
    }
}

/// 1234567 → "1.2M", 45200 → "45k".
fn short_tokens(n: u64) -> String {
    match n {
        1_000_000.. => format!("{:.1}M", n as f64 / 1_000_000.0),
        1_000.. => format!("{}k", n / 1_000),
        _ => n.to_string(),
    }
}
