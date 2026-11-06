// Estado en vivo de cada terminal de agente (plan #202, fase 2): trabajando, esperando
// permiso, terminado, error o en espera; desde cuándo, qué hace, con qué modelo y sus
// subagentes. Sale de `agent_activity` (hooks) y, donde el agente no da datos, de la salida y
// el título de su terminal, marcado como menos fiable. La barra lateral pinta estas filas.
use std::path::PathBuf;

use crate::activity::{Activity, Doing};
use crate::catalog::{self, Live};

/// Cuánto se ve «terminado» (✓) antes de pasar a «en espera».
pub const DONE_SHOWN: i64 = 10 * 60;
/// Sin datos de hooks: si su terminal escribió hace menos que esto, está trabajando.
const QUIET_AFTER: i64 = 4;
/// Con hooks: escribir menos que esto seguido es redibujar (una tecla, cambiar el tamaño).
const BUSY_FOR: i64 = 2;
/// Sin eventos ni salida en este rato, lo que seguía «trabajando» se interrumpió (Esc, Ctrl+C
/// no avisan): pasa a en espera.
const STALE_AFTER: i64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Working,
    /// Pidió permiso o una respuesta.
    Waiting,
    /// Terminó hace poco (se ve con ✓ un rato).
    Done,
    /// Falló o llegó a su límite de uso.
    Error,
    /// Abierto y quieto (terminó hace rato o aún no hizo nada).
    Idle,
}

/// Lo que la app sabe de una terminal de agente abierta.
#[derive(Debug, Clone, Default)]
pub struct TerminalInfo {
    /// `FORGE_SESSION_ID` de la terminal.
    pub session: String,
    /// Id del agente (`claude`, `codex`… o uno propio).
    pub agent_id: String,
    /// Su nombre visible («Claude Code»).
    pub agent: String,
    /// Modelo con que se abrió, si se sabe.
    pub model: Option<String>,
    /// Título que puso el agente en su terminal (OSC).
    pub title: Option<String>,
    /// Último aviso del agente para esta terminal (`waiting`, `stop`, `limit`) y cuándo.
    pub event: Option<(String, i64)>,
    /// Cuándo escribió por última vez, y desde cuándo escribe sin parar (sin hooks).
    pub output_at: Option<i64>,
    pub busy_since: Option<i64>,
    pub opened_at: i64,
}

/// Un subagente bajo la fila de su agente.
#[derive(Debug, Clone, PartialEq)]
pub struct Sub {
    pub name: String,
    pub state: State,
    pub since: i64,
    pub doing: Option<Doing>,
    pub result: Option<String>,
}

/// Una fila de agente en la barra lateral.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub session: String,
    pub agent_id: String,
    pub agent: String,
    pub state: State,
    /// Desde cuándo está en ese estado (segundos Unix).
    pub since: i64,
    pub doing: Option<Doing>,
    /// Sin hooks: lo que dice el título de su terminal.
    pub title: Option<String>,
    /// `false`: el estado sale de la salida de la terminal, no de los hooks del agente.
    pub reliable: bool,
    pub model: Option<String>,
    pub root: Option<PathBuf>,
    pub files: Vec<String>,
    pub result: Option<String>,
    /// Solo para agentes que los entregan (`AgentInfo::subagents`); nunca inventados.
    pub subagents: Vec<Sub>,
}

fn sub_state(a: &Activity, now: i64) -> (State, i64) {
    match a.finished_at {
        None => (State::Working, a.started_at),
        Some(end) if now - end < DONE_SHOWN => (State::Done, end),
        Some(end) => (State::Idle, end),
    }
}

/// Una fila por terminal de agente abierta, en el orden de `terminals`.
pub fn rows(activity: &[Activity], terminals: &[TerminalInfo], now: i64) -> Vec<Row> {
    terminals.iter().map(|t| row(activity, t, now)).collect()
}

