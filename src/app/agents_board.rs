// «Agentes en marcha» en la barra lateral: qué hace ahora cada agente y subagente del
// proyecto (por los hooks de cada agente), cuánto lleva, qué archivos cambió y si se pisa con
// otro. Al terminar queda un rato con su resultado. Solo texto: nada abre URLs.
use super::*;
use forge_core::activity::{self, Activity, Doing, Live, Terminal};
use forge_core::tr;

#[derive(Default)]
pub(super) struct AgentsBoard {
    pub(super) open: bool,
    project: PathBuf,
    /// Raíz del repositorio (los eventos de todos sus worktrees van con su clave).
    repo: Option<PathBuf>,
    rows: Vec<Activity>,
    terminals: Vec<Terminal>,
    pub(super) checked: Option<Instant>,
}

/// «3 min», «1 h 5 min».
fn took(secs: i64) -> String {
    match secs.max(0) {
        s @ ..60 => tr!("{s} s", s = s),
        s @ 60..3600 => tr!("{m} min", m = s / 60),
        s => tr!("{h} h {m} min", h = s / 3600, m = s % 3600 / 60),
    }
}

/// `text` en `max` caracteres como mucho.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!("{}…", text.chars().take(max - 1).collect::<String>())
}

fn doing(a: &Activity) -> String {
    match a.doing() {
        Doing::Editing(f) => tr!("editando {f}", f = f),
        Doing::Running(c) => tr!("corriendo {c}", c = c),
        Doing::Reading(f) => tr!("leyendo {f}", f = f),
        Doing::Tool(t) => tr!("usando {t}", t = t),
        Doing::Idle => tr!("esperando").to_string(),
    }
}

impl App {
    /// Fila «Agentes en marcha» y, abierta, una fila por agente (debajo de Worktrees).
    pub(super) fn agents_board_ui(&mut self, ui: &mut egui::Ui, active: usize) {
        let project = self.workspaces[active].project.path.clone();
        let board = &mut self.agents_board;
        if board.project != project {
            *board = AgentsBoard {
                project: project.clone(),
                open: board.open,
                ..Default::default()
            };
        }
        // Una lectura de la base (lo pesado, Git, es de `work_status`): cada 2 s abierta, cada
        // 10 s cerrada (solo para la cifra).
        let every = Duration::from_secs(if board.open { 2 } else { 10 });
        if let Some(store) = &self.store
            && board.checked.is_none_or(|t| t.elapsed() >= every)
        {
            board.checked = Some(Instant::now());
            let repo = board
                .repo
                .get_or_insert_with(|| {
                    forge_core::worktrees::main_root(&project).unwrap_or_else(|_| project.clone())
                })
                .clone();
            let now = forge_core::store::now();
            board.terminals = activity::terminals(store, &repo);
            board.rows = activity::alive(store.agent_activity(&repo, now), &board.terminals, now);
        }
        if board.open {
            ui.ctx().request_repaint_after(every);
        }
        let running = board
            .rows
            .iter()
            .filter(|a| a.finished_at.is_none())
            .count();
        let count = if running > 0 {
            running.to_string()
        } else {
            String::new()
        };
        let row = theme::row(
            ui,
            icon::ROBOT,
            theme::TEXT_3,
            tr!("Agentes en marcha"),
            &count,
            board.open,
        )
        .on_hover_text(tr!(
            "Qué hace ahora cada agente y subagente del proyecto, en todos sus worktrees, según los hooks de cada agente"
        ));
        if row.clicked() {
            board.open = !board.open;
            // Al abrir, los datos de ahora (no los de hace 10 s).
            board.checked = None;
        }
        if !board.open {
            return;
        }
        let small = |ui: &mut egui::Ui, text: String, color| {
            ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).wrap())
        };
        let overlaps = activity::overlaps(&board.rows, &[]);
        let now = forge_core::store::now();
        for a in &board.rows {
            let mine: Vec<&activity::Overlap> = overlaps
                .iter()
                .filter(|o| a.files.contains(&o.file))
                .collect();
            let dot = match (a.finished_at, mine.is_empty()) {
                (_, false) => theme::RED,
                (Some(_), _) => theme::GREEN,
                (None, _) => theme::ORANGE,
            };
            let time = took(a.finished_at.unwrap_or(now) - a.started_at);
            let hover = format!(
                "{}\n{}",
                a.label(),
                a.root.as_deref().map(tilde).unwrap_or_default()
            );
            // Un subagente va justo debajo de su terminal: basta su tarea, recortada para que
            // no pise el tiempo (entera al pasar el ratón).
            let name = match (&a.name, a.agent_id.is_empty()) {
                (Some(task), false) => short(task, 28),
                (None, false) => tr!("subagente").to_string(),
                _ => a.agent.clone(),
            };
            theme::sub_row(ui, dot, &name, &time).on_hover_text(hover);
            match (a.finished_at, &a.result) {
                (Some(_), Some(r)) => small(ui, tr!("✓ terminó: {r}", r = r), theme::TEXT_3),
                (Some(_), None) => small(ui, tr!("✓ terminó").to_string(), theme::TEXT_3),
                (None, _) => small(ui, doing(a), theme::TEXT_2),
            };
            if !a.files.is_empty() {
                let text = match a.files.len() {
                    1 => tr!("1 archivo cambiado").to_string(),
                    n => tr!("{n} archivos cambiados", n = n),
                };
                small(ui, text, theme::TEXT_3).on_hover_text(a.files.join("\n"));
            }
            for o in mine {
                small(
                    ui,
                    tr!(
                        "⚠ se pisa en {f}: {who}",
                        f = o.file,
                        who = o.who.join(", ")
                    ),
                    theme::RED,
                );
            }
        }
        // Terminales de agente sin eventos: se dice por qué, en vez de inventar.
        for t in &board.terminals {
            if board.rows.iter().any(|a| a.session == t.session) {
                continue;
            }
            theme::sub_row(ui, theme::TEXT_4, &t.agent, "");
            let why = match activity::live_data(&t.agent) {
                Live::None => tr!("sin datos en vivo para este agente"),
                _ => tr!("aún sin actividad"),
            };
            small(ui, why.to_string(), theme::TEXT_4);
        }
        if board.rows.is_empty() && board.terminals.is_empty() {
            small(
                ui,
                tr!("Ningún agente en marcha").to_string(),
                theme::TEXT_4,
            );
        }
    }
}
