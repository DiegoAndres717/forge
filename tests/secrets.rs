// Las órdenes que usa el mod (`forge anchor`, `forge secret`) se ejecutan en la CLI y no
// abren la app, y los lectores de las terminales de agente piden permiso antes de leer
// secretos.
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

/// Ejecuta con un tiempo máximo: si abriera la ventana, no terminaría.
fn run(cmd: &mut Command) -> (i32, String) {
    run_with(cmd, Stdio::null())
}

/// Como `run`, con `stdin` como entrada.
fn run_with(cmd: &mut Command, stdin: Stdio) -> (i32, String) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Sin terminal de control: nada puede preguntar por /dev/tty y quedarse esperando.
        // SAFETY: setsid entre fork y exec, sin memoria compartida.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(stdin)
        .spawn()
        .unwrap();
    let start = Instant::now();
    let timeout = Duration::from_secs(if cfg!(windows) { 30 } else { 10 });
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > timeout {
            let _ = child.kill();
            let out = child.wait_with_output().ok();
            let (stdout, stderr) = out
                .as_ref()
                .map(|o| {
                    (
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr),
                    )
                })
                .unwrap_or_default();
            panic!("{cmd:?} no terminó: ¿abrió la app?\nstdout:\n{stdout}\nstderr:\n{stderr}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn base(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("forge-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    forge_core::canonical(&dir)
}

#[test]
fn mod_commands_run_in_the_cli() {
    let dir = base("cli");
    let forge = |args: &[&str]| {
        run(Command::new(FORGE)
            .args(args)
            .current_dir(&dir)
            .env("FORGE_DB", dir.join("forge.db"))
            .env("FORGE_LANG", "es"))
    };
    assert_eq!(
        forge(&["anchor"]).0,
        0,
        "sin planes en curso: nada que decir"
    );
    let (code, out) = forge(&["secret", "--path", ".env"]);
    assert_eq!(code, 0);
    assert!(out.contains(".env"), "{out}");
    assert_eq!(forge(&["secret", "--path", ".env.example"]).0, 1);
    assert_eq!(forge(&["secret", "--command", "cat src/main.rs"]).0, 1);
    assert_eq!(forge(&["danger", "--command", "git status"]).0, 1);
    assert_eq!(forge(&["danger", "--command", "git push --force"]).0, 0);
    assert_eq!(
        forge(&["secret", "--command", "grep KEY .env.local | head"]).0,
        0
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn agent_readers_ask_before_reading_secrets() {
    let dir = base("readers");
    std::fs::write(dir.join(".env"), "TOKEN=x\n").unwrap();
    std::fs::write(dir.join("notes.txt"), "hola\n").unwrap();
    // Los lectores como los instala Forge, en un PATH de terminal de agente.
    let installed = forge_core::danger::install(&dir, std::path::Path::new(FORGE)).unwrap();
    let readers = installed["FORGE_READ_SHIMS"].clone();
    let path = format!("{readers}:/usr/bin:/bin");
    let read = |program: &str, args: &[&str], agent: bool| {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(&dir)
            .env("PATH", &path)
            .env("FORGE_BIN", FORGE)
            .env("FORGE_READ_SHIMS", &readers)
            .env("FORGE_DB", dir.join("forge.db"))
            .env("FORGE_LANG", "es")
            // Sin la app ni una terminal que pregunte: se bloquea.
            .env_remove("FORGE_APP_PID")
            .env_remove("FORGE_APPROVALS");
        if agent {
            cmd.env("FORGE_ORIGIN", "Codex");
        } else {
            cmd.env_remove("FORGE_ORIGIN");
        }
        run(&mut cmd)
    };
    let ok = (0, "hola\n".to_string());
    assert_eq!(read("cat", &["notes.txt"], true), ok, "lo normal pasa");
    // `sed` y `grep` también son lectores: el guion no se llama a sí mismo en bucle.
    assert_eq!(read("sed", &["-n", "1p", "notes.txt"], true), ok);
    assert_eq!(read("grep", &["hola", "notes.txt"], true), ok);
    let (code, out) = read("cat", &[".env"], true);
    assert_eq!(code, 126, "un agente no lee .env sin permiso");
    assert!(out.is_empty());
    assert_eq!(read("grep", &["TOKEN", ".env"], true).0, 126);
    assert_eq!(
        read("cat", &[".env"], false),
        (0, "TOKEN=x\n".into()),
        "la persona sí"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// El hook previo de Codex, Gemini CLI y Antigravity (`forge secret --agent-hook <formato>`): sin
/// la app ni una terminal que pregunte, un secreto se deniega en el formato de cada agente;
/// lo demás pasa sin decir nada (el agente sigue con sus propios permisos).
#[test]
fn agent_hooks_deny_secret_reads_in_each_format() {
    let dir = base("hook");
    let hook = |format: &str, input: &str, agent: bool| {
        let mut cmd = Command::new(FORGE);
        cmd.args(["secret", "--agent-hook", format])
            .current_dir(&dir)
            .env("FORGE_DB", dir.join("forge.db"))
            .env("FORGE_LANG", "en")
            .env_remove("FORGE_APP_PID")
            .env_remove("FORGE_APPROVALS");
        if agent {
            cmd.env("FORGE_ORIGIN", "Codex");
        } else {
            cmd.env_remove("FORGE_ORIGIN");
        }
        let file = dir.join("input.json");
        std::fs::write(&file, input).unwrap();
        let (code, out) = run_with(&mut cmd, std::fs::File::open(&file).unwrap().into());
        (code, out.trim().to_string())
    };
    let codex = r#"{"tool_name":"Bash","tool_input":{"command":"cat .env"}}"#;
    let (code, out) = hook("codex", codex, true);
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    let agy = r#"{"toolCall":{"name":"view_file","args":{"AbsolutePath":".env"}}}"#;
    let v: serde_json::Value = serde_json::from_str(&hook("agy", agy, true).1).unwrap();
    assert_eq!(v["decision"], "deny");
    let gemini = r#"{"tool_name":"read_file","tool_input":{"file_path":".env"}}"#;
    let v: serde_json::Value = serde_json::from_str(&hook("gemini", gemini, true).1).unwrap();
    assert_eq!(v["decision"], "deny");
    // Lo normal, o lo que no viene de un agente de Forge, pasa sin decisión.
    let normal = r#"{"tool_name":"Bash","tool_input":{"command":"cargo test"}}"#;
    assert_eq!(hook("codex", normal, true), (0, String::new()));
    assert_eq!(hook("codex", codex, false), (0, String::new()));
    let (code, out) = hook(
        "agy",
        r#"{"toolCall":{"name":"run_command","args":{"CommandLine":"ls"}}}"#,
        true,
    );
    assert_eq!(
        (code, out.as_str()),
        (0, "{}"),
        "agy y Gemini esperan un JSON"
    );
    let _ = std::fs::remove_dir_all(dir);
}
