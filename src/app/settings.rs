// Ventana de Ajustes (⌘,), por pestañas: General, Agentes, Uso y Este proyecto. Cada
// opción lleva un ⓘ que explica qué hace; los cambios se guardan solos.
use super::*;
use forge_core::i18n::{Lang, lang};

/// Pestañas de Ajustes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum SettingsTab {
    #[default]
    General,
    Agents,
    Usage,
    Project,
}

impl SettingsTab {
    fn label(self) -> String {
        let (glyph, name) = match self {
            Self::General => (icon::GEAR, tr!("General")),
            Self::Agents => (icon::ROBOT, tr!("Agentes")),
            Self::Usage => (icon::CHART_BAR, tr!("Uso")),
            Self::Project => (icon::FOLDER, tr!("Este proyecto")),
        };
        format!("{glyph}  {name}")
    }
}

fn tab_id() -> egui::Id {
    egui::Id::new("settings-tab")
}

/// Abre Ajustes en una pestaña.
pub(super) fn show_tab(ctx: &egui::Context, tab: SettingsTab) {
    ctx.data_mut(|d| d.insert_temp(tab_id(), tab));
}

/// Desde este ancho la etiqueta va al lado del control; por debajo, encima.
const SIDE_BY_SIDE: f32 = 480.0;

/// Una opción: su nombre con el ⓘ y el control. Si no cabe al lado, va debajo.
fn row(ui: &mut egui::Ui, label: &str, help: &str, add: impl FnOnce(&mut egui::Ui)) {
    let title = |ui: &mut egui::Ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(theme::TEXT));
            theme::help(ui, help);
        });
    };
    if ui.available_width() < SIDE_BY_SIDE {
        title(ui);
        add(ui);
    } else {
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(180.0, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(180.0);
                    title(ui);
                },
            );
            ui.vertical(add);
        });
    }
    ui.add_space(10.0);
}

/// Texto de ayuda en gris, debajo de un control.
fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(theme::TEXT_3));
}

/// Modelos con los que Forge abre Claude Code: (id, nombre, para qué sirve).
fn model_info(id: &str) -> &'static str {
    match id {
        "" => tr!(
            "Usa el modelo que tengas elegido en Claude Code (con /model o en sus ajustes). Forge no lo cambia."
        ),
        "sonnet" => {
            tr!("Rápido y capaz: buen equilibrio para el día a día y gasta menos de tu plan.")
        }
        "opus" => tr!("El más capaz, para lo difícil. Gasta tu plan más rápido."),
        "opusplan" => {
            tr!("Opus piensa el plan y Sonnet lo ejecuta: calidad de Opus con menos gasto.")
        }
        _ => "",
    }
}

