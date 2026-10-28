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
    Alerts,
    Usage,
    Project,
}

impl SettingsTab {
    fn parts(self) -> (&'static str, &'static str) {
        match self {
            Self::General => (icon::GEAR, tr!("General")),
            Self::Agents => (icon::ROBOT, tr!("Agentes")),
            Self::Alerts => (icon::BELL, tr!("Avisos")),
            Self::Usage => (icon::CHART_BAR, tr!("Uso")),
            Self::Project => (icon::FOLDER, tr!("Este proyecto")),
        }
    }

    /// Las pestañas que hay: «Este proyecto» solo con un proyecto abierto.
    fn all(has_project: bool) -> Vec<Self> {
        let mut tabs = vec![Self::General, Self::Agents, Self::Alerts, Self::Usage];
        if has_project {
            tabs.push(Self::Project);
        }
        tabs
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

/// Si la ficha de una cuenta (modelo y esfuerzo) está abierta.
fn config_id(program: &str, account: i64) -> egui::Id {
    egui::Id::new(("account-config", program, account))
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
    /// La pestaña elegida (Este proyecto solo existe con un proyecto abierto).
    fn current_tab(&self, ctx: &egui::Context) -> SettingsTab {
        let tab: SettingsTab = ctx.data(|d| d.get_temp(tab_id())).unwrap_or_default();
        if tab == SettingsTab::Project && self.active.is_none() {
            SettingsTab::General
        } else {
            tab
        }
    }

    /// La barra lateral mientras Ajustes está abierto: volver y las secciones.
    pub(super) fn settings_nav(&mut self, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
        let ctx = ui.ctx().clone();
        let tab = self.current_tab(&ctx);
        let body = Rect::from_min_max(
            rect.min + Vec2::new(10.0, 8.0),
            rect.max - Vec2::new(10.0, 8.0),
        );
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
        ui.spacing_mut().item_spacing.y = 1.0;
        if theme::row(
            &mut ui,
            icon::ARROW_LEFT,
            theme::TEXT_2,
            tr!("Volver"),
            "Esc",
            false,
        )
        .clicked()
        {
            cmds.push(UiCmd::CloseSettings);
        }
        theme::section(&mut ui, tr!("Ajustes"));
        for t in SettingsTab::all(self.active.is_some()) {
            let (glyph, name) = t.parts();
            if theme::row(&mut ui, glyph, theme::TEXT_2, name, "", t == tab).clicked() {
                show_tab(&ctx, t);
            }
        }
        ui.add_space(12.0);
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
    }

    /// Ajustes como página que cubre el espacio de trabajo (las secciones están en la barra lateral).
    pub(super) fn settings_ui(&mut self, ctx: &egui::Context, area: Rect, cmds: &mut Vec<UiCmd>) {
        if !self.settings_open {
            // Al volver a abrir se releen los archivos.
            self.guard_form = None;
            self.agents_form = None;
            self.routing_form = None;
            self.processes_form = None;
            return;
        }
        let mut changed = false;
        // Esc cierra, salvo que haya un desplegable abierto (Esc lo cierra a él).
        if ctx.input(|i| i.key_pressed(Key::Escape)) && !egui::Popup::is_any_open(ctx) {
            self.settings_open = false;
        }
        let width = (area.width() - 64.0).clamp(300.0, 760.0);
        let tab = self.current_tab(ctx);
        egui::Area::new(egui::Id::new("settings-page"))
            .order(egui::Order::Foreground)
            .fixed_pos(area.min)
            .show(ctx, |ui| {
                // Tapa el espacio de trabajo y se come los clics que irían debajo.
                ui.allocate_rect(area, Sense::click_and_drag());
                ui.painter().rect_filled(area, 0.0, theme::BG);
                let page = Rect::from_min_size(
                    egui::pos2(area.center().x - width / 2.0, area.min.y + 20.0),
                    Vec2::new(width, (area.height() - 40.0).max(100.0)),
                );
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(page));
                let (glyph, name) = tab.parts();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(glyph).size(18.0).color(theme::TEXT_2));
                    ui.label(RichText::new(name).size(16.0).strong());
                });
                ui.separator();
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt(("settings-scroll", tab as u8))
                    .auto_shrink([false, false])
                    .show(&mut ui, |ui| {
                        // Casillas cuadradas (con el redondeo general parecen radios).
                        let w = &mut ui.visuals_mut().widgets;
                        for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                            state.corner_radius = egui::CornerRadius::same(3);
                        }
                        match tab {
                            SettingsTab::General => changed |= self.general_tab(ui, cmds),
                            SettingsTab::Agents => self.agents_tab(ui, cmds),
                            SettingsTab::Alerts => self.alerts_tab(ui),
                            SettingsTab::Usage => self.usage_tab(ui),
                            SettingsTab::Project => self.project_tab(ui, cmds),
                        }
                    });
            });
        if changed {
            self.settings.font_size = self.settings.font_size.round();
            if let Err(e) = crate::save_settings(&self.settings) {
                self.error = Some(e);
            }
        }
    }

    /// Con qué se trabaja: el agente por defecto, el shell de las terminales y si los agentes
    /// editan sin preguntar a cada paso. Va en Ajustes → General y en el último paso de la
    /// bienvenida; se guarda al elegir y vale para todos los proyectos.
    pub(super) fn work_defaults_ui(&mut self, ui: &mut egui::Ui) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let detected = self.agents.lock().map(|d| d.clone()).unwrap_or_default();
        let (agent, shell, accept) = (store.preferred_agent(), store.shell(), store.accept_edits());
        let (mut new_agent, mut new_shell, mut new_accept) = (agent.clone(), shell.clone(), accept);
        let installed: Vec<_> = forge_core::agents::builtins()
            .into_iter()
            .filter(|a| detected.get(a.program()).is_some_and(|d| d.path.is_some()))
            .collect();
        row(
            ui,
            tr!("Agente por defecto"),
            tr!(
                "El que se abre con ⌘⇧A y lleva la estrella en la barra lateral. Un agente marcado en el propio proyecto (agents.toml) gana a este."
            ),
            |ui| {
                let label = installed.iter().find(|a| a.program() == agent).map_or_else(
                    || tr!("Automático (el primero instalado)").to_string(),
                    |a| a.name.clone(),
                );
                egui::ComboBox::from_id_salt("default-agent")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut new_agent,
                            String::new(),
                            tr!("Automático (el primero instalado)"),
                        );
                        for a in &installed {
                            ui.selectable_value(&mut new_agent, a.program().to_string(), &a.name);
                        }
                    });
            },
        );
        let shells = forge_core::available_shells();
        if !shells.is_empty() {
            row(
                ui,
                tr!("Terminal"),
                tr!(
                    "El shell de las terminales nuevas de Forge. Forge detecta los instalados; «del sistema» usa el que tengas por defecto en macOS. Se aplica a las terminales que abras a partir de ahora."
                ),
                |ui| {
                    let label = if shell.is_empty() {
                        tr!("Del sistema").to_string()
                    } else {
                        shell.clone()
                    };
                    egui::ComboBox::from_id_salt("default-shell")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut new_shell, String::new(), tr!("Del sistema"));
                            for s in &shells {
                                ui.selectable_value(&mut new_shell, s.clone(), s);
                            }
                        });
                },
            );
        }
        row(
            ui,
            tr!("Permisos"),
            tr!(
                "Los agentes editan archivos sin pedir permiso a cada paso, pero siguen preguntando por lo demás (comandos, red…): Claude con «aceptar ediciones», Codex con espacio de trabajo y aprobación a petición, Antigravity con «accept-edits». Guard sigue vigilando. Se aplica a las sesiones que abras a partir de ahora; OpenCode y los demás no tienen un modo equivalente y no cambian."
            ),
            |ui| {
                ui.checkbox(&mut new_accept, tr!("Aceptar ediciones sin preguntar"));
            },
        );
        if new_agent == agent && new_shell == shell && new_accept == accept {
            return;
        }
        let saved = store
            .set_launch_pref("forge", "default_agent", None, &new_agent)
            .and_then(|()| store.set_launch_pref("forge", "shell", None, &new_shell))
            .and_then(|()| {
                store.set_launch_pref(
                    "forge",
                    "permissions",
                    None,
                    if new_accept { "accept-edits" } else { "ask" },
                )
            });
        if let Err(e) = saved {
            self.error = Some(e);
        }
        self.sync_accounts();
    }

    /// Idioma, avisos y terminal. Devuelve si cambió algo de `Settings` (se guarda aparte).
    fn general_tab(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) -> bool {
        let mut changed = false;
        theme::section(ui, tr!("Cómo trabajas"));
        theme::card(ui, |ui| self.work_defaults_ui(ui));
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
        });
        theme::section(ui, "Forge Memory");
        theme::card(ui, |ui| self.semantic_ui(ui));
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
                if cfg!(target_os = "macos") {
                    tr!("Tecla Option")
                } else {
                    tr!("Tecla Alt")
                },
                tr!(
                    "Actívalo para usar ⌥ como Meta en la terminal (⌥B y ⌥F saltan palabras). Desactívalo si usas ⌥ para escribir caracteres como @ o #."
                ),
                |ui| {
                    changed |= ui
                        .checkbox(&mut self.settings.option_is_meta, tr!("Usar como Meta"))
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
        let detected = self.agents.lock().map(|d| d.clone()).unwrap_or_default();
        let installed = |p: &str| detected.get(p).is_some_and(|d| d.path.is_some());
        // Primero lo que vale para todos los agentes.
        theme::section(ui, tr!("Cómo trabajan"));
        let mut ask = self.settings.ask_first;
        theme::card(ui, |ui| {
            row(
                ui,
                tr!("Primero pregunta"),
                tr!(
                    "Ante una petición grande, el agente hace hasta tres preguntas para aclarar lo imprescindible y luego crea el plan por fases antes de escribir código. Vale para todos los agentes y se aplica a los que abras a partir de ahora."
                ),
                |ui| {
                    ui.checkbox(&mut ask, tr!("Aclarar antes de planificar"));
                },
            );
            self.tests_first_ui(ui);
            self.jev_ui(ui);
            self.advisor_ui(ui);
        });
        if ask != self.settings.ask_first {
            self.settings.ask_first = ask;
            self.sync_accounts();
            if let Err(e) = crate::save_settings(&self.settings) {
                self.error = Some(e);
            }
        }
        if installed("codex") {
            theme::section(ui, "Codex");
            if let Some(home) = forge_core::home_dir() {
                let codex = home.join(".codex");
                theme::card(ui, |ui| self.codex_skills_ui(ui, &codex));
            }
            let codex = detected.get("codex").and_then(|d| d.path.clone());
            theme::card(ui, |ui| {
                self.subagents_ui(ui, forge_core::subagents::Agent::Codex, codex.as_deref())
            });
        }
        if installed("agy") {
            theme::section(ui, "Antigravity");
            theme::card(ui, |ui| self.launch_ui(ui, "agy", None));
        }
        if installed("gemini") {
            theme::section(ui, "Gemini CLI");
            theme::card(ui, |ui| {
                self.subagents_ui(ui, forge_core::subagents::Agent::Gemini, None)
            });
        }
        if installed("opencode") {
            theme::section(ui, "OpenCode");
            theme::card(ui, |ui| {
                self.subagents_ui(ui, forge_core::subagents::Agent::OpenCode, None)
            });
        }
        if installed("qwen") {
            theme::section(ui, "Qwen Code");
            theme::card(ui, |ui| {
                self.subagents_ui(ui, forge_core::subagents::Agent::Qwen, None)
            });
        }
        self.accounts_ui(ui, cmds);
    }

    /// «Primero el test»: general y, con un proyecto abierto, lo de ese proyecto.
    fn tests_first_ui(&mut self, ui: &mut egui::Ui) {
        let project = self.active.map(|i| self.workspaces[i].project.path.clone());
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let general = store.launch_pref("forge", "tests_first", None) == "on";
        let own = project
            .as_deref()
            .map(|p| store.launch_pref("forge", "tests_first", Some(p)));
        let mut set: Option<(Option<&std::path::Path>, &str)> = None;
        row(
            ui,
            tr!("Primero el test"),
            tr!(
                "Para cada cambio de comportamiento o arreglo, el agente escribe primero un test que falla, luego el código mínimo que lo hace pasar y te enseña las dos ejecuciones. Vale para todos los agentes y se aplica a los que abras a partir de ahora. En Project Guard puedes además avisar si un cambio toca código sin tests."
            ),
            |ui| {
                let mut on = general;
                if ui
                    .checkbox(&mut on, tr!("Test que falla antes del código (TDD)"))
                    .changed()
                {
                    set = Some((None, if on { "on" } else { "off" }));
                }
                if let (Some(p), Some(own)) = (project.as_deref(), own.as_deref()) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(tr!("En este proyecto")).color(theme::TEXT_3));
                        let label = |v: &str| match v {
                            "on" => tr!("Siempre"),
                            "off" => tr!("Nunca"),
                            _ => tr!("Como en general"),
                        };
                        egui::ComboBox::from_id_salt("tests-first-project")
                            .selected_text(label(own))
                            .show_ui(ui, |ui| {
                                for v in ["", "on", "off"] {
                                    if ui.selectable_label(own == v, label(v)).clicked() {
                                        set = Some((Some(p), v));
                                    }
                                }
                            });
                    });
                }
            },
        );
        if let Some((scope, value)) = set {
            if let Err(e) = store.set_launch_pref("forge", "tests_first", scope, value) {
                self.error = Some(e);
            }
            self.sync_accounts();
        }
    }

    /// Claude Code en Sonnet con Opus de asesor (plan #81): Sonnet hace el trabajo y consulta
    /// a Opus en los momentos difíciles. Apagado de serie.
    fn advisor_ui(&mut self, ui: &mut egui::Ui) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let mut on = store.advisor();
        let before = on;
        row(
            ui,
            tr!("Asesor de Claude"),
            tr!(
                "Claude Code trabaja con Sonnet, que es más barato, y consulta a Opus cuando lo necesita. Medido en sesiones reales: la mayor parte del gasto era Opus releyendo conversaciones largas. Abre Claude en Sonnet y activa el asesor Opus de Claude Code (todavía experimental, lo cobra al precio de Opus); Forge se lo recuerda al crear un plan, cuando un mismo error se repite y antes de dar un cambio por terminado. Un /model que elijas dentro de la sesión manda. Apagado de serie; se aplica a las sesiones que abras a partir de ahora."
            ),
            |ui| {
                ui.checkbox(&mut on, tr!("Sonnet con Opus de asesor"));
            },
        );
        if on == before {
            return;
        }
        let saved = store
            .set_launch_pref("forge", "advisor", None, if on { "on" } else { "off" })
            .and_then(|()| {
                let home = forge_core::home_dir().ok_or("sin carpeta personal")?;
                forge_core::agents::set_claude_advisor(
                    &forge_core::agents::claude_settings(&home),
                    on,
                )
            });
        if let Err(e) = saved {
            self.error = Some(e);
        }
        self.sync_accounts();
    }

    /// Forge Jev: el modelo más barato que sirva para cada trabajo. Cada parte aparece con su
    /// función (hoy, las tareas de git a un subagente barato).
    fn jev_ui(&mut self, ui: &mut egui::Ui) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let mut on = store.launch_pref("forge", "jev", None) == "on";
        let mut chores = store.launch_pref("forge", "jev_chores", None) != "off";
        let before = (on, chores);
        row(
            ui,
            "Forge Jev",
            tr!(
                "Manda cada trabajo al modelo más barato que lo hace bien. Medido en sesiones reales: commits, push, ramas y PR son una cuarta parte del gasto, porque cada paso relee toda la conversación con el modelo caro; un subagente barato empieza con un contexto pequeño. Apagado de serie; se aplica a los agentes que abras a partir de ahora. En Codex, Gemini, OpenCode y Qwen hace falta tener añadidos los subagentes."
            ),
            |ui| {
                ui.checkbox(&mut on, tr!("Elegir el modelo más barato que sirva"));
                if on {
                    ui.checkbox(
                        &mut chores,
                        tr!("Tareas de git a un subagente barato (forge-chores)"),
                    );
                }
            },
        );
        if (on, chores) == before {
            return;
        }
        let saved = store
            .set_launch_pref("forge", "jev", None, if on { "on" } else { "off" })
            .and_then(|()| {
                store.set_launch_pref("forge", "jev_chores", None, if chores { "" } else { "off" })
            });
        // Los subagentes ya añadidos ganan o pierden forge-chores, con el mismo modelo.
        let chores = store.jev_chores();
        let refreshed = forge_core::home_dir().map_or(Ok(()), |home| {
            use forge_core::subagents::{self, Agent};
            [Agent::Codex, Agent::Gemini, Agent::OpenCode, Agent::Qwen]
                .into_iter()
                .filter(|a| subagents::installed(*a, &home))
                .try_for_each(|a| {
                    let model = subagents::installed_model(a, &home);
                    subagents::install(a, &home, model.as_deref(), chores)
                })
        });
        if let Err(e) = saved.and(refreshed) {
            self.error = Some(e);
        }
        self.sync_accounts();
    }

    /// Los comandos de Forge en Codex (`$forge-plans`…): skills en ~/.codex/skills,
    /// solo si el usuario lo pide (es su carpeta; las demás cuentas la comparten).
    /// Búsqueda por significado: se descarga al activarla y se quita para liberar el disco.
    fn semantic_ui(&mut self, ui: &mut egui::Ui) {
        let size_mb = forge_core::embed::FILES.iter().map(|f| f.2).sum::<u64>() / 1_000_000;
        row(
            ui,
            tr!("Búsqueda por significado"),
            tr!(
                "Las pistas de memoria que Forge da a los agentes encuentran las notas por lo que dicen, no solo por sus palabras, y entre idiomas (escribes en español, la nota está en inglés). Usa un modelo local que se descarga una vez; nada sale de tu equipo. Sin él, Forge busca por palabras."
            ),
            |ui| {
                let semantic = &mut self.semantic;
                if let Some(done) = semantic.downloading() {
                    ui.add(
                        egui::ProgressBar::new(done)
                            .desired_width(160.0)
                            .text(tr!("Descargando… {p0} %", p0 = (done * 100.0) as u32)),
                    );
                } else if semantic.installed() {
                    ui.label(
                        RichText::new(tr!("✓ Activada · {p0} MB", p0 = size_mb))
                            .color(theme::TEXT_3),
                    );
                    if ui.button(tr!("Quitar el modelo")).clicked() {
                        semantic.remove();
                    }
                } else if semantic.dir.is_some()
                    && ui
                        .button(tr!("Activar (descarga {p0} MB)", p0 = size_mb))
                        .clicked()
                {
                    semantic.install(ui.ctx());
                }
            },
        );
        if let Some(e) = &self.semantic.error {
            ui.label(RichText::new(e).small().color(theme::RED));
        }
    }

    fn codex_skills_ui(&mut self, ui: &mut egui::Ui, codex: &std::path::Path) {
        let installed = forge_core::mcp::codex_skills_installed(codex);
        let mut result = None;
        row(
            ui,
            tr!("Comandos de Forge"),
            tr!(
                "Codex no muestra los comandos /forge: con esto se escriben como skills en ~/.codex/skills y se usan con $forge-plans, $forge-plan, $forge-memory, $forge-remember y $forge-guard. Quitarlos borra solo esas carpetas."
            ),
            |ui| {
                if installed {
                    if ui.button(tr!("Quitar de Codex")).clicked() {
                        result = Some(forge_core::mcp::remove_codex_skills(codex));
                    }
                } else if ui.button(tr!("Añadir a Codex")).clicked() {
                    result = Some(forge_core::mcp::install_codex_skills(codex));
                }
            },
        );
        if let Some(Err(e)) = result {
            self.error = Some(e);
        }
    }

    /// El explorador y el revisor de Forge como subagentes de Codex o Gemini, en un modelo
    /// barato; solo si el usuario lo pide (es su carpeta).
    fn subagents_ui(
        &mut self,
        ui: &mut egui::Ui,
        agent: forge_core::subagents::Agent,
        codex: Option<&std::path::Path>,
    ) {
        use forge_core::subagents;
        let Some(home) = forge_core::home_dir() else {
            return;
        };
        let installed = subagents::installed(agent, &home);
        let chores = self.store.as_ref().is_some_and(|s| s.jev_chores());
        let mut result = None;
        // Tras instalar o quitar, el campo vuelve a mostrar el modelo instalado.
        let mut reseed = false;
        let dir = match agent {
            subagents::Agent::Codex => "~/.codex/agents",
            subagents::Agent::Gemini => "~/.gemini/agents",
            subagents::Agent::OpenCode => "~/.config/opencode/agents",
            subagents::Agent::Qwen => "~/.qwen/agents",
        };
        row(
            ui,
            tr!("Subagentes más baratos"),
            &tr!(
                "Escribe forge-explorer (busca y lee código) y forge-reviewer (corre tests y builds y revisa) en {p0}, con un modelo rápido y barato. El agente principal les pasa ese trabajo y su contexto queda limpio. Quitarlos borra solo esos archivos.",
                p0 = dir
            ),
            |ui| {
                if installed {
                    // Qué modelo usan: si Codex no encontró uno barato, heredan el principal
                    // y no ahorran nada; se puede escribir uno a mano.
                    let current = subagents::installed_model(agent, &home);
                    match &current {
                        Some(m) => ui.label(tr!("Modelo: {m}", m = m)),
                        None => ui.colored_label(
                            theme::ORANGE,
                            tr!("Sin modelo barato: usan el del agente y no ahorran. Escribe uno abajo."),
                        ),
                    };
                    let draft = self
                        .subagent_model
                        .entry(dir.to_string())
                        .or_insert_with(|| current.clone().unwrap_or_default());
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(draft)
                                .hint_text(tr!("modelo"))
                                .desired_width(170.0),
                        );
                        let wanted = draft.trim().to_string();
                        if ui
                            .add_enabled(
                                !wanted.is_empty() && Some(&wanted) != current.as_ref(),
                                egui::Button::new(tr!("Usar este modelo")),
                            )
                            .clicked()
                        {
                            result = Some(subagents::install(agent, &home, Some(&wanted), chores));
                            reseed = true;
                        }
                    });
                    if ui.button(tr!("Quitar subagentes")).clicked() {
                        result = Some(subagents::remove(agent, &home));
                        reseed = true;
                    }
                } else if ui.button(tr!("Añadir subagentes")).clicked() {
                    // ponytail: el catálogo de Codex se lee al pulsar (es local y rápido).
                    let model = codex
                        .and_then(|c| {
                            std::process::Command::new(c)
                                .args(["debug", "models"])
                                .output()
                                .ok()
                        })
                        .and_then(|o| {
                            subagents::cheap_codex_model(&String::from_utf8_lossy(&o.stdout))
                        });
                    result = Some(subagents::install(agent, &home, model.as_deref(), chores));
                }
            },
        );
        if reseed || matches!(result, Some(Ok(()))) {
            self.subagent_model.remove(dir);
        }
        if let Some(Err(e)) = result {
            self.error = Some(e);
        }
    }

    /// Avisos: no molestar, volumen y, por tipo, notificación, sonido y si avisa también
    /// con Forge al frente. Se guarda al cambiar.
    fn alerts_tab(&mut self, ui: &mut egui::Ui) {
        use super::notify::{alert_text, play, sounds};
        use crate::workspace::Alert;
        let mut alerts = self.alerts.clone();
        theme::section(ui, tr!("General"));
        theme::card(ui, |ui| {
            row(
                ui,
                tr!("No molestar"),
                tr!(
                    "Sin notificaciones ni sonidos. Los puntos de la barra lateral siguen avisando."
                ),
                |ui| {
                    ui.checkbox(&mut alerts.quiet, tr!("Silenciar todos los avisos"));
                },
            );
            row(
                ui,
                tr!("Volumen"),
                tr!("Volumen de los sonidos de Forge, aparte del volumen del Mac."),
                |ui| {
                    ui.horizontal(|ui| {
                        let slider = ui.add(
                            egui::Slider::new(&mut alerts.volume, 0.0..=1.0).show_value(false),
                        );
                        // Al soltar, se oye cómo queda.
                        if slider.drag_stopped() {
                            play(&alerts.get(Alert::Done).sound, alerts.volume);
                        }
                        ui.label(format!("{:.0} %", alerts.volume * 100.0));
                    });
                },
            );
        });
        theme::section(ui, tr!("Cada aviso"));
        theme::card(ui, |ui| {
            for alert in Alert::ALL {
                let (name, help) = alert_text(alert);
                let mut prefs = alerts.get(alert);
                row(ui, name, help, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut prefs.notify, tr!("Notificación"));
                        let shown = if prefs.sound.is_empty() {
                            tr!("Sin sonido").to_string()
                        } else {
                            prefs.sound.clone()
                        };
                        egui::ComboBox::from_id_salt(("alert-sound", alert as u8))
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut prefs.sound,
                                    String::new(),
                                    tr!("Sin sonido"),
                                );
                                for s in sounds() {
                                    if ui
                                        .selectable_value(&mut prefs.sound, s.clone(), s)
                                        .clicked()
                                    {
                                        play(s, alerts.volume);
                                    }
                                }
                            });
                        let can_play = !prefs.sound.is_empty();
                        if ui
                            .add_enabled(can_play, egui::Button::new(icon::PLAY))
                            .on_hover_text(tr!("Escuchar"))
                            .clicked()
                        {
                            play(&prefs.sound, alerts.volume);
                        }
                        ui.checkbox(&mut prefs.foreground, tr!("También con Forge al frente"));
                    });
                });
                if prefs != alerts.get(alert) {
                    alerts.kinds.insert(alert, prefs);
                }
            }
        });
        hint(
            ui,
            tr!(
                "Las notificaciones de macOS se pueden ajustar también en Ajustes del Sistema → Notificaciones → Forge."
            ),
        );
        if alerts != self.alerts {
            if let Err(e) = alerts.save(self.store.as_ref()) {
                self.error = Some(e);
            }
            self.alerts = alerts;
        }
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
        let mut submit = None;
        let mut new_key: Option<(String, String)> = None;
        let detected = self.agents.lock().map(|d| d.clone()).unwrap_or_default();
        let programs: Vec<(&str, &str)> = [
            ("claude", "Claude Code"),
            ("codex", "Codex"),
            ("gemini", "Gemini CLI"),
        ]
        .into_iter()
        .filter(|(p, _)| detected.get(*p).is_some_and(|d| d.path.is_some()))
        .collect();
        if programs.is_empty() {
            return;
        }
        let in_project = self.active.is_some();
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
                let chosen = self
                    .active
                    .and_then(|i| self.workspaces[i].account_for(program, None))
                    .map(|a| a.id);
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
                    account_row(ui, cmds, program, a, chosen, in_project, &mut new_key);
                    // Una cuenta con clave de API no tiene plan ni ventanas de uso.
                    if !a.is_api() {
                        account_usage(ui, limits.as_ref(), tokens);
                    }
                    // La ficha de la cuenta (modelo y esfuerzo) solo se ve al abrirla.
                    if ui
                        .data(|d| d.get_temp::<bool>(config_id(program, a.id)))
                        .unwrap_or(false)
                    {
                        ui.add_space(4.0);
                        if program == "gemini" {
                            // Forge no aplica modelo ni esfuerzo de arranque a Gemini CLI.
                            hint(
                                ui,
                                tr!(
                                    "Gemini CLI se abre con su propia configuración: aquí solo está la clave de la cuenta."
                                ),
                            );
                        } else {
                            self.launch_ui(ui, program, Some(a));
                        }
                    }
                    ui.add_space(4.0);
                }
                ui.add_space(4.0);
                // Gemini CLI solo tiene cuentas con clave de API (sin carpeta propia).
                if program != "gemini"
                    && ui
                        .button(tr!("{p0}  Añadir cuenta", p0 = icon::PLUS))
                        .clicked()
                {
                    cmds.push(UiCmd::AccountAdd(program.into()));
                }
                // Con una clave de API en vez de iniciar sesión: Claude Code (Anthropic,
                // DeepSeek, Kimi u otro) o Gemini CLI.
                if program == "claude" || program == "gemini" {
                    let label = if program == "gemini" {
                        tr!("Añadir clave de Gemini")
                    } else {
                        tr!("Añadir modelo por API")
                    };
                    match self.api_form.as_mut() {
                        Some(form) if form.program == program => {
                            submit = api_form_ui(ui, form);
                        }
                        // El formulario abierto es de otro agente.
                        Some(_) => {}
                        None => {
                            if ui.button(label).clicked() {
                                self.api_form = Some(ApiForm::new(if program == "gemini" {
                                    "gemini"
                                } else {
                                    "claude"
                                }));
                            }
                        }
                    }
                }
            }
        });
        if let Some((account, key)) = new_key
            && let Err(e) = self.secrets.set(&account, key.trim())
        {
            self.error = Some(e);
        }
        match submit {
            Some(true) => self.add_api_account(),
            Some(false) => self.api_form = None,
            None => {}
        }
    }

    /// Guarda la cuenta del formulario (la clave va al llavero). Si algo falla, el
    /// formulario sigue abierto con el motivo.
    fn add_api_account(&mut self) {
        let Some(mut form) = self.api_form.take() else {
            return;
        };
        let name = match form.name.trim() {
            "" => provider_label(form.provider),
            name => name.to_string(),
        };
        let custom = form.provider == "custom";
        let result = self.store.as_ref().map(|s| {
            s.add_api_account(
                form.program,
                &name,
                form.provider,
                custom.then_some(form.url.as_str()),
                (!form.model.trim().is_empty()).then_some(form.model.as_str()),
                &form.key,
                self.secrets.as_ref(),
            )
        });
        match result {
            Some(Ok(_)) => self.sync_accounts(),
            Some(Err(e)) => {
                form.error = Some(e);
                self.api_form = Some(form);
            }
            None => self.api_form = Some(form),
        }
    }
}

