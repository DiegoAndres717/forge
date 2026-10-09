// Planes (⌘⇧I): lo que hay por hacer en el proyecto. Un plan tiene fases (estado, rama
// opcional y tareas); una idea sin fases espera en el backlog. Tres secciones: En progreso,
// Backlog y Completado (plegada). La lista general (sin proyecto) usa el mismo componente
// en Inicio.
use super::*;
use crate::workspace::IdeasView;
use forge_core::ideas::{Idea, Phase};
use forge_core::tr;

/// Lista a la que se refiere un comando: `Some(i)` = workspace i, `None` = general.
pub(super) type IdeasTarget = Option<usize>;

/// Por debajo de este ancho se ocultan los datos secundarios (autor, fecha).
const COMPACT: f32 = 340.0;

fn status_style(status: &str) -> (&'static str, Color32) {
    match status {
        "done" => (icon::CHECK_CIRCLE, theme::GREEN),
        "doing" => (icon::CIRCLE_HALF, theme::ORANGE),
        _ => (icon::CIRCLE, theme::TEXT_3),
    }
}

/// Un clic en el estado de una fase: backlog → en progreso → completada → backlog.
fn next_status(status: &str) -> &'static str {
    match status {
        "pending" => "doing",
        "doing" => "done",
        _ => "pending",
    }
}

fn status_label(status: &str) -> &'static str {
    forge_core::ideas::STATUSES
        .iter()
        .find(|(id, _)| *id == status)
        .map_or("", |(_, label)| forge_core::i18n::t(label))
}

/// Lo que cambia en la vista al dibujarla (se aplica al terminar, fuera del préstamo de
/// la lista).
#[derive(Default)]
struct Toggles {
    /// (plan, desplegado)
    plan: Option<(i64, bool)>,
    /// ((plan, fase), tareas a la vista)
    phase: Option<((i64, usize), bool)>,
    phase_input: Option<Option<i64>>,
}