impl App {
    pub(super) fn settings_ui(&mut self, ctx: &egui::Context, cmds: &mut Vec<UiCmd>) {
        if !self.settings_open {
            // Al volver a abrir se releen los archivos.
            self.guard_form = None;
            self.agents_form = None;
            self.routing_form = None;
            self.processes_form = None;
            return;
        }
        let mut changed = false;
        // A la medida de la ventana: más ancho si cabe, y nunca más alto que ella (el
        // contenido se desplaza y la cabecera y las pestañas quedan fijas).
        let screen = ctx.content_rect();
        let width = (screen.width() - 64.0).clamp(300.0, 760.0);
        // Mismo alto en todas las pestañas: la ventana no salta al cambiar.
        let body_height = (screen.height() - 190.0).clamp(200.0, 560.0);
        let mut tab: SettingsTab = ctx.data(|d| d.get_temp(tab_id())).unwrap_or_default();
        if tab == SettingsTab::Project && self.active.is_none() {
            tab = SettingsTab::General;
        }
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
                ui.add_space(10.0);
                // Pestañas; en un ancho pequeño, un desplegable.
                let mut tabs = vec![
                    SettingsTab::General,
                    SettingsTab::Agents,
                    SettingsTab::Usage,
                ];
                if self.active.is_some() {
                    tabs.push(SettingsTab::Project);
                }
                if width < 520.0 {
                    egui::ComboBox::from_id_salt("settings-tabs")
                        .selected_text(tab.label())
                        .width(width)
                        .show_ui(ui, |ui| {
                            for t in &tabs {
                                ui.selectable_value(&mut tab, *t, t.label());
                            }
                        });
                } else {
                    ui.horizontal(|ui| {
                        for t in &tabs {
                            ui.selectable_value(&mut tab, *t, RichText::new(t.label()).size(13.5));
                        }
                    });
                }
                ui.separator();
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt(("settings-scroll", tab as u8))
                    .max_height(body_height)
                    .min_scrolled_height(body_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Casillas cuadradas (con el redondeo general parecen radios).
                        let w = &mut ui.visuals_mut().widgets;
                        for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                            state.corner_radius = egui::CornerRadius::same(3);
                        }
                        match tab {
                            SettingsTab::General => changed |= self.general_tab(ui, cmds),
                            SettingsTab::Agents => self.agents_tab(ui, cmds),
                            SettingsTab::Usage => self.usage_tab(ui),
                            SettingsTab::Project => self.project_tab(ui, cmds),
                        }
                        // Versión instalada, al pie (como "Acerca de").
                        ui.add_space(12.0);
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
        ctx.data_mut(|d| d.insert_temp(tab_id(), tab));
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

    /// Idioma, avisos y terminal. Devuelve si cambió algo de `Settings` (se guarda aparte).
    fn general_tab(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) -> bool {
        let mut changed = false;
        theme::section(ui, tr!("General"));
        theme::card(ui, |ui| {
            row(
                ui,
                tr!("Idioma"),
                tr!("Idioma de Forge. Los agentes responden en el idioma en que les escribas."),
                |ui| {
                    ui.horizontal(|ui| {
                        for (l, label) in [(Lang::En, "English"), (Lang::Es, "Español")] {
                            if ui.selectable_label(lang() == l, label).clicked() {
                                cmds.push(UiCmd::SetLang(l));
                            }
                        }
                    });
                },
            );
            row(
                ui,
                tr!("Notificaciones"),
                tr!(
                    "Avisos de macOS cuando Forge está en segundo plano: un agente termina o te necesita, un proceso falla o Guard bloquea un cambio."
                ),
                |ui| {
                    let mut on = self.notifications_enabled;
                    if ui.checkbox(&mut on, tr!("Avisarme")).changed() {
                        self.notifications_enabled = on;
                        self.db(|s| s.set_setting("notifications", if on { "on" } else { "off" }));
                    }
                },
            );
        });
        theme::section(ui, tr!("Terminal"));
        theme::card(ui, |ui| {
            row(
                ui,
                tr!("Tamaño de letra"),
                tr!(
                    "Tamaño del texto de las terminales. También con ⌘+ y ⌘−; ⌘0 vuelve al normal."
                ),
                |ui| {
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.settings.font_size, 9.0..=28.0)
                                .step_by(1.0)
                                .fixed_decimals(0)
                                .trailing_fill(true)
                                .suffix(" pt"),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .changed();
                },
            );
            row(
                ui,
                tr!("Tecla Option"),
                tr!(
                    "Actívalo para usar ⌥ como Meta en la terminal (⌥B y ⌥F saltan palabras). Desactívalo si usas ⌥ para escribir caracteres como @ o #."
                ),
                |ui| {
                    changed |= ui
                        .checkbox(&mut self.settings.option_as_meta, tr!("Usar como Meta"))
                        .changed();
                },
            );
            hint(
                ui,
                tr!("Fuentes propias y más opciones: ~/.config/forge/config.toml"),
            );
        });
        changed
    }

