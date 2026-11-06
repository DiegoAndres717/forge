// Filas de agentes bajo cada proyecto en la barra lateral (plan #202): logo, estado (spinner,
// ✓, permiso, error), qué hace ahora, modelo y tiempo; un clic lleva a su terminal. Debajo,
// los subagentes desplegables de los agentes que los entregan, los archivos que cambió y si
// se pisa con otro. Reemplaza la sección aparte «Agentes en marcha». Solo texto: nada abre URLs.
use super::*;
use forge_core::activity::{self, Activity, Doing, Overlap};
use forge_core::live::{self, Row, State, TerminalInfo};
use forge_core::tr;

/// Cada cuánto se relee la actividad de un repositorio (una consulta a SQLite).
const REFRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct AgentRows {
    /// Raíz del repositorio de cada proyecto (los eventos de sus worktrees van con su clave).
    repos: HashMap<PathBuf, PathBuf>,
    /// Actividad leída de cada repositorio y cuándo.
    activity: HashMap<PathBuf, (Instant, Vec<Activity>)>,
    /// Filas con sus subagentes desplegados (por sesión de la terminal).
    pub(super) open_subs: HashSet<String>,
}

impl AgentRows {
    /// Las filas de las terminales de agente de `project` y los solapes entre ellas.
    pub(super) fn rows(
        &mut self,
        store: Option<&Store>,
        project: &Path,
        terminals: &[TerminalInfo],
        now: i64,
    ) -> (Vec<Row>, Vec<Overlap>) {
        if terminals.is_empty() {
            return Default::default();
        }
        let repo = self
            .repos
            .entry(project.to_path_buf())
            .or_insert_with(|| {
                forge_core::worktrees::main_root(project).unwrap_or_else(|_| project.to_path_buf())
            })
            .clone();
        let stale = self
            .activity
            .get(&repo)
            .is_none_or(|(t, _)| t.elapsed() >= REFRESH);
        if stale && let Some(store) = store {
            // Abiertas: las de esta ventana y las que otras registraron (para los solapes).
            let mut open = activity::terminals(store, &repo);
            open.extend(terminals.iter().map(|t| activity::Terminal {
                session: t.session.clone(),
                agent: t.agent.clone(),
            }));
            let rows = activity::alive(store.agent_activity(&repo, now), &open, now);
            self.activity.insert(repo.clone(), (Instant::now(), rows));
        }
        let acts = self
            .activity
            .get(&repo)
            .map(|(_, a)| a.as_slice())
            .unwrap_or_default();
        (
            live::rows(acts, terminals, now),
            activity::overlaps(acts, &[]),
        )
    }

    /// Lo leído se vuelve a leer ya (p. ej. al cambiar algo en las pruebas).
    #[cfg(test)]
    pub(super) fn refresh(&mut self) {
        self.activity.clear();
    }
}

fn doing(d: &Doing) -> String {
    match d {
        Doing::Editing(f) => tr!("editando {f}", f = f),
        Doing::Running(c) => tr!("corriendo {c}", c = c),
        Doing::Reading(f) => tr!("leyendo {f}", f = f),
        Doing::Tool(t) => tr!("usando {t}", t = t),
        Doing::Idle => tr!("esperando").to_string(),
    }
}

/// La línea de debajo: qué hace ahora, o cómo acabó.
fn status_line(row: &Row) -> Option<String> {
    let text = match row.state {
        State::Working => match (&row.doing, &row.title) {
            (Some(d), _) => doing(d),
            // Sin hooks: el título de su terminal, marcado como estimado.
            (None, Some(t)) if !row.reliable => format!("~ {t}"),
            (None, Some(t)) => t.clone(),
            (None, None) if !row.reliable => format!("~ {}", tr!("trabajando")),
            (None, None) => tr!("trabajando").to_string(),
        },
        State::Waiting => tr!("espera tu permiso o respuesta").to_string(),
        State::Done => match &row.result {
            Some(r) => tr!("✓ terminó: {r}", r = r),
            None => tr!("✓ terminó").to_string(),
        },
        State::Error => tr!("llegó a su límite de uso").to_string(),
        State::Idle => return row.title.clone().filter(|t| !t.is_empty()),
    };
    Some(text)
}

