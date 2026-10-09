// Avisos que los agentes mandan a Forge con sus hooks (`forge agent-event`).
use std::io::Write;
use std::process::{Command, Stdio};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

#[test]
fn agent_event_records_the_agent_message() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let dir = std::env::temp_dir().join(format!("forge-events-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let run = |kind: &str, stdin: &str| {
        let mut child = Command::new(FORGE)
            .args(["agent-event", kind])
            .env("FORGE_EVENTS", &dir)
            .env("FORGE_PROJECT", "/p/bovinapp")
            .env("FORGE_ORIGIN", "Claude Code")
            .env("FORGE_DB", dir.join("forge.db"))
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success(), "el hook nunca falla");
    };
    // Lo que manda Claude Code en su hook Notification (JSON por stdin).
    run(
        "waiting",
        r#"{"hook_event_name":"Notification","message":"Claude needs your permission to use Bash"}"#,
    );
    run("stop", "{}");
    let events = forge_core::events::drain(&dir);
    let texts: Vec<&str> = events.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            // El aviso de Claude Code (en inglés) llega en el idioma de Forge.
            "Claude Code necesita permiso para usar Bash",
            "Claude Code terminó"
        ]
    );
    assert!(
        events
            .iter()
            .all(|e| e.project == std::path::Path::new("/p/bovinapp"))
    );
    assert!(
        forge_core::events::drain(&dir).is_empty(),
        "se borran al leerlos"
    );
}

#[test]
fn agent_usage_and_status_for_the_claude_mod() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let dir = std::env::temp_dir().join(format!("forge-usage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("forge.db");
    let usage = |json: &str| {
        let mut child = Command::new(FORGE)
            .arg("agent-usage")
            .env("FORGE_PROJECT", "/p/bovinapp")
            .env("FORGE_DB", &db)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
    };
    usage(
        r#"{"model":"opus","input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":20000,"cache_creation_input_tokens":0,"subagent":false}"#,
    );
    usage(
        r#"{"model":"haiku","input_tokens":8000,"output_tokens":300,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"subagent":true}"#,
    );
    usage("no es json"); // se ignora sin fallar

    let store = forge_core::store::Store::open(&db).unwrap();
    let used = store.agent_usage(None, 0).unwrap();
    let summary: Vec<(&str, u64, u64)> = used
        .iter()
        .map(|u| (u.model.as_str(), u.total(), u.subagent_turns))
        .collect();
    assert_eq!(summary, [("opus", 21_500, 0), ("haiku", 8_300, 1)]);

    store
        .add_idea(
            Some(std::path::Path::new("/p/bovinapp")),
            "Exportar a Excel",
            "",
            "usuario",
        )
        .unwrap();
    let out = Command::new(FORGE)
        .args(["status", "--json", "--project", "/p/bovinapp"])
        .env("FORGE_DB", &db)
        .output()
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["guard"], "not run");
    assert_eq!(status["ideas"], 1);
}