    /// Modelo de Claude y cuentas de los agentes.
    fn agents_tab(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
        theme::section(ui, "Claude Code");
        theme::card(ui, |ui| {
            row(
                ui,
                tr!("Modelo"),
                tr!(
                    "Con qué modelo se abre Claude Code desde Forge (barra lateral, ⌘⇧A, ⌘K). Un claude escrito a mano en la terminal usa lo que escribas."
                ),
                |ui| {
                    let current = crate::claude_plugin::CLAUDE_MODEL
                        .lock()
                        .map(|m| m.clone())
                        .unwrap_or_default();
                    let name = |v: &str| {
                        crate::claude_plugin::MODELS
                            .iter()
                            .find(|(id, _)| *id == v)
                            .map_or(v.to_string(), |(_, l)| forge_core::i18n::t(l).to_string())
                    };
                    egui::ComboBox::from_id_salt("claude-model")
                        .selected_text(name(&current))
                        .width(ui.available_width().min(320.0))
                        .show_ui(ui, |ui| {
                            for (id, label) in crate::claude_plugin::MODELS {
                                if ui
                                    .selectable_label(current == id, forge_core::i18n::t(label))
                                    .on_hover_text(model_info(id))
                                    .clicked()
                                {
                                    if let Ok(mut m) = crate::claude_plugin::CLAUDE_MODEL.lock() {
                                        *m = id.to_string();
                                    }
                                    self.db(|s| s.set_setting("claude_model", id));
                                }
                            }
                        });
                    hint(ui, model_info(&current));
                },
            );
        });
        self.accounts_ui(ui, cmds);
    }