impl App {
    /// Panel lateral del proyecto. `planner`: agente por defecto (para "Planificar con…").
    pub(super) fn ideas_ui(
        ws: &mut Workspace,
        target: IdeasTarget,
        planner: Option<&str>,
        ui: &mut egui::Ui,
        rect: Rect,
        cmds: &mut Vec<UiCmd>,
    ) {
        ui.painter().rect_filled(rect, 0.0, theme::SIDEBAR);
        ui.painter().vline(
            rect.min.x + 0.5,
            rect.y_range(),
            egui::Stroke::new(1.0, theme::SEPARATOR),
        );
        let mut ui =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(16.0, 14.0))));
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icon::LIST_CHECKS)
                    .size(18.0)
                    .color(theme::ACCENT),
            );
            ui.label(
                RichText::new(tr!("Planes"))
                    .size(15.0)
                    .color(theme::TEXT)
                    .strong(),
            );
            ui.label(
                RichText::new(ws.project.name())
                    .size(13.0)
                    .color(theme::TEXT_3),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, tr!("Cerrar (⌘⇧I)")).clicked() {
                    cmds.push(UiCmd::ToggleIdeas);
                }
            });
        });
        ui.label(
            RichText::new(tr!(
                "Cuéntale a la IA lo que quieres hacer y lo planifica por fases. Se guarda en Forge, no en el repositorio."
            ))
            .size(11.5)
            .color(theme::TEXT_3),
        );
        egui::ScrollArea::vertical()
            .id_salt("ideas-scroll")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                Self::ideas_list(&mut ws.ideas, target, planner, ui, cmds)
            });
    }

    /// Campo para anotar y las tres secciones (también en Inicio, para la lista general).
    pub(super) fn ideas_list(
        view: &mut IdeasView,
        target: IdeasTarget,
        planner: Option<&str>,
        ui: &mut egui::Ui,
        cmds: &mut Vec<UiCmd>,
    ) {
        let input = ui.add(
            theme::field(egui::TextEdit::singleline(&mut view.input))
                .hint_text(tr!("Nueva idea para el backlog… (Enter)"))
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut view.focus) {
            input.request_focus();
        }
        if input.lost_focus()
            && ui.input(|i| i.key_pressed(Key::Enter))
            && !view.input.trim().is_empty()
        {
            cmds.push(UiCmd::IdeaAdd(target));
            input.request_focus(); // para anotar varias seguidas
        }

        if view.items.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(tr!("Aún no hay planes. Anota una idea arriba, o cuéntale a la IA lo que quieres hacer: «planifica por fases…»."))
                    .color(theme::TEXT_3),
            );
            return;
        }

        let IdeasView {
            items,
            editing,
            show_done,
            toggled,
            phase_toggled,
            phase_input,
            ..
        } = view;
        let mut toggles = Toggles::default();
        let phase_cell = std::cell::RefCell::new(phase_input.take());
        let doing: Vec<&Idea> = items.iter().filter(|i| i.status == "doing").collect();
        let backlog: Vec<&Idea> = items.iter().filter(|i| i.status == "pending").collect();
        let done: Vec<&Idea> = items.iter().filter(|i| i.done()).collect();

        let mut draw = |ui: &mut egui::Ui, idea: &Idea| {
            let ctx = ItemCtx {
                target,
                planner,
                toggled,
                phase_toggled,
                phase_input: &phase_cell,
            };
            if let Some(edit) = editing.as_mut().filter(|e| e.0 == idea.id) {
                edit_card(edit, target, ui, cmds);
            } else if idea.phases.is_empty() {
                idea_row(idea, &ctx, ui, cmds);
            } else {
                plan_card(idea, &ctx, ui, cmds, &mut toggles);
            }
        };

        for (title, list) in [(tr!("En progreso"), &doing), (tr!("Backlog"), &backlog)] {
            if list.is_empty() {
                continue;
            }
            section(ui, title, list.len());
            for idea in list {
                draw(ui, idea);
            }
        }
        if !done.is_empty() {
            ui.add_space(2.0);
            let caret = if *show_done {
                icon::CARET_DOWN
            } else {
                icon::CARET_RIGHT
            };
            let label = format!(
                "{caret}  {}  {}",
                tr!("Completado").to_uppercase(),
                done.len()
            );
            let response = ui.add(
                egui::Button::new(
                    RichText::new(label)
                        .size(10.5)
                        .strong()
                        .color(theme::TEXT_3),
                )
                .frame(false),
            );
            if response.clicked() {
                *show_done = !*show_done;
            }
            if *show_done {
                for idea in &done {
                    draw(ui, idea);
                }
            }
        }

        *phase_input = phase_cell.into_inner();
        if let Some((id, open)) = toggles.plan {
            toggled.insert(id, open);
        }
        if let Some((key, open)) = toggles.phase {
            phase_toggled.insert(key, open);
        }
        if let Some(open) = toggles.phase_input {
            *phase_input = open.map(|id| (id, String::new()));
        }
    }

    /// Relee las listas visibles (los agentes escriben desde otros procesos).
    pub(super) fn ideas_tick(&mut self) {
        let every = |view: &IdeasView, secs| {
            view.dirty
                || view
                    .loaded
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(secs))
        };
        match self.active {
            Some(i) => {
                let view = &self.workspaces[i].ideas;
                // Abierto: lista completa cada 2 s; cerrado: solo el contador cada 5 s.
                let (full, count) = (view.open && every(view, 2), !view.open && every(view, 5));
                if !(full || count) {
                    return;
                }
                let path = self.workspaces[i].project.path.clone();
                let open_count = self.db(|s| s.open_ideas(Some(&path))).unwrap_or(0);
                let items = full.then(|| {
                    self.db(|s| s.list_ideas(Some(&path), true))
                        .unwrap_or_default()
                });
                let view = &mut self.workspaces[i].ideas;
                if let Some(items) = items {
                    view.items = items;
                }
                (view.open_count, view.dirty, view.loaded) =
                    (open_count, false, Some(Instant::now()));
            }
            None if every(&self.general, 2) => {
                let items = self.db(|s| s.list_ideas(None, true)).unwrap_or_default();
                let view = &mut self.general;
                view.open_count = items.iter().filter(|i| !i.done()).count() as i64;
                (view.items, view.dirty, view.loaded) = (items, false, Some(Instant::now()));
            }
            None => {}
        }
    }

    /// Comandos de planes (de un workspace o de la lista general).
    pub(super) fn apply_idea(&mut self, cmd: UiCmd) {
        let target = match &cmd {
            UiCmd::IdeaAdd(t) | UiCmd::IdeaSave(t) | UiCmd::IdeaEdit(t, _) => *t,
            UiCmd::IdeaSet(t, ..) | UiCmd::IdeaDelete(t, _) => *t,
            UiCmd::PhaseSet(t, ..) | UiCmd::TaskToggle(t, ..) | UiCmd::PhaseAdd(t, _) => *t,
            _ => return,
        };
        let path = target
            .and_then(|i| self.workspaces.get(i))
            .map(|w| w.project.path.clone());
        if target.is_some() && path.is_none() {
            return;
        }
        let project = path.as_deref();
        let view = self.ideas_view(target);
        view.dirty = true;
        match cmd {
            UiCmd::IdeaAdd(_) => {
                let title = std::mem::take(&mut view.input);
                if self
                    .db(|s| s.add_idea(project, &title, "", "usuario"))
                    .is_none()
                {
                    self.ideas_view(target).input = title; // se conserva lo escrito
                }
            }
            UiCmd::IdeaEdit(_, id) => {
                view.editing = id.and_then(|id| {
                    let idea = view.items.iter().find(|i| i.id == id)?;
                    Some((id, idea.title.clone(), idea.note.clone()))
                });
            }
            UiCmd::IdeaSave(_) => {
                let Some((id, title, note)) = view.editing.take() else {
                    return;
                };
                if self
                    .db(|s| s.update_idea(project, id, None, Some(&title), Some(&note), "usuario"))
                    .is_none()
                {
                    self.ideas_view(target).editing = Some((id, title, note));
                }
            }
            UiCmd::IdeaSet(_, id, status) => {
                self.db(|s| s.update_idea(project, id, Some(status), None, None, "usuario"));
            }
            UiCmd::IdeaDelete(_, id) => {
                self.db(|s| s.delete_idea(project, id));
            }
            UiCmd::PhaseSet(_, id, phase, status) => {
                let change = forge_core::ideas::PhaseChange {
                    status: Some(status),
                    ..Default::default()
                };
                self.db(|s| s.update_phase(project, id, phase, change, "usuario"));
            }
            UiCmd::TaskToggle(_, id, phase, task, done) => {
                let change = forge_core::ideas::PhaseChange {
                    task: Some((task, done)),
                    ..Default::default()
                };
                self.db(|s| s.update_phase(project, id, phase, change, "usuario"));
            }
            UiCmd::PhaseAdd(_, id) => {
                let Some((_, title)) = view.phase_input.take() else {
                    return;
                };
                let Some(mut phases) = view
                    .items
                    .iter()
                    .find(|i| i.id == id)
                    .map(|i| i.phases.clone())
                else {
                    return;
                };
                if title.trim().is_empty() {
                    return;
                }
                phases.push(Phase::new(&title));
                self.db(|s| s.set_phases(project, id, &phases, "usuario"));
                // El campo sigue abierto para añadir varias fases seguidas.
                self.ideas_view(target).phase_input = Some((id, String::new()));
            }
            _ => {}
        }
    }

    /// Agente por defecto del proyecto que puede planificar (lee y escribe los planes por
    /// MCP): (id, nombre).
    pub(super) fn planner(&self, workspace: usize) -> Option<(String, String)> {
        let detected = self.agents.lock().ok()?.clone();
        let ws = self.workspaces.get(workspace)?;
        agents::default_agent(&ws.project.agents, &detected)
            // Se conecta a los planes (MCP) y sabe abrirse con un primer mensaje.
            .filter(|a| a.mcp_style.is_some() && a.with_prompt(&a.command, "").is_some())
            .map(|a| (a.id.clone(), a.name.clone()))
    }

    /// "Planificar con…": el agente por defecto recibe la petición de planificar la idea.
    pub(super) fn plan_with_agent(&mut self, ctx: &egui::Context, id: i64, area: Rect) {
        let Some(i) = self.active else { return };
        let Some((agent, _)) = self.planner(i) else {
            self.error = Some(
                tr!("Para planificar con IA hace falta Claude Code, Codex u OpenCode instalado")
                    .into(),
            );
            return;
        };
        let ws = &mut self.workspaces[i];
        let Some(idea) = ws.ideas.items.iter().find(|i| i.id == id) else {
            return;
        };
        let prompt = tr!(
            "Planifica por fases la idea #{id} de los planes de Forge («{title}»). Revisa el proyecto lo necesario, propón las fases en orden (con rama solo si la merecen) y sus tareas, y guárdalo con plan_update (phases). No escribas código todavía.",
            id = id,
            title = idea.title
        );
        ws.ideas.open = false; // que se vea el agente trabajando
        ws.ask_agent(ctx, &agent, &prompt, area);
    }

    fn ideas_view(&mut self, target: IdeasTarget) -> &mut IdeasView {
        match target {
            Some(i) => &mut self.workspaces[i].ideas,
            None => &mut self.general,
        }
    }
}

