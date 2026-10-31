// Planes (⌘⇧I): lo que hay por hacer en el proyecto. Un plan tiene fases (estado, rama
// opcional y tareas); una idea sin fases espera en el backlog. Tres secciones: En progreso,
// Backlog y Completado (plegada). La lista general (sin proyecto) usa el mismo componente
// en Inicio.
use super::*;
use crate::workspace::{IdeasView, LearnItem, Learning};
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
        let reviewers = view.reviewers.clone();
        let comments = view.comments.clone();
        let runs = view.runs.clone();
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
            highlighted,
            phase_input,
            learning,
            ..
        } = view;
        let mut toggles = Toggles::default();
        let phase_cell = std::cell::RefCell::new(phase_input.take());
        let learn_cell = std::cell::RefCell::new(learning.take());
        let doing: Vec<&Idea> = items.iter().filter(|i| i.status == "doing").collect();
        let backlog: Vec<&Idea> = items.iter().filter(|i| i.status == "pending").collect();
        let done: Vec<&Idea> = items.iter().filter(|i| i.done()).collect();

        let mut draw = |ui: &mut egui::Ui, idea: &Idea| {
            let ctx = ItemCtx {
                target,
                planner,
                toggled,
                phase_toggled,
                reviewers: &reviewers,
                comments: &comments,
                runs: &runs,
                learning: &learn_cell,
                highlighted: *highlighted,
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
        *learning = learn_cell.into_inner();
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
        // Cada dos minutos, en otro hilo (git puede tardar): cierra las fases cuya rama ya
        // está en main. El panel lo verá en su próxima relectura.
        if self.merged_check.elapsed() > Duration::from_secs(120)
            && let Some(db) = self.store.as_ref().and_then(|s| s.conn.path())
            && !db.is_empty()
            // Con git lento y muchos proyectos, no se apila otra búsqueda sobre la anterior.
            && !self.merging.swap(true, Ordering::AcqRel)
        {
            self.merged_check = Instant::now();
            let db = PathBuf::from(db);
            let paths: Vec<_> = self
                .workspaces
                .iter()
                .map(|w| w.project.path.clone())
                .collect();
            let merging = self.merging.clone();
            std::thread::spawn(move || {
                if let Ok(store) = Store::open(&db) {
                    for path in paths {
                        let _ = store.close_merged(&path);
                    }
                }
                merging.store(false, Ordering::Release);
            });
        }
        let every = |view: &IdeasView, secs| {
            view.dirty
                || view
                    .loaded
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(secs))
        };
        match self.active {
            Some(i) => {
                let view = &self.workspaces[i].ideas;
                let due = every(view, if view.open { 2 } else { 5 });
                if !due {
                    return;
                }
                let path = self.workspaces[i].project.path.clone();
                let open_count = self.db(|s| s.open_ideas(Some(&path))).unwrap_or(0);
                let items = self
                    .db(|s| s.list_ideas(Some(&path), true))
                    .unwrap_or_default();
                let comments = self
                    .db(|s| s.list_project_review_comments(Some(&path)))
                    .unwrap_or_default();
                let runs = self
                    .db(|s| s.list_review_runs(Some(&path)))
                    .unwrap_or_default();
                let ws = &mut self.workspaces[i];
                let view = &mut ws.ideas;
                view.comments = comments;
                view.runs = runs;
                let new_plan = view
                    .loaded
                    .is_some()
                    .then(|| {
                        items
                            .iter()
                            .find(|idea| {
                                !view.known_ids.contains(&idea.id)
                                    && idea.source != "usuario"
                                    && !idea.phases.is_empty()
                            })
                            .map(|idea| (idea.id, idea.title.clone()))
                    })
                    .flatten();
                if let Some((id, _)) = &new_plan {
                    view.open = true;
                    view.highlighted = Some(*id);
                    view.toggled.insert(*id, true);
                }
                view.known_ids = items.iter().map(|idea| idea.id).collect();
                view.items = items;
                (view.open_count, view.dirty, view.loaded) =
                    (open_count, false, Some(Instant::now()));
                // Un plan que acaba de crear un agente: Planes a la vista (como al abrirlo
                // a mano, sin otro panel encima) y un aviso.
                if let Some((id, title)) = new_plan {
                    ws.guard.open = false;
                    ws.memory.open = false;
                    ws.git.open = false;
                    ws.files.open = false;
                    ws.agent_event(
                        crate::workspace::Alert::Plan,
                        tr!("La IA creó el plan #{id}: {title}", id = id, title = title),
                    );
                }
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
            UiCmd::CommentSet(t, ..) | UiCmd::CommentDelete(t, _) => *t,
            UiCmd::LearnOpen(t, ..)
            | UiCmd::LearnSave(t)
            | UiCmd::LearnDiscard(t)
            | UiCmd::LearnClose(t) => *t,
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
            UiCmd::LearnOpen(_, plan, phase) => {
                let Some(idea) = view.items.iter().find(|i| i.id == plan) else {
                    return;
                };
                let Some(phase_id) = idea.phases.get(phase).map(|p| p.id.clone()) else {
                    return;
                };
                let items: Vec<LearnItem> = view
                    .comments
                    .iter()
                    .filter(|c| {
                        c.plan_id == plan
                            && c.phase_id == phase_id
                            && c.status == "resolved"
                            && !c.learned
                    })
                    .map(|c| LearnItem {
                        lesson: forge_core::ideas::lesson_from_comment(idea, phase, c),
                        save: false, // nada se guarda solo: la persona marca lo que vale
                        kind: "error",
                    })
                    .collect();
                if !items.is_empty() {
                    view.learning = Some(Learning {
                        plan,
                        phase_id,
                        items,
                    });
                }
            }
            UiCmd::LearnClose(_) => view.learning = None,
            UiCmd::LearnSave(_) | UiCmd::LearnDiscard(_) => {
                // Sin proyecto (la lista general) no hay memoria donde guardar: se conserva lo elegido.
                let Some(path) = path.as_deref() else { return };
                let Some(learning) = view.learning.take() else {
                    return;
                };
                let discard = matches!(cmd, UiCmd::LearnDiscard(_));
                let chosen: Vec<(forge_core::ideas::Lesson, &str)> = learning
                    .items
                    .iter()
                    .filter(|i| i.save && !discard)
                    .map(|i| (i.lesson.clone(), i.kind))
                    .collect();
                // Lo no marcado también se da por visto: no se vuelve a proponer.
                let skipped: Vec<i64> = learning
                    .items
                    .iter()
                    .filter(|i| discard || !i.save)
                    .map(|i| i.lesson.comment_id)
                    .collect();
                let path = path.to_path_buf();
                if self
                    .db(|s| s.save_review_lessons(&path, &chosen, &skipped))
                    .is_none()
                {
                    self.ideas_view(target).learning = Some(learning); // se conserva lo elegido
                }
            }
            UiCmd::CommentSet(_, id, resolved) => {
                // Desde el panel no hay explicación: quien resuelve con una es el agente.
                self.db(|s| s.resolve_review_comment(project, id, resolved, ""));
            }
            UiCmd::CommentDelete(_, id) => {
                self.db(|s| s.delete_review_comment(project, id));
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

    /// El agente que planifica o empieza un plan: el de la terminal enfocada si sabe (lee y
    /// escribe los planes por MCP); si no, el por defecto del proyecto. (id, nombre).
    pub(super) fn planner(&self, workspace: usize) -> Option<(String, String)> {
        let detected = self.agents.lock().ok()?.clone();
        let ws = self.workspaces.get(workspace)?;
        let in_use = ws.focused_agent();
        agents::planner_agent(&ws.project.agents, &detected, in_use.as_deref())
            .map(|a| (a.id.clone(), a.name.clone()))
    }

    /// Los agentes instalados y activos que pueden revisar una fase: (id, nombre, modo de
    /// solo lectura).
    pub(super) fn reviewers(&self, workspace: usize) -> Vec<(String, String, bool)> {
        let Ok(detected) = self.agents.lock().map(|d| d.clone()) else {
            return Vec::new();
        };
        let Some(ws) = self.workspaces.get(workspace) else {
            return Vec::new();
        };
        ws.project
            .agents
            .iter()
            .filter(|a| a.enabled && detected.get(a.program()).is_some_and(|d| d.path.is_some()))
            .filter(|a| a.with_prompt(&a.command, "").is_some())
            .map(|a| {
                (
                    a.id.clone(),
                    a.name.clone(),
                    a.read_only(&a.command).is_some(),
                )
            })
            .collect()
    }

    /// «Corregir»: el agente elegido recibe los comentarios abiertos de la fase (en su terminal
    /// si ya hay una abierta, o en una nueva) y los comprueba antes de arreglar nada.
    pub(super) fn fix_phase(
        &mut self,
        ctx: &egui::Context,
        id: i64,
        phase: usize,
        agent: &str,
        area: Rect,
    ) {
        let Some(i) = self.active else { return };
        let ws = &mut self.workspaces[i];
        let Some(prompt) = ws
            .ideas
            .items
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| forge_core::ideas::fix_prompt(p, phase, &ws.ideas.comments))
        else {
            return;
        };
        ws.ideas.open = false; // que se vea al agente trabajando
        ws.ask_agent(ctx, agent, &prompt, area);
    }

    /// «Revisar»: abre al agente elegido, de solo lectura, con la fase y su rama por revisar.
    pub(super) fn review_phase(
        &mut self,
        ctx: &egui::Context,
        id: i64,
        phase: usize,
        agent: &str,
        area: Rect,
    ) {
        let Some(i) = self.active else { return };
        let ws = &mut self.workspaces[i];
        let Some(prompt) = ws
            .ideas
            .items
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| forge_core::ideas::review_prompt(p, phase))
        else {
            return;
        };
        let path = ws.project.path.clone();
        ws.ideas.open = false; // que se vea al revisor trabajando
        ws.review_agent(ctx, agent, &prompt, area);
        // Queda anotado cuándo empieza: de ahí salen el tiempo y la estimación de la fase.
        self.db(|s| s.start_review_run(Some(&path), id, phase, agent));
        self.workspaces[i].ideas.dirty = true;
    }

    /// "Planificar con…": el agente por defecto recibe la petición de planificar la idea.
    pub(super) fn plan_with_agent(&mut self, ctx: &egui::Context, id: i64, area: Rect) {
        let Some(i) = self.active else { return };
        let Some((agent, _)) = self.planner(i) else {
            self.error = Some(
                tr!("Para planificar con IA hace falta Claude Code, Codex, OpenCode o Antigravity instalado")
                    .into(),
            );
            return;
        };
        let ws = &mut self.workspaces[i];
        let Some(idea) = ws.ideas.items.iter().find(|i| i.id == id) else {
            return;
        };
        // Con fases ya planificadas se empieza la primera; sin ellas, se planifica.
        let prompt = if idea.phases.is_empty() {
            tr!(
                "Planifica por fases la idea #{id} de los planes de Forge («{title}»). Revisa el proyecto lo necesario, propón las fases en orden (con rama solo si la merecen) y sus tareas, y guárdalo con plan_update (phases). No escribas código todavía.",
                id = id,
                title = idea.title
            )
        } else {
            tr!(
                "Empieza el plan #{id} de los planes de Forge («{title}»): lee sus fases con plans_list, marca la primera in_progress con plan_update (y su rama, si la tiene, creándola desde la rama principal) y trabaja en ella tachando las tareas al terminarlas.",
                id = id,
                title = idea.title
            )
        };
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
    /// Agentes que pueden revisar una fase: (id, nombre, con modo de solo lectura).
    reviewers: &'a [(String, String, bool)],
    /// Comentarios de revisión de todos los planes del proyecto.
    comments: &'a [forge_core::ideas::ReviewComment],
    /// Revisiones de fases del proyecto (para el avance de la que está en curso).
    runs: &'a [forge_core::ideas::ReviewRun],
    /// Lo que se propone recordar (lo edita una tarjeta mientras las demás solo leen).
    learning: &'a std::cell::RefCell<Option<Learning>>,
    highlighted: Option<i64>,
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
                let mut title = RichText::new(&idea.title)
                    .color(if ctx.highlighted == Some(idea.id) {
                        theme::ACCENT
                    } else {
                        theme::TEXT
                    })
                    .strong();
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
        // Planificado pero sin empezar (backlog): el agente arranca la fase 1.
        if let (Some(agent), "pending") = (ctx.planner, idea.status.as_str()) {
            let label = format!(
                "{}  {}",
                icon::PLAY,
                tr!("Empezar con {agent}", agent = agent)
            );
            let start = ui
                .add(
                    egui::Button::new(RichText::new(label).size(12.0).color(theme::ACCENT))
                        .frame(false),
                )
                .on_hover_text(tr!(
                    "El agente marca la primera fase en curso, crea su rama y se pone a trabajar en ella"
                ));
            if start.clicked() {
                cmds.push(UiCmd::PlanWithAgent(idea.id));
            }
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
    // Los comentarios de la revisión cuelgan del id de la fase, no de su número.
    let mine: Vec<&forge_core::ideas::ReviewComment> = if phase.id.is_empty() {
        Vec::new()
    } else {
        ctx.comments
            .iter()
            .filter(|c| c.plan_id == idea.id && c.phase_id == phase.id)
            .collect()
    };
    let open_comments = mine.iter().filter(|c| c.status == "open").count();
    // Resueltos que aún no se han propuesto para recordar, y si la lista de esta fase está abierta.
    let learnable = mine
        .iter()
        .filter(|c| c.status == "resolved" && !c.learned)
        .count();
    let learning_here = ctx
        .learning
        .borrow()
        .as_ref()
        .is_some_and(|l| l.plan == idea.id && l.phase_id == phase.id);
    // Las tareas de la fase en curso se ven (y la fase con comentarios sin resolver); las
    // demás, al pulsar la fase (y lo elegido a mano se respeta).
    let open = ctx
        .phase_toggled
        .get(&(idea.id, index))
        .copied()
        .unwrap_or(current || open_comments > 0 || learning_here);
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
            if let Some(line) = review_progress_line(idea, phase, &mine, ctx) {
                ui.label(RichText::new(line).size(11.5).color(theme::ACCENT));
                // Para que los minutos avancen aunque nada más cambie.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(5));
            }
            // Rama (clic: copiar) y tareas hechas, en una línea que se parte si no cabe.
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(branch) = &phase.branch {
                    branch_chip(ui, branch);
                }
                if phase.reviewable() && !ctx.reviewers.is_empty() {
                    review_button(ui, idea.id, index, ctx, cmds);
                }
                if open_comments > 0 && !ctx.reviewers.is_empty() {
                    fix_button(ui, idea.id, index, phase, ctx, cmds);
                }
                if learnable > 0 && !learning_here {
                    let learn = ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!(
                                    "{} {} ({learnable})",
                                    icon::LIGHTBULB,
                                    tr!("Aprender")
                                ))
                                .size(11.0)
                                .color(theme::ACCENT),
                            )
                            .frame(false),
                        )
                        .on_hover_text(tr!(
                            "Elige qué errores de esta revisión debe recordar el proyecto"
                        ));
                    if learn.clicked() {
                        cmds.push(UiCmd::LearnOpen(ctx.target, idea.id, index));
                    }
                }
                if !mine.is_empty() {
                    let color = if open_comments > 0 {
                        theme::YELLOW
                    } else {
                        theme::TEXT_3
                    };
                    let chip = ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!(
                                    "{} {open_comments}/{}",
                                    icon::CHAT_TEXT,
                                    mine.len()
                                ))
                                .size(11.0)
                                .color(color),
                            )
                            .frame(false),
                        )
                        .on_hover_text(tr!(
                            "Comentarios de la revisión: {open} sin resolver de {total}",
                            open = open_comments,
                            total = mine.len()
                        ));
                    if chip.clicked() {
                        toggles.phase = Some(((idea.id, index), !open));
                    }
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
                for comment in &mine {
                    review_comment_row(ui, comment, ctx.target, cmds);
                }
                if learning_here {
                    learn_box(ui, ctx.learning, ctx.target, cmds);
                }
            }
        });
    });
}

