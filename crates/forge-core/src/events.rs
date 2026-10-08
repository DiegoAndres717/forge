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
