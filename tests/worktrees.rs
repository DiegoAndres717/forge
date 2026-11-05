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

#[test]
fn agent_shells_cannot_make_stray_worktrees_or_clones_of_the_project() {
    let e = setup("shim");
    let blocked = |dir: &Path, script: &str| {
        let out = sh(&e, dir, script);
        let err = text(&out.stderr);
        assert!(!out.status.success(), "{script} pasó: {err}");
        assert!(err.contains("forge worktree new"), "{script}: {err}");
    };
    blocked(&e.repo, "git worktree add ../wt-x");
    blocked(&e.base, "git clone repo clone-x");
    blocked(&e.base, "git clone git@github.com:acme/repo.git clone-y");
    assert!(!e.base.join("wt-x").exists() && !e.base.join("clone-x").exists());
    // Lo demás pasa: listar worktrees y clonar otro repositorio.
    let list = sh(&e, &e.repo, "git worktree list");
    assert!(list.status.success(), "{}", text(&list.stderr));
    let other = sh(&e, &e.base, "git clone -q other other-clone");
    assert!(other.status.success(), "{}", text(&other.stderr));
    // La salida explícita.
    let escape = sh(
        &e,
        &e.repo,
        "FORGE_ALLOW_WORKTREE=1 git worktree add -q ../wt-ok",
    );
    assert!(escape.status.success(), "{}", text(&escape.stderr));
    assert!(e.base.join("wt-ok/a.txt").is_file());
    // Lo que escribe la persona (sin FORGE_ORIGIN) no se frena.
    let mut person = Command::new("sh");
    person
        .args(["-c", "git worktree add -q ../wt-person"])
        .current_dir(&e.repo)
        .envs(&e.vars)
        .env_remove("FORGE_ORIGIN");
    assert!(person.output().unwrap().status.success());
}

/// El hook previo de Codex, Gemini CLI y Antigravity (`forge secret --agent-hook <formato>`)
/// deniega en el formato de cada uno; el mod de Claude Code pregunta `forge worktree check`.
#[test]
fn each_agent_hook_denies_stray_copies_and_lets_the_rest_through() {
    let e = setup("hooks");
    let hook = |format: &str, input: String| {
        let file = e.base.join("input.json");
        std::fs::write(&file, input).unwrap();
        let out = Command::new(FORGE)
            .args(["secret", "--agent-hook", format])
            .current_dir(&e.repo)
            .envs(&e.vars)
            .env_remove("FORGE_APPROVALS")
            .stdin(std::fs::File::open(&file).unwrap())
            .output()
            .unwrap();
        assert!(out.status.success(), "{format}: {}", text(&out.stderr));
        text(&out.stdout).trim().to_string()
    };
    let cwd = serde_json::to_string(&e.repo).unwrap();
    let codex =
        |c: &str| format!(r#"{{"cwd":{cwd},"tool_name":"Bash","tool_input":{{"command":"{c}"}}}}"#);
    let gemini = |c: &str| {
        format!(
            r#"{{"cwd":{cwd},"tool_name":"run_shell_command","tool_input":{{"command":"{c}"}}}}"#
        )
    };
    let agy = |c: &str| {
        format!(
            r#"{{"workspacePaths":[{cwd}],"toolCall":{{"name":"run_command","args":{{"CommandLine":"{c}"}}}}}}"#
        )
    };
    for bad in [
        "git worktree add ../x",
        "git clone https://github.com/acme/repo /tmp/x",
    ] {
        let v: serde_json::Value = serde_json::from_str(&hook("codex", codex(bad))).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecision"], "deny",
            "{bad}"
        );
        let why = v["hookSpecificOutput"]["permissionDecisionReason"].as_str();
        assert!(why.unwrap().contains("forge worktree new"), "{v}");
        for (format, input) in [("gemini", gemini(bad)), ("agy", agy(bad))] {
            let v: serde_json::Value = serde_json::from_str(&hook(format, input)).unwrap();
            assert_eq!(v["decision"], "deny", "{format}: {bad}");
            assert!(v["reason"].as_str().unwrap().contains("forge worktree new"));
        }
    }
    let other = format!("git clone {} c", e.other.display()).replace('\\', "/");
    for ok in [
        "git worktree list",
        other.as_str(),
        "FORGE_ALLOW_WORKTREE=1 git worktree add ../x",
    ] {
        assert_eq!(hook("codex", codex(ok)), "", "{ok}");
        assert_eq!(hook("gemini", gemini(ok)), "{}", "{ok}");
        assert_eq!(hook("agy", agy(ok)), "{}", "{ok}");
    }
    // Claude Code: el mod pregunta y deniega si sale 0 con el motivo.
    let check = |c: &str| forge(&e, &["worktree", "check", "--command", c]);
    let out = check("git worktree add ../x");
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains("forge worktree new"));
    assert_eq!(check("git worktree list").status.code(), Some(1));
}