/// La lista de lo que se propone recordar de una revisión: cada propuesta se marca, se corrige
/// su título y se elige si es un error o una convención. Nada se guarda hasta pulsar «Guardar».
fn learn_box(
    ui: &mut egui::Ui,
    learning: &std::cell::RefCell<Option<Learning>>,
    target: IdeasTarget,
    cmds: &mut Vec<UiCmd>,
) {
    let mut guard = learning.borrow_mut();
    let Some(learning) = guard.as_mut() else {
        return;
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(
            RichText::new(tr!("Qué recordar de esta revisión"))
                .size(12.0)
                .strong()
                .color(theme::TEXT),
        );
        ui.label(
            RichText::new(tr!(
                "Marca lo que valga la pena que todos los agentes recuerden; lo demás se descarta."
            ))
            .size(11.0)
            .color(theme::TEXT_3),
        );
        for item in learning.items.iter_mut() {
            learn_row(ui, item);
        }
        let chosen = learning.items.iter().filter(|i| i.save).count();
        ui.horizontal(|ui| {
            let save = ui.add_enabled(
                chosen > 0,
                egui::Button::new(
                    RichText::new(tr!("Guardar lo marcado ({n})", n = chosen)).size(11.5),
                ),
            );
            if save.clicked() {
                cmds.push(UiCmd::LearnSave(target));
            }
            if ui
                .button(RichText::new(tr!("Descartar todo")).size(11.5))
                .clicked()
            {
                cmds.push(UiCmd::LearnDiscard(target));
            }
            if ui.button(RichText::new(tr!("Cerrar")).size(11.5)).clicked() {
                cmds.push(UiCmd::LearnClose(target));
            }
        });
    });
}

