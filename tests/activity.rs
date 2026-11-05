// El tablero de agentes con el binario real: lo que mandan los hooks (`forge agent-activity`,
// el hook previo de Codex) llega a `forge work-status`, con los solapes.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

fn repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forge-it-act-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("repo/src")).unwrap();
    let repo = dir.join("repo");
    std::fs::write(repo.join("src/x.rs"), "v0\n").unwrap();
    let ok = Command::new("sh")
        .args([
            "-c",
            "git init -q -b main && git config user.email t@t && git config user.name t \
             && git add . && git commit -qm inicio",
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    forge_core::canonical(&dir)
}

/// `forge …` en una terminal de agente simulada, sin nada de la terminal de Forge que corre
/// la prueba (memoria #52): su propia base, sin la app.
fn forge(dir: &Path, session: &str, origin: &str, args: &[&str], stdin: &str) -> String {
    let mut cmd = Command::new(FORGE);
    for var in [
        "FORGE_EVENTS",
        "FORGE_APPROVALS",
        "FORGE_APP_PID",
        "FORGE_PANEL",
        "FORGE_READ_SHIMS",
        "FORGE_GUARD_COMMANDS",
    ] {
        cmd.env_remove(var);
    }
    let mut child = cmd
        .args(args)
        .current_dir(dir.join("repo"))
        .env("FORGE_DB", dir.join("forge.db"))
        .env("FORGE_PROJECT", dir.join("repo"))
        .env("FORGE_SESSION_ID", session)
        .env("FORGE_ORIGIN", origin)
        .env("FORGE_LANG", "en")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{args:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn two_terminals_editing_the_same_file_show_up_as_an_overlap() {
    let dir = repo("overlap");
    let file = dir.join("repo/src/x.rs");
    let edit = serde_json::json!({"kind": "tool", "tool": "Edit", "target": file, "origin": "Claude Code"})
        .to_string();
    forge(&dir, "9:1", "Claude Code", &["agent-activity"], &edit);
    let spawn = serde_json::json!({"kind": "start", "agent_id": "a1", "name": "explorer: find x", "origin": "Claude Code"})
        .to_string();
    forge(&dir, "9:1", "Claude Code", &["agent-activity"], &spawn);
    forge(&dir, "9:2", "Claude Code", &["agent-activity"], &edit);
    // Codex por su hook previo (el de Guard): se ve el comando, no las ediciones.
    let codex = r#"{"tool_name":"Bash","tool_input":{"command":"cargo build"}}"#;
    let reply = forge(
        &dir,
        "9:3",
        "Codex",
        &["secret", "--agent-hook", "codex"],
        codex,
    );
    assert_eq!(reply.trim(), "", "sin decisión: la llamada sigue");
    // Leer un .env lo bloquea Guard (sin la app no se puede autorizar): no va al tablero.
    let secret = r#"{"tool_name":"Bash","tool_input":{"command":"cat .env"}}"#;
    let reply = forge(
        &dir,
        "9:4",
        "Codex",
        &["secret", "--agent-hook", "codex"],
        secret,
    );
    assert!(reply.contains("deny"), "{reply}");

    // El hook lo apunta sin esperar (no frena la llamada): se espera a que llegue.
    let mut status = String::new();
    for _ in 0..50 {
        status = forge(&dir, "9:1", "Claude Code", &["work-status"], "");
        if status.contains("Codex (terminal 3)") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        !status.contains("cat .env") && !status.contains("terminal 4"),
        "{status}"
    );
    assert!(
        status.contains("- src/x.rs: Claude Code (terminal 1) and Claude Code (terminal 2)"),
        "{status}"
    );
    assert!(
        status.contains("Claude Code ↳ explorer: find x (terminal 1)"),
        "{status}"
    );
    assert!(
        status.contains("Codex (terminal 3) · running `cargo build`"),
        "{status}"
    );
    assert!(status.contains("commands and reads only"), "{status}");
    assert!(status.contains("[you]"), "{status}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn outside_a_forge_terminal_nothing_is_recorded() {
    let dir = repo("outside");
    let edit = r#"{"kind":"tool","tool":"Edit","target":"/x/a.rs"}"#;
    let mut child = Command::new(FORGE)
        .arg("agent-activity")
        .env_remove("FORGE_SESSION_ID")
        .env("FORGE_DB", dir.join("forge.db"))
        .env("FORGE_PROJECT", dir.join("repo"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(edit.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success(), "el hook nunca falla");
    let status = forge(&dir, "9:1", "Claude Code", &["work-status"], "");
    assert!(status.contains("- none running"), "{status}");
    std::fs::remove_dir_all(&dir).unwrap();
}