/// Lo que comparte cada elemento al dibujarse.
struct ItemCtx<'a> {
    target: IdeasTarget,
    planner: Option<&'a str>,
    toggled: &'a HashMap<i64, bool>,
    phase_toggled: &'a HashMap<(i64, usize), bool>,
    /// Campo "Añadir fase" (lo escribe una tarjeta mientras las demás solo leen).
    phase_input: &'a std::cell::RefCell<Option<(i64, String)>>,
}

/// Cabecera de sección: `EN PROGRESO  2`.
fn section(ui: &mut egui::Ui, title: &str, count: usize) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title.to_uppercase())
                .size(10.5)
                .strong()
                .color(theme::TEXT_3),
        );
        ui.label(
            RichText::new(count.to_string())
                .size(10.5)
                .color(theme::TEXT_4),
        );
    });
}

/// Una idea del backlog (sin fases): estado, título, nota y "Planificar con…".
fn idea_row(idea: &Idea, ctx: &ItemCtx, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
    let (glyph, color) = status_style(&idea.status);
    let next = if idea.done() { "pending" } else { "done" };
    let tip = if idea.done() {
        tr!("Volver al backlog")
    } else {
        tr!("Marcar como completada")
    };
    let compact = ui.available_width() < COMPACT;
    ui.horizontal_top(|ui| {
        if theme::icon_button_sized(ui, glyph, &format!("{tip}: {}", idea.title), 22.0, color)
            .clicked()
        {
            cmds.push(UiCmd::IdeaSet(ctx.target, idea.id, next));
        }
        let width = (ui.available_width() - 22.0 - ui.spacing().item_spacing.x).max(80.0);
        ui.allocate_ui(Vec2::new(width, 0.0), |ui| {
            // Ocupa todo el ancho: los botones de la derecha quedan alineados.
            ui.set_min_width(width);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let mut title = RichText::new(&idea.title).color(theme::TEXT);
                if idea.done() {
                    title = title.strikethrough().color(theme::TEXT_4);
                }
                let response = ui.add(egui::Label::new(title).wrap().sense(Sense::click()));
                if response.double_clicked() {
                    cmds.push(UiCmd::IdeaEdit(ctx.target, Some(idea.id)));
                }
                response.context_menu(|ui| item_menu(ui, idea, ctx, cmds));
                if !idea.note.is_empty() {
                    ui.add(
                        egui::Label::new(RichText::new(&idea.note).size(12.0).color(theme::TEXT_3))
                            .wrap(),
                    );
                }
                if !compact {
                    ui.label(RichText::new(meta(idea)).size(11.0).color(theme::TEXT_4));
                }
                if let (Some(agent), false) = (ctx.planner, idea.done()) {
                    let label = format!(
                        "{}  {}",
                        icon::SPARKLE,
                        tr!("Planificar con {agent}", agent = agent)
                    );
                    let plan = ui
                        .add(
                            egui::Button::new(RichText::new(label).size(12.0).color(theme::ACCENT))
                                .frame(false),
                        )
                        .on_hover_text(tr!(
                            "La IA propone las fases y tareas y las guarda en este plan"
                        ));
                    if plan.clicked() {
                        cmds.push(UiCmd::PlanWithAgent(idea.id));
                    }
                }
            });
        });
        more_button(ui, idea, ctx, cmds);
    });
}

