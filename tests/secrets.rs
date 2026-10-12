// Las órdenes que usa el mod (`forge anchor`, `forge secret`) se ejecutan en la CLI y no
// abren la app, y los lectores de las terminales de agente piden permiso antes de leer
// secretos.
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

/// Ejecuta con un tiempo máximo: si abriera la ventana, no terminaría.
fn run(cmd: &mut Command) -> (i32, String) {
    use std::os::unix::process::CommandExt;
    // Sin terminal de control: nada puede preguntar por /dev/tty y quedarse esperando.
    // SAFETY: setsid entre fork y exec, sin memoria compartida.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            panic!("{cmd:?} no terminó: ¿abrió la app?");
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
    dir.canonicalize().unwrap()
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
    assert_eq!(
        forge(&["secret", "--command", "grep KEY .env.local | head"]).0,
        0
    );
    let _ = std::fs::remove_dir_all(dir);
}

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
