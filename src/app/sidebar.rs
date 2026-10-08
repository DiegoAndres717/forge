// Barra lateral: proyectos, Guard, memoria, agentes y procesos.
use super::*;
use forge_core::tr;

/// Con más proyectos que esto, la lista se compacta.
const COMPACT_AFTER: usize = 6;

impl App {
    /// Proyectos a mostrar: todos, o (con muchos) el activo, los que tienen aviso y los
    /// usados más recientemente hasta completar `COMPACT_AFTER`.
    fn visible_workspaces(&self, attention: &[Option<crate::workspace::Attention>]) -> Vec<usize> {
        let n = self.workspaces.len();
        if n <= COMPACT_AFTER || self.show_all {
            return (0..n).collect();
        }
        let by_path = |p: &PathBuf| self.workspaces.iter().position(|w| &w.project.path == p);
        let mut out: Vec<usize> = self.active.into_iter().collect();
        out.extend((0..n).filter(|i| attention[*i].is_some()));
        out.extend(self.mru.iter().filter_map(by_path));
        out.extend(0..n);
        let mut seen = Vec::new();
        for i in out {
            let important = Some(i) == self.active || attention[i].is_some();
            if !seen.contains(&i) && (important || seen.len() < COMPACT_AFTER) {
                seen.push(i);
            }
        }
        seen.sort_unstable(); // en el orden de siempre (⌘1…⌘9)
        seen
    }

    /// Barra lateral (estilo Finder/Xcode): workspaces arriba y, del proyecto activo,
    /// agentes, procesos, comandos y accesos a Guard, Memoria y configuración.
    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
        let detected = self.agents.lock().unwrap().clone();
        let detecting = self.detecting.load(Ordering::Relaxed);
        let body = Rect::from_min_max(
            rect.min + Vec2::new(10.0, 8.0),
            rect.max - Vec2::new(10.0, 8.0),
        );
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
        ui.spacing_mut().item_spacing.y = 1.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                theme::section(ui, "Workspaces");
                if theme::row(
                    ui,
                    icon::HOUSE,
                    theme::TEXT_2,
                    tr!("Inicio"),
                    "⌘⇧H",
                    self.active.is_none(),
                )
                .clicked()
                {
                    cmds.push(UiCmd::Home);
                }
                let attention: Vec<_> = self.workspaces.iter().map(|w| w.attention()).collect();
                let visible = self.visible_workspaces(&attention);
                let ram = self.ram.lock().map(|r| r.clone()).unwrap_or_default();
                for (i, ws) in self.workspaces.iter().enumerate() {
                    if !visible.contains(&i) {
                        continue;
                    }
                    let selected = self.active == Some(i);
                    let dormant = ws.is_dormant();
                    let running = ws.processes.active_count();
                    let detail = match (running, i) {
                        _ if attention[i].is_some() => String::new(),
                        _ if dormant => tr!("dormido").to_string(),
                        (n, _) if n > 0 => tr!("{n} en marcha", n = n),
                        (_, i) if i < 9 => format!("⌘{}", i + 1),
                        _ => String::new(),
                    };
                    let (glyph, color) = match (selected, dormant) {
                        (true, _) => (icon::FOLDER, theme::ACCENT),
                        (_, true) => (icon::MOON, theme::TEXT_4),
                        _ => (icon::FOLDER, theme::TEXT_2),
                    };
                    let mut tip = tilde(&ws.project.path);
                    if let Some(bytes) = ram.get(&ws.project.path) {
                        tip += &format!("\nMemoria: {}", human_bytes(*bytes));
                    }
                    if dormant {
                        tip += tr!("\nDormido: sus terminales y procesos arrancan al entrar.");
                    }
                    let row = theme::row(ui, glyph, color, &ws.project.name(), &detail, selected);
                    // Aviso: punto de color a la derecha.
                    if let Some(a) = &attention[i] {
                        use crate::workspace::Attention;
                        let (color, text) = match a {
                            Attention::Agent(name) => (
                                theme::ORANGE,
                                tr!("{name} terminó o espera respuesta", name = name),
                            ),
                            Attention::Failed(name) => {
                                (theme::RED, tr!("El proceso {name} falló", name = name))
                            }
                            Attention::Blocked => {
                                (theme::RED, tr!("Guard bloqueó la validación").to_string())
                            }
                            Attention::Event(text) => (theme::ORANGE, text.clone()),
                        };
                        ui.painter().circle_filled(
                            egui::pos2(row.rect.max.x - 14.0, row.rect.center().y),
                            4.0,
                            color,
                        );
                        tip = format!("{text}\n{tip}");
                    }
                    let row = row.on_hover_text(tip);
                    if row.clicked() {
                        cmds.push(UiCmd::Activate(i));
                    }
                    row.context_menu(|ui| {
                        if dormant {
                            if ui.button(tr!("{p0}  Despertar", p0 = icon::SUN)).clicked() {
                                cmds.push(UiCmd::Activate(i));
                            }
                        } else if ui
                            .button(tr!("{p0}  Dormir (libera memoria)", p0 = icon::MOON))
                            .clicked()
                        {
                            cmds.push(UiCmd::Sleep(i));
                        }
                        if ui
                            .button(tr!("{p0}  Cerrar proyecto", p0 = icon::X))
                            .clicked()
                        {
                            cmds.push(UiCmd::Close(i));
                        }
                    });
                }
                let hidden = self.workspaces.len() - visible.len();
                if hidden > 0 || self.show_all {
                    let label = if self.show_all {
                        tr!("Mostrar menos").to_string()
                    } else {
                        tr!("{hidden} más…", hidden = hidden)
                    };
                    if theme::row(ui, icon::DOTS_THREE, theme::TEXT_3, &label, "⌘K", false)
                        .on_hover_text(tr!("Todos los proyectos abiertos (o búscalos con ⌘K)"))
                        .clicked()
                    {
                        self.show_all = !self.show_all;
                    }
                }
                if theme::row(
                    ui,
                    icon::PLUS,
                    theme::TEXT_3,
                    tr!("Abrir carpeta…"),
                    "⌘O",
                    false,
                )
                .clicked()
                {
                    cmds.push(UiCmd::OpenFolder);
                }