/// Un plan con fases, en tarjeta: estado, título, avance y (desplegado) las fases.
fn plan_card(
    idea: &Idea,
    ctx: &ItemCtx,
    ui: &mut egui::Ui,
    cmds: &mut Vec<UiCmd>,
    toggles: &mut Toggles,
) {
    // Lo elegido a mano se respeta (aunque cambie de sección); si no, en progreso se ve
    // desplegado y el resto plegado.
    let expanded = ctx
        .toggled
        .get(&idea.id)
        .copied()
        .unwrap_or(idea.status == "doing");
    let total = idea.phases.len();
    let finished = idea.phases.iter().filter(|p| p.status == "done").count();
    let compact = ui.available_width() < COMPACT;
    theme::card(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal_top(|ui| {
            let (glyph, color) = status_style(&idea.status);
            let tip = tr!(
                "{status} (según sus fases)",
                status = status_label(&idea.status)
            );
            ui.label(RichText::new(glyph).size(17.0).color(color))
                .on_hover_text(tip);
            let width = (ui.available_width() - 44.0 - 2.0 * ui.spacing().item_spacing.x).max(80.0);
            ui.allocate_ui(Vec2::new(width, 0.0), |ui| {
                // Ocupa todo el ancho: los botones de la derecha quedan alineados.
                ui.set_min_width(width);
                let mut title = RichText::new(&idea.title).color(theme::TEXT).strong();
                if idea.done() {
                    title = title.color(theme::TEXT_3);
                }
                let response = ui.add(egui::Label::new(title).wrap().sense(Sense::click()));
                if response.clicked() {
                    toggles.plan = Some((idea.id, !expanded));
                }
                if response.double_clicked() {
                    cmds.push(UiCmd::IdeaEdit(ctx.target, Some(idea.id)));
                }
                response.context_menu(|ui| item_menu(ui, idea, ctx, cmds));
            });
            let caret = if expanded {
                icon::CARET_DOWN
            } else {
                icon::CARET_RIGHT
            };
            let hint = if expanded {
                tr!("Plegar fases")
            } else {
                tr!("Ver fases")
            };
            if theme::icon_button_sized(ui, caret, hint, 22.0, theme::TEXT_3).clicked() {
                toggles.plan = Some((idea.id, !expanded));
            }
            more_button(ui, idea, ctx, cmds);
        });

        // Avance: "Fase 2 de 4 · Panel" y barra.
        let progress = match idea.current_phase() {
            Some((n, phase)) => tr!(
                "Fase {n} de {total} · {phase}",
                n = n + 1,
                total = total,
                phase = phase.title
            ),
            None => tr!("{total} fases completadas", total = total),
        };
        ui.add(
            egui::Label::new(RichText::new(progress).size(12.0).color(theme::TEXT_2)).truncate(),
        );
        progress_bar(ui, finished as f32 / total.max(1) as f32, idea.done());
        if !compact {
            ui.label(RichText::new(meta(idea)).size(11.0).color(theme::TEXT_4));
        }

        if expanded {
            if !idea.note.is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(&idea.note).size(12.0).color(theme::TEXT_3))
                        .wrap(),
                );
            }
            ui.add_space(2.0);
            for (index, phase) in idea.phases.iter().enumerate() {
                phase_row(idea, index, phase, ctx, ui, cmds, toggles);
            }
            add_phase(idea, ctx, ui, cmds, toggles);
        }
    });
}

