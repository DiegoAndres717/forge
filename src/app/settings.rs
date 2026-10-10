// Ventana de Ajustes (⌘,): idioma, notificaciones, terminal y atajos al proyecto actual.
use super::*;
use forge_core::i18n::{Lang, lang};

impl App {
    pub(super) fn settings_ui(&mut self, ctx: &egui::Context, cmds: &mut Vec<UiCmd>) {
        if !self.settings_open {
            return;
        }
        let mut changed = false;
        // A la medida de la ventana: más ancho si cabe, y nunca más alto que ella (el
        // contenido se desplaza y la cabecera queda fija).
        let screen = ctx.content_rect();
        let width = (screen.width() - 64.0).clamp(340.0, 760.0);
        let max_height = (screen.height() - 140.0).max(200.0);
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::SEPARATOR))
                    .corner_radius(12.0)
                    .inner_margin(20.0),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
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
                egui::ScrollArea::vertical()
                    .id_salt("settings-scroll")
                    .max_height(max_height)
                    // Si no cabe, que use toda la altura disponible (por defecto se encoge).
                    .min_scrolled_height(max_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
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
                        ui.label(tr!("Modelo de Claude"));
                        let current = crate::claude_plugin::CLAUDE_MODEL.lock().map(|m| m.clone()).unwrap_or_default();
                        let label = |v: &str| {
                            crate::claude_plugin::MODELS
                                .iter()
                                .find(|(id, _)| *id == v)
                                .map_or(v.to_string(), |(_, l)| forge_core::i18n::t(l).to_string())
                        };
                        egui::ComboBox::from_id_salt("claude-model")
                            .selected_text(label(&current))
                            .show_ui(ui, |ui| {
                                for (id, name) in crate::claude_plugin::MODELS {
                                    if ui.selectable_label(current == id, forge_core::i18n::t(name)).clicked() {
                                        if let Ok(mut m) = crate::claude_plugin::CLAUDE_MODEL.lock() {
                                            *m = id.to_string();
                                        }
                                        self.db(|s| s.set_setting("claude_model", id));
                                    }
                                }
                            })
                            .response
                            .on_hover_text(tr!("Al abrir Claude desde Forge (barra lateral, ⌘⇧A). Un claude escrito a mano usa lo que escribas."));
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

                self.accounts_ui(ui, cmds);

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
                    theme::section(ui, &tr!("Este proyecto · {name}", name = ws.project.name()));
                    theme::card(ui, |ui| {
                        ui.label(
                            RichText::new(tr!("Se guarda en la carpeta .forge/ del proyecto: súbela al repositorio para compartirla con tu equipo."))
                                .size(11.5)
                                .color(theme::TEXT_3),
                        );
                        ui.add_space(4.0);
                        // Cada archivo por lo que hace (el nombre técnico, en gris).
                        let files = [
                            (icon::SQUARES_FOUR, tr!("Distribución y procesos"), tr!("Qué terminales se abren y qué procesos (dev, api…) tiene el proyecto."), "project.toml"),
                            (icon::SHIELD_CHECK, tr!("Reglas de Guard"), tr!("Qué se revisa antes de un commit, un push o un pull request."), "rules.toml"),
                            (icon::ROBOT, tr!("Agentes"), tr!("Agentes propios y cómo se abren."), "agents.toml"),
                            (icon::SPARKLE, tr!("Revisión con IA"), tr!("Qué modelos revisan los cambios y con qué presupuesto."), "routing.toml"),
                        ];
                        for (glyph, title, what, file) in files {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(glyph).size(16.0).color(theme::ACCENT));
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(title).color(theme::TEXT));
                                        ui.label(RichText::new(file).size(11.0).color(theme::TEXT_4));
                                    });
                                    ui.label(RichText::new(what).size(11.5).color(theme::TEXT_3));
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // Sin project.toml: se crea con los scripts de package.json.
                                    if file == "project.toml" && !ws.project.has_config() {
                                        if theme::primary(ui, tr!("Crear")).on_hover_text(tr!("Detecta los scripts de package.json")).clicked() {
                                            cmds.push(UiCmd::CreateConfig);
                                        }
                                    } else if theme::secondary(ui, tr!("Editar")).clicked() {
                                        cmds.push(UiCmd::EditFile(file));
                                    }
                                });
                            });
                        }
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            if ws.project.has_config()
                                && theme::secondary(ui, format!("{}  {}", icon::ARROW_CLOCKWISE, tr!("Recargar tras editar"))).clicked()
                            {
                                cmds.push(UiCmd::ReloadConfig);
                            }
                            if ws.project.default_layout().is_some()
                                && theme::secondary(ui, format!("{}  {}", icon::SQUARES_FOUR, tr!("Restablecer la distribución de paneles"))).clicked()
                            {
                                cmds.push(UiCmd::ResetLayout);
                            }
                        });
                        ui.separator();
                        let installed = matches!(
                            forge_core::hooks::status(&ws.project.path),
                            Ok(s) if s.iter().all(|(_, state)| matches!(state, forge_core::hooks::HookState::Installed))
                        );
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(if installed {
                                    tr!("Hooks de Git instalados")
                                } else {
                                    tr!("Hooks de Git no instalados")
                                });
                                ui.label(
                                    RichText::new(tr!("Pasan Guard solo antes de cada commit y push."))
                                        .size(11.5)
                                        .color(theme::TEXT_3),
                                );
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

                // Versión instalada, al pie (como "Acerca de").
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(format!("Forge {}", env!("CARGO_PKG_VERSION")))
                            .size(11.5)
                            .color(theme::TEXT_3),
                    );
                    if ui
                        .link(RichText::new(tr!("Buscar actualizaciones")).size(11.5))
                        .clicked()
                    {
                        cmds.push(UiCmd::CheckUpdates);
                    }
                });
                    });
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