fn row(activity: &[Activity], t: &TerminalInfo, now: i64) -> Row {
    let mine = activity.iter().filter(|a| a.session == t.session);
    let main = mine.clone().find(|a| a.agent_id.is_empty());
    let info = catalog::get(&t.agent_id).or_else(|| catalog::by_name(&t.agent));
    let live = info.map_or(Live::None, |i| i.live);
    let subagents: Vec<Sub> = if info.is_some_and(|i| i.subagents) {
        mine.filter(|a| !a.agent_id.is_empty())
            .map(|a| {
                let (state, since) = sub_state(a, now);
                Sub {
                    name: a.name.clone().unwrap_or_else(|| "subagent".into()),
                    state,
                    since,
                    doing: (state == State::Working).then(|| a.doing()),
                    result: a.result.clone(),
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut row = Row {
        session: t.session.clone(),
        agent_id: t.agent_id.clone(),
        agent: t.agent.clone(),
        state: State::Idle,
        since: t.opened_at,
        doing: None,
        title: t.title.clone(),
        reliable: false,
        model: t.model.clone(),
        root: main.and_then(|m| m.root.clone()),
        files: main.map(|m| m.files.clone()).unwrap_or_default(),
        result: main.and_then(|m| m.result.clone()),
        subagents,
    };
    // Lo que el agente dijo de sí mismo después de su última actividad manda.
    let last_activity = main.map_or(0, |m| m.updated_at);
    match &t.event {
        Some((kind, at)) if kind == "limit" && *at >= last_activity => {
            row.state = State::Error;
            row.since = *at;
            row.reliable = true;
            return row;
        }
        // El aviso de «esperando» que llega tras terminar es el de la pantalla de entrada, no
        // un permiso: solo cuenta con el turno abierto.
        Some((kind, at))
            if kind == "waiting"
                && *at >= last_activity
                && main.is_none_or(|m| m.finished_at.is_none()) =>
        {
            row.state = State::Waiting;
            row.since = *at;
            row.reliable = true;
            return row;
        }
        _ => {}
    }
    if let Some(m) = main.filter(|_| live != Live::None) {
        row.reliable = true;
        let (state, since) = sub_state(m, now);
        row.state = state;
        row.since = since;
        let quiet = t.output_at.is_none_or(|o| now - o >= STALE_AFTER);
        if state == State::Working && now - m.updated_at >= STALE_AFTER && quiet {
            row.state = State::Idle;
            row.since = m.updated_at;
        } else if state == State::Working {
            row.doing = Some(m.doing());
        }
    } else if let Some(sub) = row.subagents.first().filter(|_| live != Live::None) {
        row.reliable = true;
        row.since = sub.since;
    } else if let Some((_, at)) = t.event.as_ref().filter(|(k, _)| k == "stop") {
        // Avisó que terminó (hook de fin) aunque no mande actividad.
        row.reliable = true;
        let (state, since) = if now - at < DONE_SHOWN {
            (State::Done, *at)
        } else {
            (State::Idle, *at)
        };
        row.state = state;
        row.since = since;
        // Con hooks, su aviso de fin manda; la salida solo cuenta para los que no los tienen.
        if live == Live::None
            && t.output_at
                .is_some_and(|o| o > *at && now - o < QUIET_AFTER)
        {
            row.state = State::Working;
            row.since = t.busy_since.unwrap_or(*at);
            row.reliable = false;
        }
    } else {
        // Respaldo: su terminal escribe ahora mismo → trabajando (estimado). Quieto, un agente
        // con hooks que no dijo nada está de verdad en espera.
        row.reliable = live != Live::None;
        let sustained = live == Live::None || t.busy_since.is_some_and(|b| now - b >= BUSY_FOR);
        match t.output_at {
            Some(o) if now - o < QUIET_AFTER && sustained => {
                row.state = State::Working;
                row.since = t.busy_since.unwrap_or(o);
                row.reliable = false;
            }
            Some(o) => {
                row.state = State::Idle;
                row.since = o;
            }
            None => {}
        }
    }
    // Un subagente que sigue trabajando mantiene al padre trabajando.
    if row.state != State::Working
        && let Some(sub) = row
            .subagents
            .iter()
            .filter(|s| s.state == State::Working)
            .min_by_key(|s| s.since)
        && matches!(row.state, State::Done | State::Idle)
    {
        row.state = State::Working;
        row.since = sub.since;
    }
    row
}

/// Cuánto lleva, corto: «ahora», «49m», «3h», «2d».
pub fn elapsed(secs: i64) -> String {
    match secs.max(0) {
        ..60 => crate::tr!("ahora").to_string(),
        s @ 60..3600 => format!("{}m", s / 60),
        s @ 3600..86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(session: &str, agent_id: &str, started: i64, finished: Option<i64>) -> Activity {
        Activity {
            session: session.into(),
            agent_id: agent_id.into(),
            agent: "Claude Code".into(),
            name: (!agent_id.is_empty()).then(|| format!("{agent_id}: task")),
            tool: Some("Bash".into()),
            target: Some("cargo test".into()),
            started_at: started,
            updated_at: finished.unwrap_or(started),
            finished_at: finished,
            ..Default::default()
        }
    }

    fn term(session: &str, id: &str, name: &str) -> TerminalInfo {
        TerminalInfo {
            session: session.into(),
            agent_id: id.into(),
            agent: name.into(),
            model: Some("opus".into()),
            opened_at: 0,
            ..Default::default()
        }
    }

    #[test]
    fn each_state_comes_from_the_hooks_with_its_start_time() {
        let now = 10_000;
        let acts = [
            act("w", "", now - 120, None),
            act("d", "", now - 900, Some(now - 60)),
            act("old", "", now - 9000, Some(now - DONE_SHOWN - 1)),
        ];
        let mut waiting = term("p", "claude", "Claude Code");
        waiting.event = Some(("waiting".into(), now - 5));
        let mut limit = term("l", "codex", "Codex");
        limit.event = Some(("limit".into(), now - 7));
        let terms = [
            term("w", "claude", "Claude Code"),
            term("d", "claude", "Claude Code"),
            term("old", "claude", "Claude Code"),
            waiting,
            limit,
        ];
        let rows = rows(&acts, &terms, now);
        let got: Vec<(State, i64)> = rows.iter().map(|r| (r.state, r.since)).collect();
        assert_eq!(
            got,
            [
                (State::Working, now - 120),
                (State::Done, now - 60),
                (State::Idle, now - DONE_SHOWN - 1),
                (State::Waiting, now - 5),
                (State::Error, now - 7),
            ]
        );
        assert_eq!(rows[0].doing, Some(Doing::Running("cargo test".into())));
        assert_eq!(rows[0].model.as_deref(), Some("opus"));
        assert!(rows.iter().all(|r| r.reliable));
    }

    #[test]
    fn new_activity_after_a_permission_request_means_it_works_again() {
        let now = 10_000;
        let mut t = term("s", "claude", "Claude Code");
        t.event = Some(("waiting".into(), now - 30));
        let mut a = act("s", "", now - 100, None);
        a.updated_at = now - 2;
        assert_eq!(rows(&[a], &[t], now)[0].state, State::Working);
    }

    #[test]
    fn without_hooks_the_terminal_output_decides_and_is_marked_less_reliable() {
        let now = 10_000;
        let mut busy = term("o", "opencode", "OpenCode");
        busy.output_at = Some(now - 1);
        busy.busy_since = Some(now - 300);
        busy.title = Some("Fixing the login".into());
        let mut quiet = term("q", "qwen", "Qwen Code");
        quiet.output_at = Some(now - 60);
        // OpenCode no tiene hooks: aunque hubiera actividad, no cuenta.
        let acts = [act("o", "", now - 10, Some(now - 5))];
        // Claude Code sin actividad registrada (caducó) y escribiendo: estimado; quieto: en
        // espera, sin dudas.
        let mut claude = term("c", "claude", "Claude Code");
        claude.output_at = Some(now - 2);
        claude.busy_since = Some(now - 10);
        let mut calm = term("k", "claude", "Claude Code");
        calm.output_at = Some(now - 90);
        let rows = rows(&acts, &[busy, quiet, claude, calm], now);
        assert_eq!((rows[0].state, rows[0].since), (State::Working, now - 300));
        assert_eq!(rows[0].title.as_deref(), Some("Fixing the login"));
        assert_eq!((rows[1].state, rows[1].since), (State::Idle, now - 60));
        assert!(!rows[0].reliable && !rows[1].reliable);
        assert_eq!((rows[2].state, rows[2].reliable), (State::Working, false));
        assert_eq!((rows[3].state, rows[3].reliable), (State::Idle, true));
    }

    #[test]
    fn subagents_show_only_for_agents_that_report_them_and_keep_the_parent_working() {
        let now = 10_000;
        let acts = [
            act("c", "", now - 500, Some(now - 30)),
            act("c", "a1", now - 200, None),
            act("c", "a2", now - 400, Some(now - 100)),
            act("x", "", now - 50, None),
            act("x", "a9", now - 40, None),
        ];
        let rows = rows(
            &acts,
            &[
                term("c", "claude", "Claude Code"),
                term("x", "codex", "Codex"),
            ],
            now,
        );
        let c = &rows[0];
        assert_eq!(c.subagents.len(), 2);
        assert_eq!(c.subagents[0].name, "a1: task");
        assert_eq!(c.subagents[0].state, State::Working);
        assert_eq!(c.subagents[1].state, State::Done);
        assert_eq!((c.state, c.since), (State::Working, now - 200));
        assert!(rows[1].subagents.is_empty(), "Codex no entrega subagentes");
    }

    /// Revisión: teclear o redimensionar redibuja la terminal; con hooks eso no es trabajar.
    #[test]
    fn a_redraw_does_not_wake_an_agent_with_hooks() {
        let now = 10_000;
        let mut done = term("s", "codex", "Codex");
        done.event = Some(("stop".into(), now - 100));
        done.output_at = Some(now - 1);
        done.busy_since = Some(now - 1);
        let mut fresh = term("f", "claude", "Claude Code");
        fresh.output_at = Some(now - 1);
        fresh.busy_since = Some(now - 1);
        let rows = rows(&[], &[done, fresh], now);
        assert_eq!(rows[0].state, State::Done);
        assert_eq!(rows[1].state, State::Idle);
    }

    /// Revisión: el aviso de «esperando entrada» tras terminar no es un permiso pendiente.
    #[test]
    fn a_waiting_notice_after_the_turn_ended_is_not_a_permission_request() {
        let now = 10_000;
        let mut t = term("s", "claude", "Claude Code");
        t.event = Some(("waiting".into(), now - 10));
        let a = act("s", "", now - 300, Some(now - 60));
        assert_eq!(rows(&[a], &[t], now)[0].state, State::Done);
    }

    /// Revisión: interrumpido con Esc no avisa; sin eventos ni salida un buen rato, en espera.
    #[test]
    fn a_silent_interrupted_turn_stops_spinning() {
        let now = 100_000;
        let t = term("s", "claude", "Claude Code");
        let a = act("s", "", now - STALE_AFTER - 50, None);
        let row = &rows(&[a], &[t], now)[0];
        assert_eq!(
            (row.state, row.since),
            (State::Idle, now - STALE_AFTER - 50)
        );
    }

    #[test]
    fn elapsed_is_short() {
        assert_eq!(elapsed(5), "ahora");
        assert_eq!(elapsed(49 * 60 + 5), "49m");
        assert_eq!(elapsed(3 * 3600 + 100), "3h");
        assert_eq!(elapsed(2 * 86400), "2d");
    }
}