/// Lo que devuelve la prueba de la clave: su veredicto y los modelos que la API ofrece.
type KeyCheck = (Result<(), String>, Vec<String>);

/// «Añadir modelo por API»: lo que se escribe mientras el formulario está abierto.
pub(super) struct ApiForm {
    /// El agente de la cuenta: `claude` o `gemini`.
    program: &'static str,
    provider: &'static str,
    name: String,
    key: String,
    url: String,
    model: String,
    error: Option<String>,
    /// Prueba de la clave en curso (en otro hilo) y su resultado.
    check: Option<std::sync::mpsc::Receiver<KeyCheck>>,
    checked: Option<Result<(), String>>,
    /// Los modelos que la API ofreció al probar la clave (vacío si no hay o no se pidieron).
    models: Vec<String>,
}

impl ApiForm {
    fn new(program: &'static str) -> Self {
        Self {
            program,
            provider: if program == "gemini" {
                "gemini"
            } else {
                "anthropic"
            },
            name: String::new(),
            key: String::new(),
            url: String::new(),
            model: String::new(),
            error: None,
            check: None,
            checked: None,
            models: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn provider(&self) -> &str {
        self.provider
    }

    /// Un formulario ya rellenado (los campos de texto no se teclean en las pruebas).
    #[cfg(test)]
    pub(super) fn filled(
        program: &'static str,
        provider: &'static str,
        key: &str,
        url: &str,
        model: &str,
    ) -> Self {
        Self {
            program,
            provider,
            key: key.into(),
            url: url.into(),
            model: model.into(),
            ..Self::new(program)
        }
    }

    #[cfg(test)]
    pub(super) fn models(&self) -> &[String] {
        &self.models
    }

    /// Qué falta para poder añadir; la clave nunca se muestra después.
    fn ready(&self) -> bool {
        !self.key.trim().is_empty()
            && (self.provider != "custom"
                || (!self.url.trim().is_empty() && !self.model.trim().is_empty()))
    }
}

fn provider_label(provider: &str) -> String {
    match provider {
        "deepseek" => "DeepSeek".into(),
        "kimi" => "Kimi".into(),
        "gemini" => "Gemini".into(),
        "custom" => tr!("Otro").to_string(),
        _ => "Claude (Anthropic)".into(),
    }
}

/// El formulario en tres pasos cortos. Devuelve `Some(true)` al pulsar «Añadir» y
/// `Some(false)` al cancelar.
fn api_form_ui(ui: &mut egui::Ui, form: &mut ApiForm) -> Option<bool> {
    let mut action = None;
    ui.add_space(8.0);
    theme::card(ui, |ui| {
        ui.label(RichText::new(tr!("Añadir modelo por API")).strong());
        ui.add_space(4.0);
        // Un desplegable: la lista de proveedores crecerá y así no se desborda. Gemini CLI
        // solo tiene el suyo.
        let mut chosen = form.provider;
        if form.program == "gemini" {
            ui.label(RichText::new(provider_label("gemini")).strong());
        } else {
            egui::ComboBox::from_id_salt("api-provider")
                .selected_text(provider_label(chosen))
                .width(220.0)
                .show_ui(ui, |ui| {
                    for provider in ["anthropic", "deepseek", "kimi", "custom"] {
                        ui.selectable_value(&mut chosen, provider, provider_label(provider));
                    }
                });
        }
        if chosen != form.provider {
            form.provider = chosen;
            form.error = None;
            form.checked = None;
            // El modelo y la lista eran de otro proveedor.
            form.models.clear();
            form.model.clear();
        }
        ui.label(
            RichText::new(match form.provider {
                "deepseek" => tr!("Usa los modelos de DeepSeek en Claude Code."),
                "kimi" => tr!("Usa los modelos de Kimi (Moonshot) en Claude Code."),
                "gemini" => tr!("Con tu clave de Google AI Studio, para Gemini CLI."),
                "custom" => {
                    tr!("Para cualquier servicio compatible con la API de Anthropic.")
                }
                _ => tr!("Con tu clave de la consola de Anthropic."),
            })
            .size(11.5)
            .color(theme::TEXT_3),
        );
        ui.add_space(4.0);
        ui.add(
            theme::field(egui::TextEdit::singleline(&mut form.name))
                .hint_text(tr!("Nombre (opcional)"))
                .desired_width(f32::INFINITY),
        );
        let mut edited = ui
            .add(
                theme::field(egui::TextEdit::singleline(&mut form.key).password(true))
                    .hint_text(tr!("Pega aquí tu clave"))
                    .desired_width(f32::INFINITY),
            )
            .changed();
        // Otra clave o dirección: la lista de modelos ya no vale.
        let mut connection_changed = edited;
        if form.provider == "custom" {
            let url_changed = ui
                .add(
                    theme::field(egui::TextEdit::singleline(&mut form.url))
                        .hint_text(tr!("Dirección (https://…)"))
                        .desired_width(f32::INFINITY),
                )
                .changed();
            edited |= url_changed;
            connection_changed |= url_changed;
            edited |= ui
                .add(
                    theme::field(egui::TextEdit::singleline(&mut form.model))
                        .hint_text(tr!("Modelo"))
                        .desired_width(f32::INFINITY),
                )
                .changed();
        }
        if edited {
            form.checked = None;
        }
        if connection_changed {
            form.models.clear();
        }
        // La prueba corre aparte: la ventana no se para esperando al servicio.
        if let Some(rx) = &form.check {
            match rx.try_recv() {
                Ok((result, models)) => {
                    form.checked = Some(result);
                    form.models = models;
                    form.check = None;
                }
                Err(_) => ui.ctx().request_repaint_after(Duration::from_millis(100)),
            }
        }
        ui.label(
            RichText::new(tr!(
                "La clave se guarda cifrada en el llavero de tu sistema y no se vuelve a mostrar."
            ))
            .size(11.5)
            .color(theme::TEXT_3),
        );
        if form.check.is_some() {
            ui.label(
                RichText::new(tr!("Probando…"))
                    .size(12.0)
                    .color(theme::TEXT_3),
            );
        }
        match &form.checked {
            Some(Ok(())) => {
                ui.label(
                    RichText::new(tr!("✓ La clave funciona."))
                        .size(12.0)
                        .color(theme::GREEN),
                );
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).size(12.0).color(theme::RED));
            }
            None => {}
        }
        // Los modelos que la propia API ofrece, para elegir sin escribir.
        if !form.models.is_empty() {
            ui.label(
                RichText::new(tr!("Elige el modelo de la lista"))
                    .size(11.5)
                    .color(theme::TEXT_3),
            );
            let shown = if form.model.is_empty() {
                tr!("Por defecto").to_string()
            } else {
                form.model.clone()
            };
            egui::ComboBox::from_id_salt("api-model")
                .selected_text(shown)
                .width(320.0)
                .show_ui(ui, |ui| {
                    // Una dirección propia no tiene modelo por defecto: lo exige.
                    if form.provider != "custom" {
                        ui.selectable_value(&mut form.model, String::new(), tr!("Por defecto"));
                    }
                    for id in &form.models {
                        ui.selectable_value(&mut form.model, id.clone(), id);
                    }
                });
        }
        if let Some(e) = &form.error {
            ui.label(RichText::new(e).size(12.0).color(theme::RED));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(form.ready(), theme::primary_button(tr!("Añadir modelo")))
                .clicked()
            {
                action = Some(true);
            }
            let can_test = form.ready() && form.check.is_none();
            if ui
                .add_enabled(can_test, egui::Button::new(tr!("Probar clave")))
                .clicked()
            {
                let (tx, rx) = std::sync::mpsc::channel();
                let (provider, key) = (form.provider, form.key.clone());
                let custom = provider == "custom";
                let (url, model) = (form.url.clone(), form.model.clone());
                std::thread::spawn(move || {
                    let address = custom.then_some(url.as_str());
                    let result = forge_core::apicheck::check(
                        provider,
                        address,
                        (!model.trim().is_empty()).then_some(model.as_str()),
                        &key,
                    );
                    // Si la clave vale, se pide la lista; si no hay lista, se escribe a mano.
                    let models = if result.is_ok() {
                        forge_core::apicheck::list_models(provider, address, &key)
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let _ = tx.send((result, models));
                });
                form.check = Some(rx);
                form.checked = None;
            }
            if theme::secondary(ui, tr!("Cancelar")).clicked() {
                action = Some(false);
            }
        });
    });
    action
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
    new_key: &mut Option<(String, String)>,
) {
    let id = egui::Id::new(("account-name", program, a.id));
    let saved = egui::Id::new(("account-saved", program, a.id));
    let mut text = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| a.name.clone());
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::PENCIL_SIMPLE).color(theme::TEXT_3))
            .on_hover_text(tr!("Escribe para cambiar el nombre"));
        // Deja sitio a «Configurar» y a la papelera; el campo ocupa el resto (nunca menos
        // de 120).
        let width =
            (ui.available_width() - if a.is_main() { 180.0 } else { 140.0 }).clamp(120.0, 320.0);
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
                // La hora antes: `ui.input` dentro de `data_mut` vuelve a pedir el cerrojo
                // del contexto y Forge se queda esperándose a sí mismo.
                let now = ui.input(|i| i.time);
                ui.data_mut(|d| d.insert_temp(saved, now));
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !a.is_main() {
                let confirm = egui::Id::new(("account-delete", program, a.id));
                if !ui.data(|d| d.get_temp::<bool>(confirm)).unwrap_or(false)
                    && theme::icon_button(ui, icon::TRASH, tr!("Borrar cuenta")).clicked()
                {
                    ui.data_mut(|d| d.insert_temp(confirm, true));
                }
            }
            let open = ui
                .data(|d| d.get_temp::<bool>(config_id(program, a.id)))
                .unwrap_or(false);
            if ui
                .button(if open {
                    tr!("Cerrar")
                } else {
                    tr!("Configurar")
                })
                .on_hover_text(tr!("Modelo, esfuerzo y clave de esta cuenta"))
                .clicked()
            {
                ui.data_mut(|d| d.insert_temp(config_id(program, a.id), !open));
            }
        });
    });
    // Confirmación de borrado, en su propia línea.
    let confirm = egui::Id::new(("account-delete", program, a.id));
    if ui.data(|d| d.get_temp::<bool>(confirm)).unwrap_or(false) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(if a.program == "gemini" {
                    tr!("Se borra su clave de API.")
                } else {
                    tr!("Se borran su inicio de sesión y su historial.")
                })
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
    if let Some(provider) = &a.provider {
        let open = egui::Id::new(("account-key-open", program, a.id));
        let typed = egui::Id::new(("account-key-text", program, a.id));
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tr!("Clave de API · {p0}", p0 = provider_label(provider)))
                    .size(11.5)
                    .color(theme::TEXT_3),
            );
            let sheet = ui
                .data(|d| d.get_temp::<bool>(config_id(program, a.id)))
                .unwrap_or(false);
            if sheet
                && !ui.data(|d| d.get_temp::<bool>(open)).unwrap_or(false)
                && ui.small_button(tr!("Cambiar clave")).clicked()
            {
                ui.data_mut(|d| d.insert_temp(open, true));
            }
        });
        if ui.data(|d| d.get_temp::<bool>(open)).unwrap_or(false) {
            let mut text = ui.data(|d| d.get_temp::<String>(typed)).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.add(
                    theme::field(egui::TextEdit::singleline(&mut text).password(true))
                        .hint_text(tr!("Pega aquí tu clave nueva"))
                        .desired_width(240.0),
                );
                let save = ui
                    .add_enabled(
                        !text.trim().is_empty(),
                        egui::Button::new(tr!("Guardar clave")),
                    )
                    .clicked();
                let cancel = ui.button(tr!("Cancelar")).clicked();
                if save {
                    *new_key = Some((a.secret_id(), std::mem::take(&mut text)));
                }
                if save || cancel {
                    ui.data_mut(|d| {
                        d.remove::<bool>(open);
                        d.remove::<String>(typed);
                    });
                } else {
                    ui.data_mut(|d| d.insert_temp(typed, text.clone()));
                }
            });
        }
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
            if !a.is_api()
                && ui
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

