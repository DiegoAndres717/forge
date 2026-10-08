// Eventos que los propios agentes envían a Forge al terminar o al necesitar al usuario
// (hooks `Stop`/`Notification` de Claude Code, `notify` de Codex). Mucho más fiable que
// adivinar por la salida de la terminal: llegan justo cuando toca y con su mensaje.
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentEvent {
    pub project: PathBuf,
    /// Texto para el usuario: "Claude Code terminó" o el mensaje del agente.
    pub text: String,
}

/// Tokens de un turno de un agente (lo manda el mod de Forge para Claude Code).
#[derive(Deserialize, Debug, PartialEq)]
pub struct TurnUsage {
    pub model: String,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// Turno de un subagente (no de la conversación principal).
    #[serde(default)]
    pub subagent: bool,
}

/// Consumo por modelo en un periodo.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelUse {
    pub model: String,
    pub turns: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Turnos hechos por subagentes.
    pub subagent_turns: u64,
}

impl ModelUse {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

pub fn init(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_usage (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            project     TEXT NOT NULL,
            model       TEXT NOT NULL,
            input       INTEGER NOT NULL,
            output      INTEGER NOT NULL,
            cache_read  INTEGER NOT NULL,
            cache_write INTEGER NOT NULL,
            subagent    INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
        );",
    )
}

impl crate::store::Store {
    pub fn add_agent_usage(&self, project: &Path, u: &TurnUsage) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO agent_usage (project, model, input, output, cache_read, cache_write, subagent, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    project.to_string_lossy(),
                    u.model,
                    u.input_tokens as i64,
                    u.output_tokens as i64,
                    u.cache_read_input_tokens as i64,
                    u.cache_creation_input_tokens as i64,
                    u.subagent,
                    crate::store::now()
                ],
            )
            .map(drop)
            .map_err(|e| e.to_string())
    }

    /// Consumo por modelo desde `since` (todos los proyectos si `project` es None),
    /// del modelo que más tokens usó al que menos.
    pub fn agent_usage(&self, project: Option<&Path>, since: i64) -> Result<Vec<ModelUse>, String> {
        let project = project.map(|p| p.to_string_lossy().into_owned());
        let mut stmt = self
            .conn
            .prepare(
                "SELECT model, COUNT(*), SUM(input), SUM(output), SUM(cache_read), SUM(cache_write), SUM(subagent)
                 FROM agent_usage WHERE created_at >= ?1 AND (?2 IS NULL OR project = ?2)
                 GROUP BY model ORDER BY SUM(input + output + cache_read + cache_write) DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![since, project], |r| {
                Ok(ModelUse {
                    model: r.get(0)?,
                    turns: r.get::<_, i64>(1)? as u64,
                    input: r.get::<_, i64>(2)? as u64,
                    output: r.get::<_, i64>(3)? as u64,
                    cache_read: r.get::<_, i64>(4)? as u64,
                    cache_write: r.get::<_, i64>(5)? as u64,
                    subagent_turns: r.get::<_, i64>(6)? as u64,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }
}

/// `forge agent-usage`: guarda los tokens de un turno (JSON por stdin). Nunca falla.
pub fn record_usage() {
    let Some(project) = std::env::var_os("FORGE_PROJECT") else {
        return;
    };
    let mut payload = String::new();
    let _ = std::io::stdin()
        .take(64 * 1024)
        .read_to_string(&mut payload);
    let Ok(usage) = serde_json::from_str::<TurnUsage>(&payload) else {
        return;
    };
    if let Some(store) =
        crate::store::Store::default_path().and_then(|p| crate::store::Store::open(&p).ok())
    {
        let _ = store.add_agent_usage(Path::new(&project), &usage);
    }
}

/// Lo que ejecuta el hook (`forge agent-event stop|waiting [json]`). Nunca falla: un
/// error aquí no debe interrumpir al agente.
pub fn record(kind: &str, json_arg: Option<&str>) {
    let (Some(dir), Some(project)) = (
        std::env::var_os("FORGE_EVENTS"),
        std::env::var_os("FORGE_PROJECT"),
    ) else {
        return;
    };
    // Claude manda el detalle por stdin; Codex, como último argumento.
    let mut payload = json_arg.unwrap_or_default().to_string();
    if payload.is_empty() {
        let _ = std::io::stdin()
            .take(64 * 1024)
            .read_to_string(&mut payload);
    }
    let message = serde_json::from_str::<serde_json::Value>(&payload)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .filter(|m| !m.trim().is_empty());
    let name = std::env::var("FORGE_ORIGIN").unwrap_or_else(|_| "Agente".into());
    let text = match (kind, message) {
        ("waiting", Some(message)) => message,
        ("waiting", None) => crate::tr!("{name} espera tu respuesta", name = name),
        // El mod de Forge manda la primera línea de la respuesta como resumen.
        (_, Some(summary)) => format!("{name}: {summary}"),
        _ => crate::tr!("{name} terminó", name = name),
    };
    let event = AgentEvent {
        project: PathBuf::from(project),
        text,
    };
    let dir = PathBuf::from(dir);
    let id = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
        std::process::id()
    );
    let tmp = dir.join(format!("{id}.tmp"));
    if let Ok(json) = serde_json::to_vec(&event)
        && std::fs::write(&tmp, json).is_ok()
    {
        let _ = std::fs::rename(&tmp, dir.join(format!("{id}.json")));
    }
}

/// Eventos pendientes (se borran al leerlos), del más antiguo al más nuevo.
pub fn drain(dir: &Path) -> Vec<AgentEvent> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|p| {
            let event = serde_json::from_slice(&std::fs::read(&p).ok()?).ok();
            let _ = std::fs::remove_file(&p);
            event
        })
        .collect()
}