/// Una propuesta: casilla, título editable y tipo de memoria.
fn learn_row(ui: &mut egui::Ui, item: &mut LearnItem) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut item.save, "");
        ui.vertical(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut item.lesson.title)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Small),
            );
            egui::ComboBox::from_id_salt(("learn-kind", item.lesson.comment_id))
                .selected_text(
                    forge_core::memory::KINDS
                        .iter()
                        .find(|(id, _)| *id == item.kind)
                        .map_or(item.kind, |(_, label)| forge_core::i18n::t(label)),
                )
                .show_ui(ui, |ui| {
                    for (id, label) in forge_core::memory::KINDS
                        .iter()
                        .filter(|(id, _)| matches!(*id, "error" | "convention"))
                    {
                        ui.selectable_value(&mut item.kind, id, forge_core::i18n::t(label));
                    }
                });
        });
    });
}

/// Un comentario de la revisión: pendiente o resuelto (con cómo), con su archivo y quién lo
/// dejó; se puede resolver, reabrir o borrar.
fn review_comment_row(
    ui: &mut egui::Ui,
    comment: &forge_core::ideas::ReviewComment,
    target: IdeasTarget,
    cmds: &mut Vec<UiCmd>,
) {
    let resolved = comment.status == "resolved";
    ui.horizontal_top(|ui| {
        let (glyph, color, tip) = if resolved {
            (icon::CHECK_CIRCLE, theme::GREEN, tr!("Reabrir"))
        } else {
            (icon::CIRCLE, theme::YELLOW, tr!("Marcar como resuelto"))
        };
        if theme::icon_button_sized(ui, glyph, tip, 18.0, color).clicked() {
            cmds.push(UiCmd::CommentSet(target, comment.id, !resolved));
        }
        ui.vertical(|ui| {
            // Deja sitio al icono de borrar, que va a la derecha.
            ui.set_max_width((ui.available_width() - 26.0).max(80.0));
            ui.spacing_mut().item_spacing.y = 1.0;
            let mut text = RichText::new(&comment.text).size(12.0).color(if resolved {
                theme::TEXT_4
            } else {
                theme::TEXT_2
            });
            if resolved {
                text = text.strikethrough();
            }
            ui.add(egui::Label::new(text).wrap());
            let place = match (&comment.file, comment.line) {
                (Some(file), Some(line)) => format!("{file}:{line} · "),
                (Some(file), None) => format!("{file} · "),
                _ => String::new(),
            };
            ui.label(
                RichText::new(format!("{place}{}", comment.reviewer))
                    .size(10.5)
                    .color(theme::TEXT_3),
            );
            if !comment.resolution.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("→ {}", comment.resolution))
                            .size(11.0)
                            .color(theme::TEXT_3),
                    )
                    .wrap(),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if theme::icon_button_sized(
                ui,
                icon::TRASH,
                tr!("Borrar comentario"),
                18.0,
                theme::TEXT_3,
            )
            .clicked()
            {
                cmds.push(UiCmd::CommentDelete(target, comment.id));
            }
        });
    });
}

