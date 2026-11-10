// Filas de agentes bajo cada proyecto en la barra lateral (plan #202), compactas: una línea
// por agente con su estado (spinner, ✓, permiso, error), su logo, qué hace ahora y, a la
// derecha, modelo y tiempo; un clic en cualquier parte lleva a su terminal. Debajo, solo si
// hace falta: si se pisa con otro y sus subagentes desplegables (uno por línea). Lo demás
// (nombre, archivos cambiados, dónde trabaja) va en el aviso. Solo texto: nada abre URLs.
use super::*;
use forge_core::activity::{self, Activity, Doing, Overlap};
use forge_core::live::{self, Row, State, TerminalInfo};
use forge_core::tr;

/// Cada cuánto se relee la actividad de un repositorio (una consulta a SQLite).
const REFRESH: Duration = Duration::from_secs(2);
/// Columnas de una fila: sangría, marca de estado y logo; el texto empieza en `LABEL_X`.
const ROW_INDENT: f32 = 12.0;
const MARK_W: f32 = 16.0;
const ICON_W: f32 = 20.0;
/// Donde empieza el texto de una fila; las líneas de debajo se alinean con él.
const LABEL_X: f32 = ROW_INDENT + MARK_W + ICON_W;

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

/// El texto de la fila: qué hace ahora o cómo acabó, con su color; `None` si no hay nada
/// mejor que el nombre del agente.
fn status(row: &Row) -> Option<(String, Color32)> {
    let text = match row.state {
        State::Working => match (&row.doing, &row.title) {
            (Some(d), _) => doing(d),
            // Sin hooks: el título de su terminal, marcado como estimado.
            (None, Some(t)) if !row.reliable => format!("~ {t}"),
            (None, Some(t)) => t.clone(),
            (None, None) if !row.reliable => format!("~ {}", tr!("trabajando")),
            (None, None) => tr!("trabajando").to_string(),
        },
        State::Waiting => {
            return Some((tr!("espera tu permiso o respuesta").into(), theme::ORANGE));
        }
        State::Error => return Some((tr!("llegó a su límite de uso").into(), theme::RED)),
        // El ✓ ya va en la marca: el texto es solo el resultado.
        State::Done => row.result.clone().unwrap_or_else(|| tr!("terminó").into()),
        State::Idle => row.title.clone().filter(|t| !t.is_empty())?,
    };
    Some((text, theme::TEXT))
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

/// El indicador de estado, a la izquierda de la fila.
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
        State::Idle => mark("·", theme::TEXT_3),
    }
}

/// Texto en una línea recortado a `width` con «…» (`elided` dice si se recortó).
fn one_line(ui: &egui::Ui, text: &str, size: f32, color: Color32, width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat::simple(FontId::proportional(size), color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

/// Reserva el fondo de un bloque antes de pintarlo; `highlight` lo rellena luego si hay ratón.
fn background(ui: &egui::Ui) -> egui::layers::ShapeIdx {
    ui.painter().add(egui::Shape::Noop)
}

/// Resalta el bloque entero mientras el ratón esté dentro (también entre sus líneas).
fn highlight(ui: &egui::Ui, at: egui::layers::ShapeIdx, block: &egui::Response) {
    if ui.rect_contains_pointer(block.rect) {
        ui.painter().set(
            at,
            egui::Shape::rect_filled(block.rect, 6.0, theme::SELECTED),
        );
    }
}

/// Una fila en una línea: [estado] [logo] texto … detalle (modelo · tiempo). Todo el ancho
/// es pulsable; `name` es su nombre accesible y `tip` su aviso (si no hay, el texto entero
/// cuando se recorta: un solo aviso por fila).
#[allow(clippy::too_many_arguments)]
fn head(
    ui: &mut egui::Ui,
    indent: f32,
    icon: Option<(&str, &str)>,
    label: &str,
    color: Color32,
    detail: &str,
    state: State,
    unread: bool,
    name: &str,
    tip: Option<String>,
) -> egui::Response {
    let height = 22.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let mid = rect.center().y;
    let mut x = rect.min.x + indent;
    state_mark(ui, state, egui::pos2(x + MARK_W / 2.0 - 2.0, mid));
    x += MARK_W;
    if let Some((id, agent)) = icon {
        let at = Rect::from_min_size(egui::pos2(x, mid - 7.0), Vec2::splat(14.0));
        theme::agent_icon(ui, id, agent, at, false);
        if unread {
            ui.painter()
                .circle_filled(egui::pos2(at.max.x, at.min.y), 3.0, theme::ORANGE);
        }
        x += ICON_W;
    }
    // Derecha: modelo · tiempo, en monoespaciada pequeña.
    let mut text_end = rect.max.x - 8.0;
    if !detail.is_empty() {
        let galley =
            ui.painter()
                .layout_no_wrap(detail.to_string(), FontId::monospace(10.5), theme::TEXT_3);
        let w = galley.size().x;
        ui.painter().galley(
            egui::pos2(text_end - w, mid - galley.size().y / 2.0),
            galley,
            theme::TEXT_3,
        );
        text_end -= w + 8.0;
    }
    let galley = one_line(ui, label, 12.5, color, text_end - x);
    let elided = galley.elided;
    ui.painter()
        .galley(egui::pos2(x, mid - galley.size().y / 2.0), galley, color);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    let response = match tip {
        Some(tip) => response.on_hover_text(tip),
        None if elided => response.on_hover_text(label),
        None => response,
    };
    response.on_hover_cursor(egui::CursorIcon::Default)
}

/// Línea pequeña bajo una fila, empezando en `indent`, pulsable en todo su ancho y recortada
/// (entera al pasar el ratón solo si se recortó).
fn small(
    ui: &mut egui::Ui,
    indent: f32,
    text: &str,
    color: Color32,
    kind: egui::WidgetType,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::click());
    let x = rect.min.x + indent;
    let galley = one_line(ui, text, 11.0, color, rect.max.x - 8.0 - x);
    let elided = galley.elided;
    ui.painter().galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(kind, true, text));
    if elided {
        response.on_hover_text(text)
    } else {
        response
    }
}

