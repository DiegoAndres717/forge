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
        // Con Ajustes abiertos la barra lateral pasa a ser su índice.
        if self.settings_open {
            self.settings_nav(ui, rect, cmds);
            return;
        }
        let detected = self.agents.lock().unwrap().clone();
        let detecting = self.detecting.load(Ordering::Relaxed);
        let body = Rect::from_min_max(
            rect.min + Vec2::new(10.0, 8.0),
            rect.max - Vec2::new(10.0, 8.0),
        );
        // Ajustes, fijo abajo.
        let (body, footer) = body.split_top_bottom_at_y(body.max.y - 34.0);
        let mut bottom = ui.new_child(egui::UiBuilder::new().max_rect(footer));
        if theme::row(
            &mut bottom,
            icon::GEAR_SIX,
            theme::TEXT_2,
            tr!("Ajustes"),
            &forge_core::i18n::keys("⌘,"),
            false,
        )
        .clicked()
        {
            cmds.push(UiCmd::OpenSettings);
        }
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
        ui.spacing_mut().item_spacing.y = 1.0;
        egui::ScrollArea::vertical()
            .id_salt("sidebar-scroll")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                theme::section(ui, "Workspaces");
                if theme::row(
                    ui,
                    icon::HOUSE,
                    theme::TEXT_2,
                    tr!("Inicio"),
                    &forge_core::i18n::keys("⌘⇧H"),
                    self.active.is_none(),
                )
                .clicked()
                {
                    cmds.push(UiCmd::Home);
                }
                let attention: Vec<_> = self.workspaces.iter().map(|w| w.attention()).collect();
                // La rama de cada proyecto (se relee cada 2 s) y los agentes abiertos del activo.
                let branches: Vec<Option<String>> =
                    self.workspaces.iter_mut().map(|w| w.branch()).collect();
                let open_agents: Vec<_> = self
                    .active
                    .map(|a| self.workspaces[a].active_agent_panels())
                    .unwrap_or_default();
                let visible = self.visible_workspaces(&attention);
                let ram = self.ram.lock().map(|r| r.clone()).unwrap_or_default();
                // Filas dibujadas y el arrastre en curso (desde, ¿soltado?).
                let mut rows: Vec<(usize, Rect)> = Vec::new();
                let mut drag = None;
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
                        (_, i) if i < 9 => {
                            forge_core::i18n::keys(&format!("⌘{}", i + 1)).into_owned()
                        }
                        _ => String::new(),
                    };
                    let mut tip = tilde(&ws.project.path);
                    if let Some(bytes) = ram.get(&ws.project.path) {
                        tip += &format!("\nMemoria: {}", human_bytes(*bytes));
                    }
                    if dormant {
                        tip += tr!("\nDormido: sus terminales y procesos arrancan al entrar.");
                    }
                    // El punto de estado: aviso, en marcha, dormido o en calma.
                    let mut dot = match (dormant, running > 0) {
                        (true, _) => theme::TEXT_4,
                        (_, true) => theme::GREEN,
                        _ => theme::TEXT_3,
                    };
                    let row = theme::project_card(
                        ui,
                        dot,
                        &ws.project.name(),
                        branches[i].as_deref().unwrap_or(""),
                        &detail,
                        selected,
                    );
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
                            Attention::Event(_, text) => (theme::ORANGE, text.clone()),
                        };
                        dot = color;
                        ui.painter().circle_filled(
                            egui::pos2(row.rect.min.x + 14.0, row.rect.center().y),
                            4.0,
                            dot,
                        );
                        tip = format!("{text}\n{tip}");
                    }
                    // Se agarra por el icono de carpeta para reordenar.
                    let grip = ui
                        .interact(
                            Rect::from_min_size(row.rect.min, Vec2::new(30.0, row.rect.height())),
                            egui::Id::new(("project-grip", &ws.project.path)),
                            Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab);
                    if grip.dragged() || grip.drag_stopped() {
                        drag = Some((i, grip.drag_stopped()));
                    }
                    // Clic y menú igual sobre el icono que sobre el resto de la fila.
                    let row = row.union(grip).on_hover_text(tip);
                    if row.clicked() {
                        cmds.push(UiCmd::Activate(i));
                    }
                    rows.push((i, row.rect));
                    row.context_menu(|ui| {
                        editor_menu(ui, i, self.store.as_ref(), cmds);
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
                    // Los agentes abiertos del proyecto elegido, debajo de su tarjeta.
                    if selected {
                        for (panel, name, _) in &open_agents {
                            if theme::sub_row(ui, theme::GREEN, name, "").clicked() {
                                cmds.push(UiCmd::GoToPanel(*panel));
                            }
                        }
                    }
                }
                if let Some((from, dropped)) = drag
                    && let Some(p) = ui.ctx().pointer_interact_pos()
                    && let Some(last) = rows.last()
                {
                    // Ranura: antes de la primera fila cuyo centro queda bajo el puntero.
                    let (to, y) = match rows.iter().find(|(_, r)| p.y < r.center().y) {
                        Some((i, r)) => (*i, r.min.y),
                        None => (last.0 + 1, last.1.max.y),
                    };
                    if dropped {
                        cmds.push(UiCmd::MoveProject(from, to));
                    } else {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        ui.painter().hline(
                            (last.1.min.x + 8.0)..=(last.1.max.x - 8.0),
                            y,
                            egui::Stroke::new(2.0, theme::ACCENT),
                        );
                    }
                }
                let hidden = self.workspaces.len() - visible.len();
                if hidden > 0 || self.show_all {
                    let label = if self.show_all {
                        tr!("Mostrar menos").to_string()
                    } else {
                        tr!("{hidden} más…", hidden = hidden)
                    };
                    if theme::row(
                        ui,
                        icon::DOTS_THREE,
                        theme::TEXT_3,
                        &label,
                        &forge_core::i18n::keys("⌘K"),
                        false,
                    )
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
                    &forge_core::i18n::keys("⌘O"),
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

                // Los procesos se usan menos: al final de la lista.
                ui.add_space(12.0);
                Self::processes_ui(ws, ui, cmds);
            });
    }

    /// Sección AGENTES: instalados y configurados. Clic: a su terminal abierta, o reanuda su
    /// última sesión, o una nueva; + o ⌥-clic: sesión nueva; clic derecho: más.
    pub(super) fn agents_ui(
        ws: &mut Workspace,
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
        let sessions = ws.last_sessions().clone();
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
            let open = ws.agent_panels(&agent.id);
            let session = sessions.get(&agent.id).cloned();
            let row_rect =
                Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 26.0));
            let hovered = installed && ui.rect_contains_pointer(row_rect);
            let detail = match (&session, open.len()) {
                _ if hovered => String::new(), // sitio para el +
                (_, 1) => tr!("abierto").into(),
                (_, n @ 2..) => tr!("{n} abiertos", n = n),
                (Some(s), _) => forge_core::store::ago_precise(s.when),
                (None, _) => version.clone(),
            };
            let click = match (&session, open.is_empty()) {
                (_, false) => tr!("Clic: ir a su terminal").into(),
                (Some(s), true) if !s.topic.is_empty() => tr!(
                    "Clic: reanudar «{p0}» ({p1})",
                    p0 = s.topic,
                    p1 = forge_core::store::ago_precise(s.when)
                ),
                (Some(s), true) => tr!(
                    "Clic: reanudar la última sesión ({p0})",
                    p0 = forge_core::store::ago_precise(s.when)
                ),
                (None, true) => tr!("Clic: sesión nueva").into(),
            };
            let color = if installed {
                theme::ORANGE
            } else {
                theme::TEXT_4
            };
            let tip = match found.and_then(|d| d.path.as_ref()) {
                Some(path) => tr!(
                    "{p0} {p1}\n{p2}\n\n{click}\n⌥-clic o +: sesión nueva · clic derecho: más",
                    p0 = agent.command,
                    p1 = version,
                    p2 = path.display(),
                    click = click
                ),
                None => tr!("{p0} no está instalado", p0 = agent.program()),
            };
            let has_logo = theme::has_agent_logo(agent.program());
            let glyph = if has_logo { "" } else { icon::ROBOT };
            let row = theme::row(
                ui,
                glyph,
                color,
                &match ws.account_for(agent.program(), None) {
                    Some(a) if ws.has_accounts(agent.program()) => {
                        format!("{} · {}{star}", agent.name, a.name)
                    }
                    _ => format!("{}{star}", agent.name),
                },
                &detail,
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
                let new = ui.input(|i| i.modifiers.alt);
                cmds.push(UiCmd::AgentClick(agent.id.clone(), new));
            }
            if hovered {
                let at = Rect::from_center_size(
                    egui::pos2(row.rect.max.x - 16.0, row.rect.center().y),
                    Vec2::splat(22.0),
                );
                let plus = ui
                    .put(
                        at,
                        egui::Button::new(RichText::new(icon::PLUS).color(theme::TEXT_3))
                            .frame(false),
                    )
                    .on_hover_text(tr!("Sesión nueva"));
                if plus.clicked() {
                    cmds.push(UiCmd::AgentClick(agent.id.clone(), true));
                }
            }
            row.context_menu(|ui| {
                ui.set_min_width(240.0);
                for panel in &open {
                    let label = tr!(
                        "{p0}  Ir a «{p1}»",
                        p0 = icon::TERMINAL_WINDOW,
                        p1 = ws.tab_name(*panel)
                    );
                    if ui.button(label).clicked() {
                        cmds.push(UiCmd::GoToPanel(*panel));
                    }
                }
                if ui
                    .add_enabled(
                        installed,
                        egui::Button::new(tr!("{p0}  Sesión nueva", p0 = icon::PLUS)),
                    )
                    .clicked()
                {
                    cmds.push(UiCmd::AgentClick(agent.id.clone(), true));
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
                // Contra «lost in the middle»: una sesión nueva y corta del mismo agente
                // que empieza con el resumen de la anterior, en vez de arrastrar una larga.
                if session.is_some()
                    && agent.with_prompt(&agent.command, "x").is_some()
                    && ui
                        .add_enabled(
                            installed,
                            egui::Button::new(tr!("{p0}  Empezar limpio", p0 = icon::SPARKLE)),
                        )
                        .on_hover_text(tr!("Abre una sesión nueva de {p0} que empieza con un resumen de la última (lo hecho, lo pendiente y los archivos tocados). Con conversaciones largas el agente se pierde en lo del medio; así arranca con poco contexto y lo importante delante.", p0 = agent.name))
                        .clicked()
                {
                    cmds.push(UiCmd::AgentHandoff(agent.id.clone(), agent.id.clone()));
                }
                // Seguir la última conversación de este agente en otro (con un resumen).
                if session.is_some() {
                    for to in ws.agent_handoff_targets(&agent.id) {
                        if ui
                            .button(tr!(
                                "{p0}  Seguir la conversación en {p1}",
                                p0 = icon::ARROW_SQUARE_OUT,
                                p1 = to.name
                            ))
                            .on_hover_text(tr!("Abre {p0} con un resumen de la última conversación (lo hecho, lo pendiente y los archivos tocados)", p0 = to.name))
                            .clicked()
                        {
                            cmds.push(UiCmd::AgentHandoff(agent.id.clone(), to.id.clone()));
                        }
                    }
                }
                if forge_core::accounts::supported(agent.program()) {
                    ui.separator();
                    let current = ws.account_for(agent.program(), None).map(|a| a.id);
                    for a in ws.accounts_of(agent.program()) {
                        if ui
                            .radio(current == Some(a.id), &a.name)
                            .on_hover_text(tr!("Usar esta cuenta en este proyecto"))
                            .clicked()
                        {
                            cmds.push(UiCmd::UseAccount(a.program.clone(), a.id));
                        }
                    }
                    // Con su terminal abierta: pasar esa conversación a otra cuenta.
                    if let Some(panel) = open.first() {
                        for a in ws.accounts_of(agent.program()) {
                            if current == Some(a.id) {
                                continue;
                            }
                            if ui
                                .button(tr!(
                                    "{p0}  Seguir la conversación con «{p1}»",
                                    p0 = icon::USER_SWITCH,
                                    p1 = a.name
                                ))
                                .clicked()
                            {
                                cmds.push(UiCmd::Handoff(*panel, a.id));
                            }
                        }
                    }
                    if ui
                        .button(tr!("{p0}  Cuentas…", p0 = icon::USER))
                        .on_hover_text(tr!("Añadir, renombrar o iniciar sesión (Ajustes)"))
                        .clicked()
                    {
                        cmds.push(UiCmd::OpenSettingsTab(super::settings::SettingsTab::Agents));
                    }
                    ui.separator();
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
        let add = || UiCmd::OpenSettingsTab(super::settings::SettingsTab::Project);
        ui.horizontal(|ui| {
            theme::section(ui, tr!("Procesos"));
            if !ws.processes.list.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button_sized(
                        ui,
                        icon::PLUS,
                        tr!("Añadir o cambiar procesos (Ajustes)"),
                        20.0,
                        theme::TEXT_4,
                    )
                    .clicked()
                    {
                        cmds.push(add());
                    }
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
        if ws.processes.list.is_empty()
            && theme::row(ui, icon::PLUS, theme::TEXT_3, tr!("Añadir proceso"), "", false)
                .on_hover_text(tr!("Servidores como dev o api que Forge arranca, vigila y reinicia. Se configuran en Ajustes → Este proyecto."))
                .clicked()
        {
            cmds.push(add());
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

/// «Abrir en <preferido>» y, con varios editores, «Abrir con» y la lista.
fn editor_menu(ui: &mut egui::Ui, i: usize, store: Option<&Store>, cmds: &mut Vec<UiCmd>) {
    let editors = super::editors::installed();
    let Some((name, bundle)) = super::editors::preferred(store, editors) else {
        return;
    };
    // Cada editor con su icono de macOS (o el genérico si no se pudo leer).
    let button = |ui: &mut egui::Ui, bundle: &str, text: String| match super::editors::icon(
        ui.ctx(),
        bundle,
    ) {
        Some(t) => ui.add(egui::Button::image_and_text(
            egui::Image::new((t.id(), Vec2::splat(16.0))),
            text,
        )),
        None => ui.button(format!("{}  {text}", icon::CODE)),
    };
    if button(ui, bundle, tr!("Abrir en {name}", name = name)).clicked() {
        cmds.push(UiCmd::OpenInEditor(i, Some(bundle)));
    }
    if editors.len() > 1 {
        ui.menu_button(tr!("{p0}  Abrir con", p0 = icon::CODE), |ui| {
            for &(name, bundle) in editors {
                if button(ui, bundle, name.to_string()).clicked() {
                    cmds.push(UiCmd::OpenInEditor(i, Some(bundle)));
                }
            }
        });
    }
    ui.separator();
}