/// Cuánto lleva y cuánto falta de la revisión en curso de una fase: «Revisando con Codex · 3 min ·
/// 2 hallazgos · faltan ~4 min». `None` si no hay una en curso (o se quedó abierta hace horas).
fn review_progress_line(
    idea: &Idea,
    phase: &Phase,
    comments: &[&forge_core::ideas::ReviewComment],
    ctx: &ItemCtx,
) -> Option<String> {
    if phase.id.is_empty() {
        return None;
    }
    let now = forge_core::store::now();
    let run = ctx
        .runs
        .iter()
        .rev()
        .find(|r| r.plan_id == idea.id && r.phase_id == phase.id && r.ended_at.is_none())?;
    // Una revisión que nadie cerró hace horas ya no está en curso.
    if now - run.started_at > 2 * 3600 {
        return None;
    }
    let name = ctx
        .reviewers
        .iter()
        .find(|(id, _, _)| *id == run.agent)
        .map_or(run.agent.as_str(), |(_, name, _)| name.as_str());
    let minutes = |secs: i64| (secs + 59) / 60; // sube al minuto: «0 min» no dice nada
    let mut parts = vec![
        tr!("Revisando con {agent}", agent = name).to_string(),
        tr!("{m} min", m = minutes(now - run.started_at)),
    ];
    let found = comments
        .iter()
        .filter(|c| c.created_at >= run.started_at)
        .count();
    parts.push(tr!("{n} hallazgos", n = found));
    // Lo que tardaron las revisiones ya terminadas de este proyecto (las 10 últimas).
    let finished: Vec<i64> = ctx
        .runs
        .iter()
        .rev()
        .filter_map(|r| r.ended_at.map(|e| e - r.started_at))
        .filter(|d| *d > 0)
        .take(10)
        .collect();
    if let (Some(step), Some(total)) = (run.step, run.total)
        && step > 0
    {
        parts.push(tr!("paso {s} de {t}", s = step, t = total));
    }
    if let Some(left) = forge_core::ideas::review_eta(run, now, &finished) {
        parts.push(tr!("faltan ~{m} min", m = minutes(left)));
    }
    Some(format!("{} {}", icon::MAGNIFYING_GLASS, parts.join(" · ")))
}