/// Una fase: estado (clic para avanzarla), número y título, rama y tareas.
fn phase_row(
    idea: &Idea,
    index: usize,
    phase: &Phase,
    ctx: &ItemCtx,
    ui: &mut egui::Ui,
    cmds: &mut Vec<UiCmd>,
    toggles: &mut Toggles,
) {
    let current = idea.current_phase().map(|(n, _)| n) == Some(index);
    // Las tareas de la fase en curso se ven; las demás, al pulsar la fase (y lo elegido a
    // mano se respeta).
    let open = ctx
        .phase_toggled
        .get(&(idea.id, index))
        .copied()
        .unwrap_or(current);
    let (glyph, color) = status_style(&phase.status);
    let next = next_status(&phase.status);
    ui.horizontal_top(|ui| {
        let tip = tr!(
            "{status}: pasar a {next}",
            status = status_label(&phase.status),
            next = status_label(next)
        );
        if theme::icon_button_sized(ui, glyph, &format!("{tip} — {}", phase.title), 20.0, color)
            .clicked()
        {
            cmds.push(UiCmd::PhaseSet(ctx.target, idea.id, index, next));
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut title = RichText::new(format!("{}. {}", index + 1, phase.title)).size(13.0);
            title = match phase.status.as_str() {
                "done" => title.color(theme::TEXT_3),
                _ if current => title.color(theme::TEXT).strong(),
                _ => title.color(theme::TEXT_2),
            };
            let response = ui.add(egui::Label::new(title).wrap().sense(Sense::click()));
            if response.clicked() && !phase.tasks.is_empty() {
                toggles.phase = Some(((idea.id, index), !open));
            }
            // Rama (clic: copiar) y tareas hechas, en una línea que se parte si no cabe.
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(branch) = &phase.branch {
                    branch_chip(ui, branch);
                }
                if !phase.tasks.is_empty() {
                    let done = phase.tasks.iter().filter(|t| t.done).count();
                    let caret = if open {
                        icon::CARET_DOWN
                    } else {
                        icon::CARET_RIGHT
                    };
                    let text = tr!(
                        "{done}/{total} tareas",
                        done = done,
                        total = phase.tasks.len()
                    );
                    let tasks = ui.add(
                        egui::Button::new(
                            RichText::new(format!("{caret} {text}"))
                                .size(11.0)
                                .color(theme::TEXT_3),
                        )
                        .frame(false),
                    );
                    if tasks.clicked() {
                        toggles.phase = Some(((idea.id, index), !open));
                    }
                }
            });
            if open {
                if !phase.notes.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&phase.notes).size(11.5).color(theme::TEXT_3),
                        )
                        .wrap(),
                    );
                }
                for (t, task) in phase.tasks.iter().enumerate() {
                    ui.horizontal_top(|ui| {
                        let (glyph, color, tip) = if task.done {
                            (icon::CHECK_SQUARE, theme::GREEN, tr!("Desmarcar"))
                        } else {
                            (icon::SQUARE, theme::TEXT_3, tr!("Tachar"))
                        };
                        let check = theme::icon_button_sized(
                            ui,
                            glyph,
                            &format!("{tip}: {}", task.text),
                            18.0,
                            color,
                        );
                        if check.clicked() {
                            cmds.push(UiCmd::TaskToggle(ctx.target, idea.id, index, t, !task.done));
                        }
                        let mut text = RichText::new(&task.text).size(12.0).color(theme::TEXT_2);
                        if task.done {
                            text = text.strikethrough().color(theme::TEXT_4);
                        }
                        ui.add(egui::Label::new(text).wrap());
                    });
                }
            }
        });
    });
}