/// Spinner propio y lento (5 fotogramas por segundo): el de egui repinta sin parar.
fn spinner(ui: &egui::Ui, center: egui::Pos2, color: Color32) {
    let t = ui.input(|i| i.time) as f32;
    let start = t * 4.0;
    let points: Vec<egui::Pos2> = (0..=12)
        .map(|k| {
            let a = start + k as f32 * 0.35;
            center + 4.5 * Vec2::angled(a)
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(1.6, color)));
    ui.ctx().request_repaint_after(Duration::from_millis(200));
}

/// El indicador de estado a la derecha.
fn state_mark(ui: &egui::Ui, state: State, center: egui::Pos2) {
    let mark = |text: &str, color| {
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            text,
            FontId::proportional(12.0),
            color,
        );
    };
    match state {
        State::Working => spinner(ui, center, theme::ORANGE),
        State::Done => mark("✓", theme::GREEN),
        State::Waiting => mark("●", theme::ORANGE),
        State::Error => mark("✕", theme::RED),
        State::Idle => {}
    }
}

/// Texto en una línea recortado a `width` con «…»; dice si se recortó.
fn one_line(ui: &egui::Ui, text: &str, size: f32, color: Color32, width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat::simple(FontId::proportional(size), color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

/// Primera línea de una fila: icono, nombre y, a la derecha, modelo, tiempo y estado.
fn head(
    ui: &mut egui::Ui,
    indent: f32,
    icon: Option<(&str, &str)>,
    label: &str,
    detail: &str,
    state: State,
    unread: bool,
) -> egui::Response {
    let height = 22.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 6.0, theme::SELECTED);
    }
    let mid = rect.center().y;
    let mut x = rect.min.x + indent;
    if let Some((id, name)) = icon {
        let at = Rect::from_min_size(egui::pos2(x, mid - 7.0), Vec2::splat(14.0));
        theme::agent_icon(ui, id, name, at, false);
        if unread {
            ui.painter()
                .circle_filled(egui::pos2(at.max.x, at.min.y), 3.0, theme::ORANGE);
        }
        x += 20.0;
    }
    // Derecha: estado y detalle (modelo · tiempo).
    let right = rect.max.x - 8.0;
    state_mark(ui, state, egui::pos2(right - 5.0, mid));
    let mut text_end = right - 16.0;
    if !detail.is_empty() {
        let galley = ui.painter().layout_no_wrap(
            detail.to_string(),
            FontId::proportional(11.0),
            theme::TEXT_3,
        );
        let w = galley.size().x;
        ui.painter().galley(
            egui::pos2(text_end - w, mid - galley.size().y / 2.0),
            galley,
            theme::TEXT_3,
        );
        text_end -= w + 8.0;
    }
    let galley = one_line(ui, label, 12.5, theme::TEXT, text_end - x);
    let elided = galley.elided;
    ui.painter().galley(
        egui::pos2(x, mid - galley.size().y / 2.0),
        galley,
        theme::TEXT,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let response = if elided {
        response.on_hover_text(label)
    } else {
        response
    };
    response.on_hover_cursor(egui::CursorIcon::Default)
}

/// Línea pequeña bajo una fila, recortada (entera al pasar el ratón solo si se recortó).
fn small(ui: &mut egui::Ui, indent: f32, text: String, color: Color32) -> egui::Response {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).truncate())
    })
    .inner
}