impl App {
    /// Cuentas de Claude Code y Codex: varias a la vez, cada una con su login.
    fn accounts_ui(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
        let detected = self.agents.lock().map(|d| d.clone()).unwrap_or_default();
        let programs: Vec<(&str, &str)> = [("claude", "Claude Code"), ("codex", "Codex")]
            .into_iter()
            .filter(|(p, _)| detected.get(*p).is_some_and(|d| d.path.is_some()))
            .collect();
        if programs.is_empty() {
            return;
        }
        let ws = self.active.map(|i| &self.workspaces[i]);
        ui.add_space(6.0);
        theme::section(ui, tr!("Cuentas"));
        theme::card(ui, |ui| {
            ui.label(
                RichText::new(tr!("Usa varias cuentas a la vez sin cerrar sesión. Cada una guarda su inicio de sesión e historial; los ajustes, las instrucciones y las skills se comparten con la principal."))
                    .size(11.5)
                    .color(theme::TEXT_3),
            );
            for (program, label) in programs {
                ui.add_space(8.0);
                ui.label(RichText::new(label).strong());
                let accounts = self
                    .store
                    .as_ref()
                    .map(|s| s.accounts(program))
                    .unwrap_or_default();
                let chosen = ws.and_then(|w| w.account_for(program, None)).map(|a| a.id);
                // Solo Claude Code (con el mod) cuenta tokens y manda el uso del plan.
                let usage: HashMap<i64, u64> = match (program, self.store.as_ref()) {
                    ("claude", Some(s)) => s
                        .usage_by_account(store::now() - 30 * 86_400)
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                    _ => HashMap::new(),
                };
                egui::Grid::new(("accounts", program))
                    .num_columns(3)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for a in &accounts {
                            let id = egui::Id::new(("account-name", program, a.id));
                            let mut text = ui
                                .data(|d| d.get_temp::<String>(id))
                                .unwrap_or_else(|| a.name.clone());
                            let limits = self
                                .store
                                .as_ref()
                                .and_then(|s| s.account_limits(program, a.id));
                            let tokens = usage.get(&a.id).copied();
                            let edit = ui
                                .vertical(|ui| {
                                    let edit = ui
                                        .add(egui::TextEdit::singleline(&mut text).desired_width(150.0))
                                        .on_hover_text(tr!("Nombre de la cuenta"));
                                    if program == "claude" {
                                        account_usage(ui, limits.as_ref(), tokens);
                                    }
                                    edit
                                })
                                .inner;
                            if edit.changed() {
                                ui.data_mut(|d| d.insert_temp(id, text.clone()));
                            }
                            if edit.lost_focus() {
                                if !text.trim().is_empty() && text.trim() != a.name {
                                    cmds.push(UiCmd::AccountRename(program.into(), a.id, text.clone()));
                                }
                                ui.data_mut(|d| d.remove::<String>(id));
                            }
                            ui.horizontal(|ui| {
                                if ws.is_some() {
                                    if ui
                                        .selectable_label(chosen == Some(a.id), tr!("En este proyecto"))
                                        .on_hover_text(tr!("Se abre con esta cuenta al pulsar el agente en este proyecto"))
                                        .clicked()
                                    {
                                        cmds.push(UiCmd::UseAccount(program.into(), a.id));
                                    }
                                    if ui
                                        .button(tr!("Iniciar sesión"))
                                        .on_hover_text(tr!("Abre el agente con esta cuenta para que inicies sesión (solo la primera vez)"))
                                        .clicked()
                                    {
                                        cmds.push(UiCmd::AccountLogin(program.into(), a.id));
                                    }
                                }
                                if a.is_main() {
                                    ui.label(RichText::new(tr!("principal")).size(11.0).color(theme::TEXT_3));
                                }
                            });
                            if a.is_main() {
                                ui.label("");
                            } else {
                                let confirm = egui::Id::new(("account-delete", program, a.id));
                                let asked = ui.data(|d| d.get_temp::<bool>(confirm)).unwrap_or(false);
                                if asked {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .button(RichText::new(tr!("Borrar su login e historial")).color(theme::RED))
                                            .clicked()
                                        {
                                            cmds.push(UiCmd::AccountDelete(program.into(), a.id));
                                            ui.data_mut(|d| d.remove::<bool>(confirm));
                                        }
                                        if ui.button(tr!("Cancelar")).clicked() {
                                            ui.data_mut(|d| d.remove::<bool>(confirm));
                                        }
                                    });
                                } else if theme::icon_button(ui, icon::TRASH, tr!("Borrar cuenta")).clicked() {
                                    ui.data_mut(|d| d.insert_temp(confirm, true));
                                }
                            }
                            ui.end_row();
                        }
                    });
                if ui
                    .button(tr!("{p0}  Añadir cuenta", p0 = icon::PLUS))
                    .clicked()
                {
                    cmds.push(UiCmd::AccountAdd(program.into()));
                }
            }
        });
    }
}