                let Some(i) = self.active else { return };
                let ws = &mut self.workspaces[i];
                ui.add_space(12.0);
                Self::agents_ui(ws, &detected, detecting, ui, cmds);
                ui.add_space(12.0);
                Self::processes_ui(ws, ui, cmds);

                if !ws.project.config.commands.is_empty() {
                    ui.add_space(12.0);
                    theme::section(ui, tr!("Comandos"));
                    for command in &ws.project.config.commands {
                        let row =
                            theme::row(ui, icon::PLAY, theme::TEXT_3, &command.name, "", false)
                                .on_hover_text(&command.command);
                        if row.clicked() {
                            cmds.push(UiCmd::RunCommand(command.clone()));
                        }
                    }
                }

                ui.add_space(12.0);
                theme::section(ui, tr!("Proyecto"));
                let (guard_text, guard_color) = match &ws.guard.run {
                    None => (tr!("sin ejecutar").to_string(), theme::TEXT_3),
                    Some(run) => run.snapshot(|r| verdict_short(r.verdict())),
                };
                if theme::row(
                    ui,
                    icon::SHIELD_CHECK,
                    guard_color,
                    "Guard",
                    &guard_text,
                    ws.guard.open,
                )
                .on_hover_text("⌘G")
                .clicked()
                {
                    cmds.push(UiCmd::ToggleGuard);
                }
                let notes = if ws.memory.count > 0 {
                    ws.memory.count.to_string()
                } else {
                    String::new()
                };
                let open = if ws.ideas.open_count > 0 {
                    ws.ideas.open_count.to_string()
                } else {
                    String::new()
                };
                if theme::row(
                    ui,
                    icon::LIGHTBULB,
                    theme::YELLOW,
                    tr!("Ideas"),
                    &open,
                    ws.ideas.open,
                )
                .on_hover_text("⌘⇧I")
                .clicked()
                {
                    cmds.push(UiCmd::ToggleIdeas);
                }
                if theme::row(
                    ui,
                    icon::BRAIN,
                    theme::PURPLE,
                    tr!("Memoria"),
                    &notes,
                    ws.memory.open,
                )
                .on_hover_text("⌘⇧M")
                .clicked()
                {
                    cmds.push(UiCmd::ToggleMemory);
                }
                let config = theme::row(
                    ui,
                    icon::GEAR,
                    theme::TEXT_3,
                    tr!("Configuración"),
                    "",
                    false,
                );
                egui::Popup::menu(&config).show(|ui| {
                    ui.set_min_width(220.0);
                    if !ws.project.has_config()
                        && ui.button(tr!("Crear .forge/project.toml")).clicked()
                    {
                        cmds.push(UiCmd::CreateConfig);
                    }
                    if ws.project.has_config() {
                        if ui
                            .button(tr!("{p0}  Editar project.toml", p0 = icon::NOTE_PENCIL))
                            .clicked()
                        {
                            cmds.push(UiCmd::EditConfig);
                        }
                        if ui
                            .button(tr!(
                                "{p0}  Recargar configuración",
                                p0 = icon::ARROW_CLOCKWISE
                            ))
                            .clicked()
                        {
                            cmds.push(UiCmd::ReloadConfig);
                        }
                    }
                    if ws.project.default_layout().is_some()
                        && ui.button(tr!("Restablecer layout")).clicked()
                    {
                        cmds.push(UiCmd::ResetLayout);
                    }
                });
            });
    }

    /// Sección AGENTES: instalados (con versión) y configurados; clic abre, clic derecho reanuda.
    pub(super) fn agents_ui(
        ws: &Workspace,
        detected: &HashMap<String, agents::Detection>,
        detecting: bool,
        ui: &mut egui::Ui,
        cmds: &mut Vec<UiCmd>,
    ) {
        ui.horizontal(|ui| {
            theme::section(ui, tr!("Agentes"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if detecting {
                    ui.spinner();
                } else if theme::icon_button_sized(
                    ui,
                    icon::ARROW_CLOCKWISE,
                    tr!("Volver a detectar agentes"),
                    20.0,
                    theme::TEXT_4,
                )
                .clicked()
                {
                    cmds.push(UiCmd::DetectAgents);
                }
            });
        });
        let default = agents::default_agent(&ws.project.agents, detected).map(|a| a.id.clone());
        let mut missing = Vec::new();
        for agent in ws.project.agents.iter().filter(|a| a.enabled) {
            let found = detected.get(agent.program());
            let installed = found.is_some_and(|d| d.path.is_some());
            if !installed && !agent.configured {
                if found.is_some() {
                    missing.push(agent);
                }
                continue;
            }
            let star = if default.as_deref() == Some(agent.id.as_str()) {
                "  ★"
            } else {
                ""
            };
            let version = found
                .and_then(|d| d.version.as_deref())
                .map(short_version)
                .unwrap_or_default();
            let color = if installed {
                theme::ORANGE
            } else {
                theme::TEXT_4
            };
            let tip = match found.and_then(|d| d.path.as_ref()) {
                Some(path) => tr!(
                    "{p0}\n{p1}\nClic: abrir · clic derecho: reanudar y más",
                    p0 = agent.command,
                    p1 = path.display()
                ),
                None => tr!("{p0} no está instalado", p0 = agent.program()),
            };
            let has_logo = matches!(agent.program(), "claude" | "codex" | "opencode");
            let glyph = if has_logo { "" } else { icon::ROBOT };
            let row = theme::row(
                ui,
                glyph,
                color,
                &format!("{}{star}", agent.name),
                &version,
                false,
            );
            if has_logo {
                let at = Rect::from_center_size(
                    egui::pos2(row.rect.min.x + 17.0, row.rect.center().y),
                    Vec2::splat(16.0),
                );
                theme::agent_logo(ui, agent.program(), at, !installed);
            }
            let row = row.on_hover_text(tip);
            if row.clicked() && installed {
                cmds.push(UiCmd::OpenAgent(agent.id.clone(), false));
            }
            row.context_menu(|ui| {
                ui.set_min_width(240.0);
                if ui
                    .add_enabled(
                        installed,
                        egui::Button::new(tr!("{p0}  Abrir", p0 = icon::TERMINAL_WINDOW)),
                    )
                    .clicked()
                {
                    cmds.push(UiCmd::OpenAgent(agent.id.clone(), false));
                }
                if let Some(resume) = &agent.resume {
                    let button = egui::Button::new(tr!(
                        "{p0}  Reanudar última sesión",
                        p0 = icon::CLOCK_COUNTER_CLOCKWISE
                    ));
                    if ui
                        .add_enabled(installed, button)
                        .on_hover_text(resume)
                        .clicked()
                    {
                        cmds.push(UiCmd::OpenAgent(agent.id.clone(), true));
                    }
                }
                if ui
                    .button(tr!("{p0}  Copiar comando", p0 = icon::COPY))
                    .clicked()
                {
                    ui.ctx().copy_text(agent.command.clone());
                }
                ui.separator();
                for cap in agent.capabilities() {
                    ui.label(
                        RichText::new(format!("• {cap}"))
                            .small()
                            .color(theme::TEXT_3),
                    );
                }
                if !installed && let Some(hint) = &agent.install_hint {
                    ui.label(
                        RichText::new(tr!("Instalar: {hint}", hint = hint))
                            .small()
                            .monospace(),
                    );
                }
            });
        }
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|a| a.name.as_str()).collect();
            let hints: Vec<String> = missing
                .iter()
                .filter_map(|a| a.install_hint.as_ref().map(|h| format!("{}: {h}", a.name)))
                .collect();
            ui.add_space(2.0);
            ui.label(
                RichText::new(tr!("   No instalados: {p0}", p0 = names.join(", ")))
                    .size(11.0)
                    .color(theme::TEXT_4),
            )
            .on_hover_text(hints.join("\n"));
        }
    }

    /// Sección PROCESOS: estado, puerto y controles (aparecen al pasar el ratón).
    pub(super) fn processes_ui(ws: &Workspace, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
        ui.horizontal(|ui| {
            theme::section(ui, tr!("Procesos"));
            if !ws.processes.list.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button_sized(
                        ui,
                        icon::STOP,
                        tr!("Detener todos"),
                        20.0,
                        theme::TEXT_4,
                    )
                    .clicked()
                    {
                        cmds.push(UiCmd::StopAll);
                    }
                    if theme::icon_button_sized(
                        ui,
                        icon::PLAY,
                        tr!("Iniciar todos"),
                        20.0,
                        theme::TEXT_4,
                    )
                    .clicked()
                    {
                        cmds.push(UiCmd::StartAll);
                    }
                });
            }
        });
        if ws.processes.list.is_empty() {
            ui.label(
                RichText::new(tr!("   Define [[processes]] en .forge/project.toml"))
                    .size(11.0)
                    .color(theme::TEXT_4),
            );
        }
        for m in &ws.processes.list {
            let id = m.def.id.clone();
            let (status, tone) = m.describe();
            let detail = match m.ports.first() {
                Some(port) => format!(":{port}"),
                None => status.clone(),
            };
            let row = theme::row(ui, "●", tone_color(tone), m.def.label(), &detail, false)
                .on_hover_text(tr!(
                    "{p0}\n{status}\nClic: ver logs · clic derecho: más opciones",
                    p0 = m.def.command,
                    status = status
                ));
            if row.clicked() {
                cmds.push(UiCmd::ShowProcess(id.clone()));
            }
            // Controles al pasar el ratón, sobre el lado derecho de la fila.
            if ui.rect_contains_pointer(row.rect) {
                let size = 22.0;
                let mut x = row.rect.max.x - 2.0;
                let mut button = |glyph: &str, tip: &str, cmd: UiCmd, ui: &mut egui::Ui| {
                    x -= size;
                    let r = Rect::from_min_size(
                        egui::pos2(x, row.rect.center().y - size / 2.0),
                        Vec2::splat(size),
                    );
                    ui.painter().rect_filled(r, 6.0, theme::SURFACE);
                    let resp = ui
                        .interact(r, egui::Id::new(("proc-btn", &id, glyph)), Sense::click())
                        .on_hover_text(tip);
                    theme::paint_icon_button(ui, r, glyph, &resp, theme::TEXT_2);
                    if resp.clicked() {
                        cmds.push(cmd);
                    }
                };
                button(
                    icon::ARROW_CLOCKWISE,
                    tr!("Reiniciar"),
                    UiCmd::Restart(id.clone()),
                    ui,
                );
                if m.is_active() {
                    button(
                        icon::STOP,
                        tr!("Detener (Ctrl+C)"),
                        UiCmd::Stop(id.clone()),
                        ui,
                    );
                } else {
                    button(icon::PLAY, tr!("Iniciar"), UiCmd::Start(id.clone()), ui);
                }
            }
            row.context_menu(|ui| {
                ui.set_min_width(240.0);
                if ui
                    .button(tr!("{p0}  Ver logs", p0 = icon::TERMINAL_WINDOW))
                    .clicked()
                {
                    cmds.push(UiCmd::ShowProcess(id.clone()));
                }
                if ui
                    .button(tr!("{p0}  Copiar comando", p0 = icon::COPY))
                    .clicked()
                {
                    ui.ctx().copy_text(m.def.command.clone());
                }
                ui.menu_button(
                    tr!("{p0}  Variables de entorno", p0 = icon::LIST_CHECKS),
                    |ui| {
                        let env = ws.processes.environment(&m.def);
                        if env.is_empty() {
                            ui.label(tr!("Sin variables propias"));
                        }
                        let mut keys: Vec<_> = env.keys().collect();
                        keys.sort();
                        for key in keys {
                            ui.label(
                                RichText::new(format!("{key}={}", processes::mask(key, &env[key])))
                                    .monospace(),
                            );
                        }
                    },
                );
                ui.separator();
                let restart = match m.def.auto_restart {
                    AutoRestart::Never => tr!("no"),
                    AutoRestart::OnFailure => tr!("si falla"),
                    AutoRestart::Always => tr!("siempre"),
                };
                ui.label(
                    RichText::new(tr!("Estado: {status}", status = status))
                        .small()
                        .color(theme::TEXT_3),
                );
                ui.label(
                    RichText::new(tr!("Reinicio automático: {restart}", restart = restart))
                        .small()
                        .color(theme::TEXT_3),
                );
                // Puerto/URL solo como texto (sin abrir navegador).
                if let Some(url) = m.urls().first() {
                    ui.label(RichText::new(url).small().monospace().color(theme::TEXT_3));
                }
            });
        }
    }
}
