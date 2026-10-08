// Control de comandos peligrosos con el binario real: `git`/`rm` pasan por los enlaces de
// Forge, lo inofensivo se ejecuta directo y lo peligroso espera la respuesta de la app.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use forge_core::danger;

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

struct Env {
    base: PathBuf,
    repo: PathBuf,
    vars: std::collections::HashMap<String, String>,
}

fn setup(name: &str) -> Env {
    let base = std::env::temp_dir().join(format!("forge-danger-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let base = base.canonicalize().unwrap();
    let repo = repo.canonicalize().unwrap();
    let mut vars = danger::install(&base.join("forge"), Path::new(FORGE)).unwrap();
    // El "proceso de la app" es este test (sigue vivo mientras espera la respuesta).
    vars.insert("FORGE_APP_PID".into(), std::process::id().to_string());
    vars.insert("FORGE_PROJECT".into(), repo.to_string_lossy().into_owned());
    vars.insert("FORGE_ORIGIN".into(), "Claude Code".into());
    let path = format!("{}:{}", vars["FORGE_SHIMS"], std::env::var("PATH").unwrap());
    vars.insert("PATH".into(), path);
    vars.remove("ZDOTDIR");
    let e = Env { base, repo, vars };
    assert!(sh(&e, "git init -q -b main && git config user.email t@t && git config user.name t && echo v1 > a.txt && git add . && git commit -qm inicio").status.success());
    e
}

fn sh(e: &Env, script: &str) -> Output {
    Command::new("sh")
        .args(["-c", script])
        .current_dir(&e.repo)
        .envs(&e.vars)
        .output()
        .unwrap()
}

/// Lanza el comando, espera su solicitud y la responde.
fn answered(e: &Env, script: &str, allow: bool) -> (Output, danger::Request) {
    let dir = PathBuf::from(&e.vars["FORGE_APPROVALS"]);
    let child = Command::new("sh")
        .args(["-c", script])
        .current_dir(&e.repo)
        .envs(&e.vars)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let request = loop {
        if let Some(r) = danger::pending(&dir).into_iter().next() {
            break r;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no llegó la solicitud"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    danger::answer(&dir, &request.id, allow).unwrap();
    (child.wait_with_output().unwrap(), request)
}

#[test]
fn dangerous_commands_wait_for_authorization() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let e = setup("git");
    // Inofensivo: se ejecuta sin preguntar (y es el git real).
    let out = sh(&e, "git status --short && rm -rf build");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(danger::pending(Path::new(&e.vars["FORGE_APPROVALS"])).is_empty());

    std::fs::write(e.repo.join("a.txt"), "cambio sin commit").unwrap();
    let (out, request) = answered(&e, "git reset --hard", false);
    assert_eq!(request.command, "git reset --hard");
    assert_eq!(request.origin.as_deref(), Some("Claude Code"));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("comando rechazado"));
    assert_eq!(
        std::fs::read_to_string(e.repo.join("a.txt")).unwrap(),
        "cambio sin commit"
    );

    let (out, _) = answered(&e, "git reset --hard", true);
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_to_string(e.repo.join("a.txt")).unwrap(),
        "v1\n"
    );

    // rm -rf fuera del proyecto.
    std::fs::create_dir_all(e.base.join("otro")).unwrap();
    let (out, request) = answered(&e, "rm -rf ../otro", false);
    assert!(!out.status.success());
    assert!(request.reason.contains("fuera de la carpeta del proyecto"));
    assert!(e.base.join("otro").exists());
}

#[test]
fn without_forge_open_and_no_terminal_it_is_blocked() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let mut e = setup("closed");
    e.vars.insert("FORGE_APP_PID".into(), "999999".into());
    let out = Command::new("sh")
        .args(["-c", "git reset --hard"])
        .current_dir(&e.repo)
        .envs(&e.vars)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("comando bloqueado"));
}

/// El arranque de zsh de Forge carga la configuración del usuario y deja los enlaces
/// primero en el PATH (después de path_helper y compañía).
#[test]
fn forge_zsh_puts_shims_first() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    if !Path::new("/bin/zsh").exists() {
        return;
    }
    let e = setup("zsh");
    let shims = e.vars["FORGE_SHIMS"].clone();
    let zdotdir = Path::new(&shims).parent().unwrap().join("zsh");
    let out = Command::new("/bin/zsh")
        .args([
            "-l",
            "-i",
            "-c",
            "echo FORGE_PATH=$PATH; echo FORGE_HIST=$HISTFILE; command -v git",
        ])
        .env("ZDOTDIR", &zdotdir)
        .env("FORGE_SHIMS", &shims)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let path = text
        .lines()
        // El prompt del usuario puede dejar secuencias de escape delante.
        .find_map(|l| l.split_once("FORGE_PATH=").map(|(_, p)| p))
        .unwrap_or_else(|| panic!("sin PATH en la salida:\n{text}"));
    assert!(path.starts_with(&format!("{shims}:")), "{path}");
    assert!(
        text.lines().any(|l| l.ends_with(&format!("{shims}/git"))),
        "{text}"
    );
}