/// "+ Añadir fase" (abre un campo; Enter añade y deja el campo para la siguiente).
fn add_phase(
    idea: &Idea,
    ctx: &ItemCtx,
    ui: &mut egui::Ui,
    cmds: &mut Vec<UiCmd>,
    toggles: &mut Toggles,
) {
    let mut input = ctx.phase_input.borrow_mut();
    match input.as_mut().filter(|(id, _)| *id == idea.id) {
        Some((_, text)) => {
            let field = ui.add(
                theme::field(egui::TextEdit::singleline(text))
                    .hint_text(tr!("Nueva fase… (Enter para añadir, Esc para cerrar)"))
                    .desired_width(f32::INFINITY),
            );
            if !field.has_focus() && !field.lost_focus() {
                field.request_focus();
            }
            if field.lost_focus() {
                if ui.input(|i| i.key_pressed(Key::Enter)) && !text.trim().is_empty() {
                    cmds.push(UiCmd::PhaseAdd(ctx.target, idea.id));
                } else {
                    toggles.phase_input = Some(None);
                }
            }
        }
        None => {
            let label = format!("{}  {}", icon::PLUS, tr!("Añadir fase"));
            let add = ui.add(
                egui::Button::new(RichText::new(label).size(12.0).color(theme::TEXT_3))
                    .frame(false),
            );
            if add.clicked() {
                toggles.phase_input = Some(Some(idea.id));
            }
        }
    }
}