/// Uso del plan de una cuenta (5 h y semanal, de la última vez que se usó desde Forge) y
/// sus tokens de los últimos 30 días.
fn account_usage(
    ui: &mut egui::Ui,
    limits: Option<&forge_core::accounts::AccountLimits>,
    tokens: Option<u64>,
) {
    let windows = limits.map_or(&[][..], |l| l.windows.as_slice());
    if windows.is_empty() && tokens.is_none() {
        ui.label(
            RichText::new(tr!("Sin uso todavía"))
                .size(11.0)
                .color(theme::TEXT_4),
        )
        .on_hover_text(tr!("Aparece después de usar la cuenta desde Forge"));
        return;
    }
    ui.horizontal(|ui| {
        for w in windows {
            let label = match w.kind.as_str() {
                "five_hour" => "5h",
                "seven_day" => tr!("semana"),
                other => other,
            };
            let used = (w.percent_used / 100.0).clamp(0.0, 1.0) as f32;
            let color = if used >= 0.8 {
                theme::RED
            } else {
                theme::ORANGE
            };
            ui.label(RichText::new(label).size(11.0).color(theme::TEXT_3));
            let (bar, _) = ui.allocate_exact_size(Vec2::new(48.0, 5.0), Sense::hover());
            ui.painter().rect_filled(bar, 2.5, theme::SEPARATOR);
            let mut fill = bar;
            fill.set_width((bar.width() * used).max(if used > 0.0 { 3.0 } else { 0.0 }));
            ui.painter().rect_filled(fill, 2.5, color);
            ui.label(
                RichText::new(format!("{:.0}%", w.percent_used))
                    .size(11.0)
                    .color(if used >= 0.8 {
                        theme::RED
                    } else {
                        theme::TEXT_2
                    }),
            );
            ui.add_space(6.0);
        }
        if let Some(n) = tokens {
            ui.label(
                RichText::new(tr!("{p0} tokens · 30 días", p0 = short_tokens(n)))
                    .size(11.0)
                    .color(theme::TEXT_3),
            );
        }
    })
    .response
    .on_hover_text(match limits {
        Some(l) => tr!("Uso del plan visto {p0}", p0 = store::ago_precise(l.at)),
        None => tr!("Tokens usados desde Forge en 30 días").into(),
    });
}