/// Las filas de agentes de un proyecto, bajo su tarjeta. `panels`: la terminal de cada fila.
#[allow(clippy::too_many_arguments)]
pub(super) fn agent_rows_ui(
    ui: &mut egui::Ui,
    rows: &[Row],
    panels: &[PanelId],
    overlaps: &[Overlap],
    unread: &HashSet<PanelId>,
    open_subs: &mut HashSet<String>,
    now: i64,
    cmds: &mut Vec<UiCmd>,
) {
    // El tiempo de cada fila avanza solo (sin spinner no hay otro repintado).
    if !rows.is_empty() {
        ui.ctx().request_repaint_after(Duration::from_secs(10));
    }
    // Estrecho: sin el modelo (sigue en el aviso al pasar el ratón).
    let narrow = ui.available_width() < 190.0;
    for (row, panel) in rows.iter().zip(panels) {
        let time = live::elapsed(now - row.since);
        let detail = match (&row.model, narrow) {
            (Some(m), false) => format!("{m} · {time}"),
            _ => time,
        };
        let response = head(
            ui,
            12.0,
            Some((&row.agent_id, &row.agent)),
            &row.agent,
            &detail,
            row.state,
            unread.contains(panel),
        );
        // El detalle de la fila: dónde trabaja, con qué modelo y qué tan fiable es el estado.
        let mut tip: Vec<String> = Vec::new();
        if let Some(m) = &row.model {
            tip.push(tr!("Modelo: {m}", m = m));
        }
        if let Some(root) = &row.root {
            tip.push(tilde(root));
        }
        if !row.reliable {
            tip.push(
                tr!("Estado estimado por la salida de su terminal: sin datos de los hooks del agente").to_string(),
            );
        }
        let response = if tip.is_empty() {
            response
        } else {
            response.on_hover_text(tip.join("\n"))
        };
        if response.clicked() {
            cmds.push(UiCmd::GoToPanel(*panel));
        }
        let is_unread = unread.contains(panel);
        response.context_menu(|ui| {
            let label = if is_unread {
                tr!("Marcar como leído")
            } else {
                tr!("Marcar como no leído")
            };
            if ui.button(label).clicked() {
                cmds.push(UiCmd::ToggleUnread(*panel));
                ui.close();
            }
        });
        let color = match row.state {
            State::Waiting => theme::ORANGE,
            State::Error => theme::RED,
            State::Working => theme::TEXT_2,
            _ => theme::TEXT_3,
        };
        if let Some(text) = status_line(row) {
            small(ui, 46.0, text, color);
        }
        if !row.files.is_empty() {
            let text = match row.files.len() {
                1 => tr!("1 archivo cambiado").to_string(),
                n => tr!("{n} archivos cambiados", n = n),
            };
            small(ui, 46.0, text, theme::TEXT_3).on_hover_text(row.files.join("\n"));
        }
        for o in overlaps.iter().filter(|o| row.files.contains(&o.file)) {
            small(
                ui,
                46.0,
                tr!(
                    "⚠ se pisa en {f}: {who}",
                    f = o.file,
                    who = o.who.join(", ")
                ),
                theme::RED,
            );
        }
        // Subagentes: solo de los agentes que los entregan (nunca inventados), desplegables.
        if row.subagents.is_empty() {
            continue;
        }
        let open = open_subs.contains(&row.session);
        let working = row
            .subagents
            .iter()
            .filter(|s| s.state == State::Working)
            .count();
        let caret = if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        };
        let text = match (row.subagents.len(), working) {
            (1, 0) => tr!("{c} 1 subagente", c = caret),
            (n, 0) => tr!("{c} {n} subagentes", c = caret, n = n),
            (n, w) => tr!(
                "{c} {n} subagentes ({w} trabajando)",
                c = caret,
                n = n,
                w = w
            ),
        };
        let toggle = small(ui, 46.0, text, theme::TEXT_3).interact(Sense::click());
        if toggle.clicked() {
            if open {
                open_subs.remove(&row.session);
            } else {
                open_subs.insert(row.session.clone());
            }
        }
        if open {
            for sub in &row.subagents {
                let r = head(
                    ui,
                    46.0,
                    None,
                    &sub.name,
                    &live::elapsed(now - sub.since),
                    sub.state,
                    false,
                );
                if r.clicked() {
                    cmds.push(UiCmd::GoToPanel(*panel));
                }
                let line = match (sub.state, &sub.doing, &sub.result) {
                    (State::Working, Some(d), _) => Some(doing(d)),
                    (State::Done, _, Some(r)) => Some(tr!("✓ terminó: {r}", r = r)),
                    _ => None,
                };
                if let Some(line) = line {
                    small(ui, 58.0, line, theme::TEXT_3);
                }
            }
        }
    }
}