/// Edición de título y nota (doble clic o menú → Editar).
fn edit_card(
    edit: &mut (i64, String, String),
    target: IdeasTarget,
    ui: &mut egui::Ui,
    cmds: &mut Vec<UiCmd>,
) {
    let (_, title, note) = edit;
    theme::card(ui, |ui| {
        ui.add(theme::field(egui::TextEdit::singleline(title)).desired_width(f32::INFINITY));
        ui.add(
            theme::field(egui::TextEdit::multiline(note))
                .hint_text(tr!("Detalle o criterio de terminado (opcional)"))
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if theme::primary(ui, tr!("Guardar")).clicked() {
                cmds.push(UiCmd::IdeaSave(target));
            }
            if theme::secondary(ui, tr!("Cancelar")).clicked() {
                cmds.push(UiCmd::IdeaEdit(target, None));
            }
        });
    });
}

/// Chip de la rama de una fase; un clic copia el nombre.
fn branch_chip(ui: &mut egui::Ui, branch: &str) {
    let shown = if branch.chars().count() > 34 {
        format!("{}…", branch.chars().take(33).collect::<String>())
    } else {
        branch.to_string()
    };
    // Fuente normal: los iconos no existen en la monoespaciada.
    let text = RichText::new(format!("{}  {shown}", icon::GIT_BRANCH))
        .size(11.5)
        .color(theme::TEXT_2);
    let chip = ui
        .add(
            egui::Button::new(text)
                .fill(theme::SURFACE_HOVER)
                .corner_radius(9.0)
                .min_size(Vec2::new(0.0, 20.0)),
        )
        .on_hover_text(tr!("Copiar «{branch}»", branch = branch));
    if chip.clicked() {
        ui.ctx().copy_text(branch.to_string());
    }
}

/// Barra fina de avance (verde al completarse).
fn progress_bar(ui: &mut egui::Ui, fraction: f32, done: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, theme::SURFACE_HOVER);
    let mut fill = rect;
    fill.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    let color = if done { theme::GREEN } else { theme::ACCENT };
    ui.painter().rect_filled(fill, 2.0, color);
}

/// "#3 · claude-code · hace 2 h".
fn meta(idea: &Idea) -> String {
    format!(
        "#{} · {} · {}",
        idea.id,
        idea.source,
        store::ago(idea.updated_at)
    )
}

/// Botón "⋯" con el mismo menú que el clic derecho (para que se descubra).
fn more_button(ui: &mut egui::Ui, idea: &Idea, ctx: &ItemCtx, cmds: &mut Vec<UiCmd>) {
    let more = theme::icon_button_sized(
        ui,
        icon::DOTS_THREE,
        &tr!("Más acciones: {title}", title = idea.title),
        22.0,
        theme::TEXT_3,
    );
    egui::Popup::menu(&more).show(|ui| item_menu(ui, idea, ctx, cmds));
}

fn item_menu(ui: &mut egui::Ui, idea: &Idea, ctx: &ItemCtx, cmds: &mut Vec<UiCmd>) {
    ui.set_min_width(200.0);
    // Con fases, el estado lo dan sus fases.
    if idea.phases.is_empty() {
        for (status, label) in forge_core::ideas::STATUSES {
            if status != idea.status
                && ui
                    .button(format!(
                        "{}  {}",
                        status_style(status).0,
                        forge_core::i18n::t(label)
                    ))
                    .clicked()
            {
                cmds.push(UiCmd::IdeaSet(ctx.target, idea.id, status));
            }
        }
        if let (Some(agent), false) = (ctx.planner, idea.done()) {
            let label = format!(
                "{}  {}",
                icon::SPARKLE,
                tr!("Planificar con {agent}", agent = agent)
            );
            if ui.button(label).clicked() {
                cmds.push(UiCmd::PlanWithAgent(idea.id));
            }
        }
        ui.separator();
    }
    if ui
        .button(tr!("{p0}  Editar", p0 = icon::PENCIL_SIMPLE))
        .clicked()
    {
        cmds.push(UiCmd::IdeaEdit(ctx.target, Some(idea.id)));
    }
    if ui.button(tr!("{p0}  Borrar", p0 = icon::TRASH)).clicked() {
        cmds.push(UiCmd::IdeaDelete(ctx.target, idea.id));
    }
}
