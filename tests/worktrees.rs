// Plan #194, fase 2, con el binario real: `forge worktree new` crea el worktree en la
// carpeta de Forge y lo registra, y en las terminales de agente Guard frena los
// `git worktree add` y `git clone` del propio proyecto (enlaces de Guard y hooks previos de
// cada agente), con `FORGE_ALLOW_WORKTREE=1` como salida.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

struct Env {
    base: PathBuf,
    repo: PathBuf,
    /// Otro repositorio (clonarlo no es una copia del proyecto).
    other: PathBuf,
    vars: std::collections::HashMap<String, String>,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Un proyecto con remoto y el entorno de una terminal de agente de Forge (enlaces de
/// Guard primero en el PATH, base de datos propia).
fn setup(name: &str) -> Env {
    let base = std::env::temp_dir().join(format!("forge-wt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for dir in ["repo", "other"] {
        std::fs::create_dir_all(base.join(dir)).unwrap();
    }
    let base = forge_core::canonical(&base);
    let (repo, other) = (base.join("repo"), base.join("other"));
    let mut vars = forge_core::danger::install(&base.join("forge"), Path::new(FORGE)).unwrap();
    vars.insert("FORGE_PROJECT".into(), repo.to_string_lossy().into_owned());
    vars.insert("FORGE_ORIGIN".into(), "Codex".into());
    vars.insert("FORGE_SESSION_ID".into(), "42:7".into());
    vars.insert(
        "FORGE_DB".into(),
        base.join("data/forge.db").to_string_lossy().into_owned(),
    );
    vars.insert("FORGE_LANG".into(), "en".into());
    // Si un clon no se frenara, que falle al instante en vez de salir a la red o preguntar.
    vars.insert("GIT_SSH_COMMAND".into(), "false".into());
    vars.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    vars.insert("GIT_ALLOW_PROTOCOL".into(), "file".into());
    let path = std::env::join_paths(
        std::iter::once(PathBuf::from(&vars["FORGE_SHIMS"]))
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    vars.insert("PATH".into(), path.to_string_lossy().into_owned());
    vars.remove("ZDOTDIR");
    let e = Env {
        base,
        repo,
        other,
        vars,
    };
    let init = "git init -q -b main && git config user.email t@t && git config user.name t \
                && git config core.autocrlf false && echo v1 > a.txt && git add . \
                && git commit -qm inicio";
    for dir in [&e.repo, &e.other] {
        assert!(sh(&e, dir, init).status.success());
    }
    assert!(
        sh(
            &e,
            &e.repo,
            "git remote add origin https://github.com/acme/repo.git"
        )
        .status
        .success()
    );
    e
}

/// `sh -c script` en `dir` con el entorno de la terminal de agente.
fn sh(e: &Env, dir: &Path, script: &str) -> Output {
    // Windows: el sh de Git for Windows pone su git.exe delante; los enlaces vuelven primero.
    let script = if cfg!(windows) {
        format!("PATH=\"$(cygpath -u \"$FORGE_SHIMS\"):$PATH\"; {script}")
    } else {
        script.to_string()
    };
    Command::new("sh")
        .args(["-c", &script])
        .current_dir(dir)
        .envs(&e.vars)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(out: &[u8]) -> String {
    String::from_utf8_lossy(out).into_owned()
}

fn forge(e: &Env, args: &[&str]) -> Output {
    Command::new(FORGE)
        .args(args)
        .current_dir(&e.repo)
        .envs(&e.vars)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn worktree_new_creates_it_in_forges_folder_and_registers_who_asked() {
    let e = setup("new");
    let out = forge(&e, &["worktree", "new", "feat/login"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    // Solo la ruta en stdout: `cd "$(forge worktree new x)"` funciona.
    let printed = PathBuf::from(text(&out.stdout).trim());
    let expected = e.base.join("data/worktrees/repo/feat-login");
    assert_eq!(forge_core::canonical(&printed), expected);
    assert!(expected.join("a.txt").is_file());
    let branch = sh(&e, &expected, "git branch --show-current");
    assert_eq!(text(&branch.stdout).trim(), "feat/login");
    // `forge worktrees` (fase 1) dice quién lo pidió.
    let list = text(&forge(&e, &["worktrees"]).stdout);
    assert!(
        list.contains("feat/login") && list.contains("Codex"),
        "{list}"
    );
}