/// «Corregir»: manda los comentarios abiertos al agente que elijas. El que hizo la fase va el
/// primero y marcado: es quien mejor sabe qué quiso hacer.
fn fix_button(
    ui: &mut egui::Ui,
    plan: i64,
    index: usize,
    phase: &Phase,
    ctx: &ItemCtx,
    cmds: &mut Vec<UiCmd>,
) {
    let button = ui
        .add(
            egui::Button::new(
                RichText::new(format!("{} {}", icon::WRENCH, tr!("Corregir")))
                    .size(11.0)
                    .color(theme::ACCENT),
            )
            .frame(false),
        )
        .on_hover_text(tr!(
            "Mandar los comentarios abiertos a un agente para que los compruebe y corrija"
        ));
    let author = phase
        .worked_by
        .as_deref()
        .and_then(forge_core::ideas::client_agent_id);
    egui::Popup::menu(&button).show(|ui| {
        ui.set_min_width(220.0);
        ui.label(
            RichText::new(tr!("Corregir con…"))
                .size(11.0)
                .color(theme::TEXT_3),
        );
        let mut agents: Vec<&(String, String, bool)> = ctx.reviewers.iter().collect();
        agents.sort_by_key(|(id, _, _)| Some(id.as_str()) != author);
        for (id, name, _) in agents {
            let label = if Some(id.as_str()) == author {
                format!("{name} · {}", tr!("hizo la fase"))
            } else {
                name.clone()
            };
            if ui.button(label).clicked() {
                cmds.push(UiCmd::FixPhase(plan, index, id.clone()));
                ui.close();
            }
        }
    });
}

/// «Revisar»: un menú con los agentes que pueden revisar la fase; el elegido abre su terminal.
fn review_button(ui: &mut egui::Ui, plan: i64, index: usize, ctx: &ItemCtx, cmds: &mut Vec<UiCmd>) {
    let button = ui.add(
        egui::Button::new(
            RichText::new(format!("{} {}", icon::MAGNIFYING_GLASS, tr!("Revisar")))
                .size(11.0)
                .color(theme::ACCENT),
        )
        .frame(false),
    );
    let button = button.on_hover_text(tr!("Pedirle a otro agente que revise esta fase"));
    egui::Popup::menu(&button).show(|ui| {
        ui.set_min_width(220.0);
        ui.label(
            RichText::new(tr!("Revisar con…"))
                .size(11.0)
                .color(theme::TEXT_3),
        );
        for (id, name, read_only) in ctx.reviewers {
            let item = ui.button(name.as_str());
            let item = if *read_only {
                item
            } else {
                item.on_hover_text(tr!(
                    "Este agente no tiene modo de solo lectura: solo el mensaje le pide no editar"
                ))
            };
            if item.clicked() {
                cmds.push(UiCmd::ReviewPhase(plan, index, id.clone()));
                ui.close();
            }
        }
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