    /// A dónde van los tokens (Claude Code abierto desde Forge, 30 días).
    fn usage_tab(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, tr!("Uso de modelos (30 días)"));
        let usage = self
            .db(|s| s.agent_usage(None, store::now() - 30 * 86_400))
            .unwrap_or_default();
        theme::card(ui, |ui| {
            if usage.is_empty() {
                hint(
                    ui,
                    tr!(
                        "Aún no hay uso registrado. Aparece al usar Claude Code abierto desde Forge."
                    ),
                );
                return;
            }
            let total: u64 = usage.iter().map(|u| u.total()).sum::<u64>().max(1);
            for u in &usage {
                ui.horizontal_wrapped(|ui| {
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
                });
                ui.add_space(4.0);
            }
            hint(
                ui,
                tr!(
                    "Claude Code abierto desde Forge delega búsquedas en Haiku y revisiones en Sonnet; aquí ves a dónde van los tokens."
                ),
            );
        });
    }

    /// Configuración del proyecto abierto (carpeta .forge/) y hooks de Git.
    fn project_tab(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
        let Some(i) = self.active else { return };
        let ws = &self.workspaces[i];
        theme::section(ui, &ws.project.name());
        theme::card(ui, |ui| {
            hint(
                ui,
                tr!(
                    "Se guarda en la carpeta .forge/ del proyecto: súbela al repositorio para compartirla con tu equipo."
                ),
            );
            ui.add_space(6.0);
            // Cada archivo por lo que hace (el nombre técnico, en gris).
            let files = [(
                icon::SQUARES_FOUR,
                tr!("Archivo del proyecto"),
                tr!(
                    "Nombre, distribución de paneles y variables de entorno (lo avanzado se edita en el archivo)."
                ),
                "project.toml",
            )];
            let narrow = ui.available_width() < SIDE_BY_SIDE;
            for (glyph, title, what, file) in files {
                let exists = ws.project.path.join(".forge").join(file).exists();
                let text = |ui: &mut egui::Ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(glyph).size(16.0).color(theme::ACCENT));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(title).color(theme::TEXT));
                                ui.label(RichText::new(file).size(11.0).color(theme::TEXT_4));
                            });
                            ui.label(RichText::new(what).size(11.5).color(theme::TEXT_3));
                        });
                    });
                };
                let button = |ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>| {
                    if file == "project.toml" && !ws.project.has_config() {
                        if theme::primary(ui, tr!("Crear"))
                            .on_hover_text(tr!(
                                "Lo crea con los scripts que encuentre en package.json"
                            ))
                            .clicked()
                        {
                            cmds.push(UiCmd::CreateConfig);
                        }
                    } else {
                        let (label, tip) = if exists {
                            (tr!("Abrir archivo"), tr!("Lo abre en tu editor de texto"))
                        } else {
                            (
                                tr!("Crear con plantilla"),
                                tr!(
                                    "Lo crea con una plantilla comentada (con ejemplos) y lo abre en tu editor"
                                ),
                            )
                        };
                        if theme::secondary(ui, label).on_hover_text(tip).clicked() {
                            cmds.push(UiCmd::EditFile(file));
                        }
                    }
                };
                if narrow {
                    text(ui);
                    button(ui, cmds);
                } else {
                    ui.horizontal(|ui| {
                        text(ui);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            button(ui, cmds);
                        });
                    });
                }
                ui.add_space(6.0);
            }
            ui.horizontal_wrapped(|ui| {
                if ws.project.has_config()
                    && theme::secondary(
                        ui,
                        format!("{}  {}", icon::ARROW_CLOCKWISE, tr!("Recargar tras editar")),
                    )
                    .on_hover_text(tr!(
                        "Vuelve a leer los archivos de .forge/ si los cambiaste a mano"
                    ))
                    .clicked()
                {
                    cmds.push(UiCmd::ReloadConfig);
                }
                if ws.project.default_layout().is_some()
                    && theme::secondary(
                        ui,
                        format!(
                            "{}  {}",
                            icon::SQUARES_FOUR,
                            tr!("Restablecer la distribución de paneles")
                        ),
                    )
                    .clicked()
                {
                    cmds.push(UiCmd::ResetLayout);
                }
            });
        });
        self.processes_form_ui(ui, cmds);
        self.guard_form_ui(ui, cmds);
        self.agents_form_ui(ui, cmds);
        self.routing_form_ui(ui, cmds);
        let Some(i) = self.active else { return };
        let ws = &self.workspaces[i];
        theme::section(ui, tr!("Hooks de Git"));
        theme::card(ui, |ui| {
            let installed = matches!(
                forge_core::hooks::status(&ws.project.path),
                Ok(s) if s.iter().all(|(_, state)| matches!(state, forge_core::hooks::HookState::Installed))
            );
            row(
                ui,
                tr!("Guard en cada commit y push"),
                tr!(
                    "Instala hooks de Git que pasan Project Guard antes de cada commit y push, también si los haces desde fuera de Forge. Si algo no cumple las reglas, Git no deja continuar."
                ),
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(if installed {
                            RichText::new(tr!("Instalados")).color(theme::GREEN)
                        } else {
                            RichText::new(tr!("No instalados")).color(theme::TEXT_3)
                        });
                        if installed {
                            if theme::secondary(ui, tr!("Quitar")).clicked() {
                                cmds.push(UiCmd::HooksUninstall);
                            }
                        } else if theme::primary(ui, tr!("Instalar")).clicked() {
                            cmds.push(UiCmd::HooksInstall);
                        }
                    });
                },
            );
        });
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
                for (n, a) in accounts.iter().enumerate() {
                    if n > 0 {
                        ui.separator();
                    }
                    let limits = match program {
                        // Codex lo guarda en sus sesiones; se relee cada 10 s.
                        "codex" => codex_limits_cached(ui, a),
                        _ => self
                            .store
                            .as_ref()
                            .and_then(|s| s.account_limits(program, a.id)),
                    };
                    let tokens = usage.get(&a.id).copied();
                    account_row(ui, cmds, program, a, chosen, ws.is_some());
                    account_usage(ui, limits.as_ref(), tokens);
                    ui.add_space(4.0);
                }
                ui.add_space(4.0);
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
    ui.horizontal_wrapped(|ui| {
        for w in windows {
            let label = match w.kind.as_str() {
                "five_hour" => "5h",
                "seven_day" => tr!("semana"),
                "30d" => tr!("30 días"),
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

/// Una cuenta: su nombre (se cambia escribiendo; se guarda al salir del campo o con Enter),
/// y lo que se puede hacer con ella. Todo baja de línea si no cabe.
fn account_row(
    ui: &mut egui::Ui,
    cmds: &mut Vec<UiCmd>,
    program: &str,
    a: &forge_core::accounts::Account,
    chosen: Option<i64>,
    in_project: bool,
) {
    let id = egui::Id::new(("account-name", program, a.id));
    let saved = egui::Id::new(("account-saved", program, a.id));
    let mut text = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| a.name.clone());
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::PENCIL_SIMPLE).color(theme::TEXT_3))
            .on_hover_text(tr!("Escribe para cambiar el nombre"));
        // Deja sitio a la papelera; el campo ocupa el resto (nunca menos de 120).
        let width =
            (ui.available_width() - if a.is_main() { 90.0 } else { 40.0 }).clamp(120.0, 320.0);
        let edit = ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(width)
                    .hint_text(tr!("Nombre de la cuenta")),
            )
            .on_hover_text(tr!(
                "Escribe para cambiar el nombre; se guarda al salir del campo o con Enter"
            ));
        if edit.changed() {
            ui.data_mut(|d| d.insert_temp(id, text.clone()));
        }
        if edit.lost_focus() {
            if !text.trim().is_empty() && text.trim() != a.name {
                cmds.push(UiCmd::AccountRename(program.into(), a.id, text.clone()));
                ui.data_mut(|d| d.insert_temp(saved, ui.input(|i| i.time)));
            }
            ui.data_mut(|d| d.remove::<String>(id));
        }
        // "✓ Guardado" un par de segundos tras renombrar.
        let now = ui.input(|i| i.time);
        if ui
            .data(|d| d.get_temp::<f64>(saved))
            .is_some_and(|t| now - t < 2.0)
        {
            ui.label(
                RichText::new(tr!("✓ Guardado"))
                    .size(11.5)
                    .color(theme::GREEN),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        } else if a.is_main() {
            ui.label(
                RichText::new(tr!("principal"))
                    .size(11.0)
                    .color(theme::TEXT_3),
            )
            .on_hover_text(tr!(
                "La cuenta de siempre (~/.claude o ~/.codex). No se puede borrar."
            ));
        }
        if !a.is_main() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let confirm = egui::Id::new(("account-delete", program, a.id));
                if !ui.data(|d| d.get_temp::<bool>(confirm)).unwrap_or(false)
                    && theme::icon_button(ui, icon::TRASH, tr!("Borrar cuenta")).clicked()
                {
                    ui.data_mut(|d| d.insert_temp(confirm, true));
                }
            });
        }
    });
    // Confirmación de borrado, en su propia línea.
    let confirm = egui::Id::new(("account-delete", program, a.id));
    if ui.data(|d| d.get_temp::<bool>(confirm)).unwrap_or(false) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(tr!("Se borran su inicio de sesión y su historial."))
                    .color(theme::TEXT_2),
            );
            if ui
                .button(RichText::new(tr!("Borrar")).color(theme::RED))
                .clicked()
            {
                cmds.push(UiCmd::AccountDelete(program.into(), a.id));
                ui.data_mut(|d| d.remove::<bool>(confirm));
            }
            if ui.button(tr!("Cancelar")).clicked() {
                ui.data_mut(|d| d.remove::<bool>(confirm));
            }
        });
    }
    if in_project {
        ui.horizontal_wrapped(|ui| {
            let here = chosen == Some(a.id);
            let label = if here {
                format!("{}  {}", icon::CHECK, tr!("Usada en este proyecto"))
            } else {
                tr!("Usar en este proyecto").to_string()
            };
            if ui
                .selectable_label(here, label)
                .on_hover_text(tr!(
                    "Al pulsar el agente en este proyecto se abre con esta cuenta"
                ))
                .clicked()
            {
                cmds.push(UiCmd::UseAccount(program.into(), a.id));
            }
            if ui
                .button(tr!("Iniciar sesión"))
                .on_hover_text(tr!(
                    "Abre el agente con esta cuenta para que inicies sesión (solo la primera vez)"
                ))
                .clicked()
            {
                cmds.push(UiCmd::AccountLogin(program.into(), a.id));
            }
        });
    }
}

/// Uso del plan de una cuenta de Codex, leído de sus sesiones como mucho cada 10 s.
fn codex_limits_cached(
    ui: &egui::Ui,
    a: &forge_core::accounts::Account,
) -> Option<forge_core::accounts::AccountLimits> {
    type Cached = (f64, Option<forge_core::accounts::AccountLimits>);
    let id = egui::Id::new(("codex-limits", a.id));
    let now = ui.input(|i| i.time);
    if let Some((at, limits)) = ui.data(|d| d.get_temp::<Cached>(id))
        && now - at < 10.0
    {
        return limits;
    }
    let limits = forge_core::sessions::codex_limits(a.dir.as_deref());
    ui.data_mut(|d| d.insert_temp::<Cached>(id, (now, limits.clone())));
    limits
}