/// Las filas de agentes de un proyecto, bajo su tarjeta. `panels`: la terminal de cada fila.
/// `switch`: el proyecto en el que entrar antes de ir a la terminal, si no es el activo.
#[allow(clippy::too_many_arguments)]
pub(super) fn agent_rows_ui(
    ui: &mut egui::Ui,
    switch: Option<usize>,
    rows: &[Row],
    panels: &[PanelId],
    overlaps: &[Overlap],
    unread: &HashSet<PanelId>,
    open_subs: &mut HashSet<String>,
    now: i64,
    cmds: &mut Vec<UiCmd>,
) {
    // Ir a la terminal de una fila: si es de otro proyecto, primero se entra en él (como un
    // clic en su tarjeta), porque «ir al panel» actúa sobre el proyecto activo.
    let go = |cmds: &mut Vec<UiCmd>, panel: PanelId| {
        if let Some(i) = switch {
            cmds.push(UiCmd::Activate(i));
        }
        cmds.push(UiCmd::GoToPanel(panel));
    };
    // El tiempo de cada fila avanza solo (sin spinner no hay otro repintado).
    if !rows.is_empty() {
        ui.ctx().request_repaint_after(Duration::from_secs(10));
    }
    // Estrecho: sin el modelo (sigue en el aviso al pasar el ratón).
    let narrow = ui.available_width() < 240.0;
    for (row, panel) in rows.iter().zip(panels) {
        let time = live::elapsed(now - row.since);
        let detail = match (&row.model, narrow) {
            (Some(m), false) => format!("{m} · {time}"),
            _ => time,
        };
        let (label, color) = status(row).unwrap_or((row.agent.clone(), theme::TEXT));
        let name = if label == row.agent {
            row.agent.clone()
        } else {
            format!("{} · {label}", row.agent)
        };
        // Todo el bloque (la fila y sus solapes) lleva a su terminal; el desplegable de
        // subagentes tiene su propia acción y queda fuera.
        // El aviso: quién es, qué hace entero, dónde trabaja, qué cambió y qué tan fiable es.
        let mut tip: Vec<String> = vec![row.agent.clone()];
        if label != row.agent {
            tip.push(label.clone());
        }
        if let Some(m) = &row.model {
            tip.push(tr!("Modelo: {m}", m = m));
        }
        if let Some(root) = &row.root {
            tip.push(tilde(root));
        }
        if !row.files.is_empty() {
            tip.push(match row.files.len() {
                1 => tr!("1 archivo cambiado").to_string(),
                n => tr!("{n} archivos cambiados", n = n),
            });
            tip.extend(row.files.iter().map(|f| format!("  {f}")));
        }
        if !row.reliable {
            tip.push(
                tr!("Estado estimado por la salida de su terminal: sin datos de los hooks del agente").to_string(),
            );
        }
        let bg = background(ui);
        let response = head(
            ui,
            ROW_INDENT,
            Some((&row.agent_id, &row.agent)),
            &label,
            color,
            &detail,
            row.state,
            unread.contains(panel),
            &name,
            Some(tip.join("\n")),
        );
        let mut block = response;
        // Que se pisa con otro sí se ve sin pasar el ratón.
        for o in overlaps.iter().filter(|o| row.files.contains(&o.file)) {
            let text = tr!(
                "⚠ se pisa en {f}: {who}",
                f = o.file,
                who = o.who.join(", ")
            );
            block |= small(ui, LABEL_X, &text, theme::RED, egui::WidgetType::Label);
        }
        highlight(ui, bg, &block);
        if block.clicked() {
            go(cmds, *panel);
        }
        let is_unread = unread.contains(panel);
        block.context_menu(|ui| {
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
        let bg = background(ui);
        let toggle = small(ui, LABEL_X, &text, theme::TEXT_3, egui::WidgetType::Button);
        highlight(ui, bg, &toggle);
        if toggle.clicked() {
            if open {
                open_subs.remove(&row.session);
            } else {
                open_subs.insert(row.session.clone());
            }
        }
        if !open {
            continue;
        }
        // Un subagente por línea: [estado] nombre … tiempo; lo que hace, en el aviso. Lleva a
        // la terminal de su agente.
        for sub in &row.subagents {
            let line = match (sub.state, &sub.doing, &sub.result) {
                (State::Working, Some(d), _) => Some(doing(d)),
                (State::Done, _, Some(r)) => Some(tr!("✓ terminó: {r}", r = r)),
                _ => None,
            };
            let bg = background(ui);
            let r = head(
                ui,
                LABEL_X - MARK_W,
                None,
                &sub.name,
                theme::TEXT_2,
                &live::elapsed(now - sub.since),
                sub.state,
                false,
                &sub.name,
                line.map(|l| format!("{}\n{l}", sub.name)),
            );
            highlight(ui, bg, &r);
            if r.clicked() {
                go(cmds, *panel);
            }
        }
    }
}