/// Qué hace cada nivel de esfuerzo.
fn effort_info(level: &str) -> &'static str {
    match level {
        "" => tr!("El que tenga el agente en su propia configuración. Forge no lo cambia."),
        "low" => tr!("Piensa poco: rápido y barato, para tareas sencillas."),
        "medium" => tr!("Equilibrio entre rapidez y cuidado."),
        "high" => tr!("Piensa más: para cambios que tocan varias piezas."),
        "xhigh" => tr!("Para problemas difíciles. Más lento y gasta más."),
        "max" => tr!("Lo máximo: el más lento y caro. Solo para lo más difícil."),
        _ => "",
    }
}

fn effort_label(level: &str) -> &'static str {
    match level {
        "" => tr!("Tu configuración"),
        "low" => tr!("Bajo"),
        "medium" => tr!("Medio"),
        "high" => tr!("Alto"),
        "xhigh" => tr!("Muy alto"),
        "max" => tr!("Máximo"),
        _ => "",
    }
}

impl App {
    /// Modelo y esfuerzo de un agente: en general y, si hay un proyecto abierto, solo para
    /// él. Se aplican a los agentes que se abran después.
    fn launch_ui(
        &mut self,
        ui: &mut egui::Ui,
        program: &str,
        account: Option<&forge_core::accounts::Account>,
    ) {
        let project = self.active.map(|i| self.workspaces[i].project.path.clone());
        let Some(store) = self.store.as_ref() else {
            return;
        };
        // Lo que tiene guardado como suyo: una cuenta, lo de la cuenta; un agente sin cuentas,
        // lo de siempre.
        let own = |what: &str, scope: Option<&std::path::Path>| -> Option<String> {
            match account {
                Some(a) => store.account_launch_own(a, what, scope),
                None => Some(store.launch_pref(program, what, scope)),
            }
        };
        // Una cuenta que nunca se configuró sigue usando lo de antes (lo general): se dice.
        let before = |what: &str, scope: Option<&std::path::Path>| -> String {
            if account.is_some() && scope.is_none() && own(what, None).is_none() {
                store.launch_pref(program, what, None)
            } else {
                String::new()
            }
        };
        // El modelo de una cuenta con API ya lo fija su proveedor.
        let model_fixed = account.is_some_and(|a| a.fixes_model());
        // Lo que significa «Tu configuración» en Codex: lo de su config.toml.
        let codex = (program == "codex")
            .then(|| {
                forge_core::home_dir()
                    .map(|h| forge_core::launch::codex_defaults(&h.join(".codex")))
            })
            .flatten()
            .unwrap_or_default();
        let mine = |what: &str| -> String {
            match (program, what) {
                ("codex", "model") if !codex.0.is_empty() => {
                    tr!("Tu configuración ({p0})", p0 = codex.0)
                }
                ("codex", "effort") if !codex.1.is_empty() => {
                    tr!("Tu configuración ({p0})", p0 = effort_label(&codex.1))
                }
                _ => tr!("Tu configuración").to_string(),
            }
        };
        let mut changes: Vec<(&str, Option<std::path::PathBuf>, String)> = Vec::new();
        let scopes: Vec<(Option<std::path::PathBuf>, &str)> =
            std::iter::once((None, tr!("En general")))
                .chain(project.clone().map(|p| (Some(p), tr!("En este proyecto"))))
                .collect();
        for (scope, scope_label) in &scopes {
            let model = own("model", scope.as_deref()).unwrap_or_default();
            let effort = own("effort", scope.as_deref()).unwrap_or_default();
            let (before_model, before_effort) = (
                before("model", scope.as_deref()),
                before("effort", scope.as_deref()),
            );
            let inherit = tr!("Como en general");
            let empty = if scope.is_some() {
                inherit.to_string()
            } else if !before_model.is_empty() {
                tr!("Como antes ({p0})", p0 = before_model)
            } else {
                mine("model")
            };
            let empty_effort = if scope.is_some() {
                inherit.to_string()
            } else if !before_effort.is_empty() {
                tr!("Como antes ({p0})", p0 = effort_label(&before_effort))
            } else {
                mine("effort")
            };
            let salt = (program, account.map_or(-1, |a| a.id), scope.is_some());
            if model_fixed {
                if scope.is_none() {
                    hint(
                        ui,
                        tr!("El modelo de esta cuenta lo fija su proveedor: no se elige aquí."),
                    );
                }
            } else {
                row(
                    ui,
                    &format!("{} · {scope_label}", tr!("Modelo")),
                    if program == "claude" {
                        tr!(
                            "Con qué modelo se abre Claude Code desde Forge (barra lateral, ⌘⇧A, ⌘K). Un claude escrito a mano en la terminal usa lo que escribas. «En este proyecto» manda sobre «En general»."
                        )
                    } else if program == "agy" {
                        tr!(
                            "Con qué modelo se abre Antigravity desde Forge. Escribe el nombre tal como lo pide agy (p. ej. gemini-3.8-flash-high). «En este proyecto» manda sobre «En general»."
                        )
                    } else {
                        tr!(
                            "Con qué modelo se abre Codex desde Forge. Escribe el nombre tal como lo pide Codex (p. ej. gpt-5). «En este proyecto» manda sobre «En general»."
                        )
                    },
                    |ui| {
                        if program == "claude" {
                            let name = |v: &str| {
                                if v.is_empty() {
                                    empty.clone()
                                } else {
                                    crate::claude_plugin::MODELS
                                        .iter()
                                        .find(|(id, _)| *id == v)
                                        .map_or(v.to_string(), |(_, l)| {
                                            forge_core::i18n::t(l).to_string()
                                        })
                                }
                            };
                            egui::ComboBox::from_id_salt(("model", salt))
                                .selected_text(name(&model))
                                .width(ui.available_width().min(320.0))
                                .show_ui(ui, |ui| {
                                    for (id, _) in crate::claude_plugin::MODELS {
                                        if ui
                                            .selectable_label(model == id, name(id))
                                            .on_hover_text(model_info(id))
                                            .clicked()
                                        {
                                            changes.push(("model", scope.clone(), id.to_string()));
                                        }
                                    }
                                });
                            if !model.is_empty() || scope.is_none() {
                                hint(ui, model_info(&model));
                            }
                        } else {
                            let id = egui::Id::new((
                                "codex-model",
                                account.map_or(-1, |a| a.id),
                                scope.is_some(),
                            ));
                            let mut text = ui
                                .data(|d| d.get_temp::<String>(id))
                                .unwrap_or_else(|| model.clone());
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut text)
                                    .hint_text(empty.clone())
                                    .desired_width(ui.available_width().min(320.0)),
                            );
                            if edit.changed() {
                                ui.data_mut(|d| d.insert_temp(id, text.clone()));
                            }
                            if edit.lost_focus() {
                                if text.trim() != model {
                                    changes.push(("model", scope.clone(), text.trim().to_string()));
                                }
                                ui.data_mut(|d| d.remove::<String>(id));
                            }
                        }
                    },
                );
            }
            row(
                ui,
                &format!("{} · {scope_label}", tr!("Esfuerzo")),
                tr!(
                    "Cuánto piensa el agente antes de responder. Más esfuerzo: mejores respuestas en lo difícil, pero más lento y más gasto de tu plan. En Claude Code puedes cambiarlo en la sesión con /forge-effort."
                ),
                |ui| {
                    let name = |v: &str| {
                        if v.is_empty() {
                            empty_effort.clone()
                        } else {
                            effort_label(v).to_string()
                        }
                    };
                    egui::ComboBox::from_id_salt(("effort", salt))
                        .selected_text(name(&effort))
                        .width(ui.available_width().min(320.0))
                        .show_ui(ui, |ui| {
                            for level in std::iter::once("")
                                .chain(forge_core::launch::efforts(program).iter().copied())
                            {
                                if ui
                                    .selectable_label(effort == level, name(level))
                                    .on_hover_text(effort_info(level))
                                    .clicked()
                                {
                                    changes.push(("effort", scope.clone(), level.to_string()));
                                }
                            }
                        });
                    if !effort.is_empty() {
                        hint(ui, effort_info(&effort));
                    }
                },
            );
        }
        hint(
            ui,
            tr!("Se aplica a los agentes que abras a partir de ahora."),
        );
        if !changes.is_empty() {
            for (what, scope, value) in changes {
                self.db(|s| match account {
                    Some(a) => s.set_account_launch_pref(a, what, scope.as_deref(), &value),
                    None => s.set_launch_pref(program, what, scope.as_deref(), &value),
                });
            }
            self.sync_accounts();
        }
    }
}
