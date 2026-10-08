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
            "Claude needs your permission to use Bash",
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
