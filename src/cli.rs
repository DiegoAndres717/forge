// Línea de comandos: `forge guard …` y `forge hooks …` (sin abrir la ventana).
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use forge_core::guard::{
    self, CheckState, Exception, GuardRun, Level, Rules, RunOptions, Scope, Stage, Verdict,
};
use forge_core::hooks::{self, HookState};
use forge_core::project::Project;
use forge_core::store::{self, Store};

pub const USAGE: &str = "\
Forge — workspace de proyectos con terminales, agentes y Project Guard

Uso:
  forge [carpeta]                       abre la app (y ese proyecto)
  forge guard commit|push|pr            comprueba si el proyecto está listo
  forge guard status                    reglas, hooks y excepciones del proyecto
  forge guard explain [commit|push|pr]  qué se exige en cada etapa
  forge guard allow <regla> --reason \"motivo\" [--scope commit|pull-request|session] [--stage commit|push|pr]
  forge guard exceptions                historial de excepciones
  forge guard revoke <id>               desactiva una excepción
  forge guard history                   últimas validaciones del proyecto
  forge guard report [id] [--output f]  reporte en Markdown (por defecto el último)
  forge hooks install|uninstall|status  hooks pre-commit y pre-push
  forge agent list                      agentes detectados, versión y capacidades
  forge agent open <id> [--resume]      abre el agente del proyecto en esta terminal
  forge doctor                          comprueba git, base de datos, configuración, hooks y agentes
  forge ai route [commit|push|pr]       riesgo del cambio y qué modelos lo revisarían (sin gastar)
  forge ai usage                        consumo de modelos de este mes
  forge ai init                         crea .forge/routing.toml
  forge memory search <texto> [--kind k]   busca en la memoria del proyecto
  forge memory add <tipo> \"título\" \"texto\" [--tags a,b]
  forge memory list [--kind k] · forge memory delete <id>
  forge mcp                             servidor MCP (memoria y contexto) para agentes

Opciones:
  --project <carpeta>   proyecto (por defecto: la carpeta con .forge/ más cercana o la raíz del repo)
  --hook                modo hook de Git (lo usan los hooks instalados)
";

/// Salida con colores solo si es un terminal.
struct Out {
    to_stderr: bool,
    color: bool,
}

impl Out {
    fn new(to_stderr: bool) -> Self {
        let color = if to_stderr {
            std::io::stderr().is_terminal()
        } else {
            std::io::stdout().is_terminal()
        };
        Self { to_stderr, color }
    }

    fn line(&self, text: &str) {
        if self.to_stderr {
            let _ = writeln!(std::io::stderr(), "{text}");
        } else {
            let _ = writeln!(std::io::stdout(), "{text}");
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

fn level_mark(out: &Out, level: Level) -> String {
    match level {
        Level::Pass => out.paint("32", "✓"),
        Level::Warn => out.paint("33", "⚠"),
        Level::Block => out.paint("31", "✕"),
        Level::Pending => out.paint("2", "○"),
        Level::Info => out.paint("2", "•"),
        Level::Excepted => out.paint("33", "↷"),
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Argumentos posicionales (sin opciones ni sus valores).
fn positional(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if matches!(
            a.as_str(),
            "--project" | "--reason" | "--scope" | "--stage" | "--output" | "--kind" | "--tags"
        ) {
            skip = true;
        } else if !a.starts_with("--") {
            out.push(a.as_str());
        }
    }
    out
}

/// Carpeta con `.forge/` más cercana; si no, la raíz del repositorio; si no, la actual.
fn find_project(args: &[String]) -> PathBuf {
    if let Some(p) = flag(args, "--project") {
        return PathBuf::from(p);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.ancestors()
        .find(|p| p.join(".forge").is_dir())
        .map(Path::to_path_buf)
        .or_else(|| guard::repo_root(&cwd).ok())
        .unwrap_or(cwd)
}

fn open_store() -> Result<Store, String> {
    Store::default_path()
        .ok_or_else(|| "no se encontró $HOME".to_string())
        .and_then(|p| Store::open(&p))
}

pub fn run(args: &[String]) -> i32 {
    let pos = positional(args);
    let project = find_project(args);
    let hook = args.iter().any(|a| a == "--hook");
    let out = Out::new(hook);
    let result = match (pos.first().copied(), pos.get(1).copied()) {
        (Some("guard"), Some(stage @ ("commit" | "push" | "pr"))) => gate(
            &out,
            &project,
            Stage::parse(stage).unwrap_or(Stage::Commit),
            hook,
        ),
        (Some("guard"), Some("status")) => status(&out, &project),
        (Some("guard"), Some("explain")) => {
            explain(&out, &project, pos.get(2).and_then(|s| Stage::parse(s)))
        }
        (Some("guard"), Some("allow")) => allow(&out, &project, args, pos.get(2).copied()),
        (Some("guard"), Some("exceptions")) => exceptions(&out, &project),
        (Some("guard"), Some("revoke")) => revoke(&out, &project, pos.get(2).copied()),
        (Some("guard"), Some("history")) => history(&out, &project),
        (Some("guard"), Some("report")) => {
            report(&out, &project, pos.get(2).copied(), flag(args, "--output"))
        }
        (Some("hooks"), Some("install")) => hooks_install(&out, &project),
        (Some("hooks"), Some("uninstall")) => hooks::uninstall(&project).map(|r| {
            r.iter().for_each(|l| out.line(l));
            0
        }),
        (Some("hooks"), Some("status")) => hooks_status(&out, &project),
        (Some("agent"), Some("list")) => agent_list(&out, &project),
        (Some("agent"), Some("open")) => agent_open(
            &project,
            pos.get(2).copied(),
            args.iter().any(|a| a == "--resume"),
        ),
        (Some("doctor"), _) => doctor(&out, &project),
        (Some("ai"), Some("route")) => ai_route(
            &out,
            &project,
            pos.get(2)
                .and_then(|s| Stage::parse(s))
                .unwrap_or(Stage::Commit),
        ),
        (Some("ai"), Some("usage")) => ai_usage(&out, &project),
        (Some("mcp"), _) => open_store().and_then(|store| {
            forge_core::mcp::serve(&project, &store)
                .map(|_| 0)
                .map_err(|e| e.to_string())
        }),
        (Some("memory"), Some(action)) => memory_cli(&out, &project, action, &pos[2..], args),
        (Some("ai"), Some("init")) => forge_core::router::write_template(&project).map(|_| {
            out.line(&format!(
                "Creado {}",
                project.join(".forge/routing.toml").display()
            ));
            0
        }),
        _ => {
            out.line(USAGE);
            Ok(2)
        }
    };
    result.unwrap_or_else(|e| {
        out.line(&format!("{} {e}", out.paint("31", "Forge:")));
        1
    })
}

/// Ejecuta el Guard de una etapa e imprime el resultado. 0 = listo; 1 = bloqueado.
fn gate(out: &Out, project_dir: &Path, stage: Stage, hook: bool) -> Result<i32, String> {
    let project = Project::load(project_dir)?;
    let rules = Rules::load(project_dir)?.unwrap_or_default();
    if !rules.stage(stage).enabled {
        out.line(&format!(
            "Forge Guard: {} desactivado en .forge/rules.toml",
            stage.label()
        ));
        return Ok(0);
    }
    // pre-push: las referencias a subir llegan por stdin.
    let mut push_range = None;
    if hook && stage == Stage::Push {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        let deletions_only = !input.trim().is_empty()
            && input.lines().all(|l| {
                l.split_whitespace()
                    .nth(1)
                    .is_some_and(|sha| sha.chars().all(|c| c == '0'))
            });
        if deletions_only {
            return Ok(0);
        }
        push_range = hooks::push_range(&input);
    }
    let store = open_store();
    let (exceptions, evidence) = match &store {
        Ok(store) => (
            store.exceptions(project_dir).unwrap_or_default(),
            store.evidence(project_dir).unwrap_or_default(),
        ),
        Err(e) => {
            out.line(&format!(
                "{} sin excepciones ni evidencias guardadas: {e}",
                out.paint("33", "⚠")
            ));
            (Vec::new(), Vec::new())
        }
    };

    out.line(&out.paint(
        "1",
        &format!("Forge Guard · {} · {}", project.name(), stage.label()),
    ));
    let options = RunOptions {
        env: project.config.environment.clone(),
        exceptions,
        push_range,
        evidence,
        name: project.name(),
        routing: Some(forge_core::router::RouterConfig::load(project_dir)?),
        month_spent: store
            .as_ref()
            .ok()
            .and_then(|s| s.month_spent(project_dir).ok())
            .unwrap_or(0.0),
        reviewers: forge_core::reviewers::load(project_dir)?,
    };
    let handle = guard::run(project_dir, &project.root(), rules, stage, options, || {});

    // Muestra el análisis y luego cada check según termina.
    let mut shown_items = false;
    let mut shown_checks = vec![false; 0];
    loop {
        let finished = handle.snapshot(|r| {
            if !shown_items && !r.analyzing {
                shown_items = true;
                if let Some(e) = &r.error {
                    out.line(&format!("  {} {e}", level_mark(out, Level::Block)));
                }
                if let Some(c) = &r.frozen {
                    let place = if c.isolated {
                        " · checks en copia aislada del candidato"
                    } else {
                        ""
                    };
                    out.line(&out.paint("2", &format!("  candidato {}{place}", c.id)));
                }
                // Las revisiones con IA se muestran al final, cuando terminan.
                for item in r.items.iter().filter(|i| !is_ai(&i.id)) {
                    out.line(&format!("  {} {}", level_mark(out, item.level), item.label));
                    if item.level != Level::Pass && item.level != Level::Info {
                        for d in item.details.iter().take(8) {
                            out.line(&out.paint("2", &format!("      {d}")));
                        }
                    }
                }
                shown_checks = vec![false; r.checks.len()];
                if !r.checks.is_empty() {
                    let names: Vec<&str> = r.checks.iter().map(|c| c.check.id.as_str()).collect();
                    out.line(&out.paint("2", &format!("  … ejecutando {}", names.join(", "))));
                }
            }
            for (i, c) in r.checks.iter().enumerate() {
                if shown_checks.get(i) == Some(&false)
                    && !matches!(c.state, CheckState::Waiting | CheckState::Running)
                {
                    shown_checks[i] = true;
                    print_check(out, r, c);
                }
            }
            r.finished()
        });
        if finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    // Evidencia y historial (solo de ejecuciones con candidato).
    if let Ok(store) = &store {
        let (report, evidence, usage) =
            handle.snapshot(|r| (r.report(), r.new_evidence(), r.usage.clone()));
        let _ = store.save_usage(project_dir, &usage);
        if report.candidate.is_some() {
            let _ = store.save_evidence(project_dir, &evidence);
            let _ = store.save_validation(project_dir, &report);
        }
    }

    handle.snapshot(|r| {
        for item in r.items.iter().filter(|i| is_ai(&i.id)) {
            out.line(&format!("  {} {}", level_mark(out, item.level), item.label));
            for d in item.details.iter().take(12) {
                out.line(&out.paint("2", &format!("      {d}")));
            }
        }
        let verdict = r.verdict();
        let (done, total) = r.progress();
        let (text, code) = match verdict {
            Verdict::Ready => (
                out.paint(
                    "32;1",
                    &format!("LISTO PARA {}", stage.label().to_uppercase()),
                ),
                0,
            ),
            Verdict::Warnings => (out.paint("33;1", "LISTO CON ADVERTENCIAS"), 0),
            Verdict::NothingToCheck => (out.paint("2", "SIN CAMBIOS QUE EVALUAR"), 0),
            Verdict::Pending => (
                out.paint(
                    "33;1",
                    "PENDIENTE: faltan revisiones que aún no están disponibles",
                ),
                1,
            ),
            Verdict::Blocked | Verdict::Running => (out.paint("31;1", "BLOQUEADO"), 1),
        };
        out.line(&format!("{text}  ({done} de {total} controles)"));
        if code != 0 {
            let what = match stage {
                Stage::Commit => "El commit no se creó.",
                Stage::Push => "El push no se envió.",
                Stage::PullRequest => "No crees el pull request todavía.",
            };
            if hook {
                out.line(what);
            }
            let rules = r.blocking_rules();
            if !rules.is_empty() {
                out.line(&out.paint("2", "Para omitir una regla, con motivo registrado:"));
                for rule in rules {
                    out.line(&out.paint(
                        "2",
                        &format!(
                            "  forge guard allow {rule} --stage {} --reason \"motivo\"",
                            stage.as_str()
                        ),
                    ));
                }
            }
        }
        Ok(code)
    })
}

/// Controles que se resuelven con IA al final (se imprimen cuando terminan).
fn is_ai(rule: &str) -> bool {
    matches!(rule, "ai-review" | "security-review" | "reviewers-compare")
        || rule.starts_with("reviewer:")
}

fn print_check(out: &Out, r: &GuardRun, c: &guard::CheckRun) {
    let name = c.check.name.clone().unwrap_or_else(|| c.check.id.clone());
    let secs = format!("{:.1} s", c.duration.unwrap_or_default().as_secs_f32()).replace('.', ",");
    let level = r.check_level(c).unwrap_or(Level::Pending);
    let status = match &c.state {
        CheckState::Passed if c.reused.is_some() => {
            format!(
                "evidencia reutilizada ({})",
                store::ago_precise(c.reused.unwrap_or_default())
            )
        }
        CheckState::Passed => secs,
        CheckState::Failed(code) => format!("código {code} · {secs}"),
        CheckState::TimedOut => format!("tiempo agotado ({} s)", c.check.timeout_seconds),
        CheckState::Cancelled => "cancelado".into(),
        CheckState::Error(e) => e.clone(),
        _ => String::new(),
    };
    out.line(&format!("  {} {name} · {status}", level_mark(out, level)));
    if let Some(e) = r
        .exception_for(&GuardRun::check_rule(c))
        .filter(|_| level == Level::Excepted)
    {
        out.line(&out.paint("2", &format!("      {}", e.summary())));
    } else if !matches!(c.state, CheckState::Passed) {
        let lines: Vec<&str> = c.output.lines().collect();
        for l in &lines[lines.len().saturating_sub(20)..] {
            out.line(&out.paint("2", &format!("      {l}")));
        }
    }
}

/// Reglas que se pueden omitir con `allow`.
fn valid_rule(rule: &str, rules: &Rules, project: &Path) -> Result<(), String> {
    let fixed = [
        "lines",
        "files",
        "forbidden",
        "secrets",
        "ai-review",
        "security-review",
    ];
    if fixed.contains(&rule) || rule.starts_with("missing:") {
        return Ok(());
    }
    if let Some(id) = rule.strip_prefix("reviewer:") {
        let reviewers = forge_core::reviewers::load(project)?;
        return match reviewers.iter().any(|r| r.id == id) {
            true => Ok(()),
            false => Err(format!("no hay revisor \"{id}\" (forge ai route)")),
        };
    }
    if let Some(id) = rule.strip_prefix("check:") {
        return match rules.checks.iter().any(|c| c.id == id) {
            true => Ok(()),
            false => Err(format!("no hay [[checks]] con id = \"{id}\"")),
        };
    }
    Err(format!(
        "regla desconocida \"{rule}\". Válidas: {}, check:<id>, missing:<id>, reviewer:<id>",
        fixed.join(", ")
    ))
}

fn allow(out: &Out, project: &Path, args: &[String], rule: Option<&str>) -> Result<i32, String> {
    let rule = rule.ok_or("falta la regla: forge guard allow <regla> --reason \"motivo\"")?;
    let reason = flag(args, "--reason")
        .map(|r| r.trim().to_string())
        .unwrap_or_default();
    if reason.chars().count() < 5 {
        return Err(
            "toda excepción necesita un motivo: --reason \"…\" (al menos 5 caracteres)".into(),
        );
    }
    let scope = match flag(args, "--scope") {
        Some(s) => Scope::parse(&s).ok_or(format!(
            "--scope inválido \"{s}\": commit, pull-request o session"
        ))?,
        None => Scope::Commit,
    };
    let stage = match flag(args, "--stage") {
        Some(s) => {
            Stage::parse(&s).ok_or(format!("--stage inválido \"{s}\": commit, push o pr"))?
        }
        None => Stage::Commit,
    };
    let rules = Rules::load(project)?.unwrap_or_default();
    valid_rule(rule, &rules, project)?;
    let repo = guard::repo_root(project)?;
    // El candidato es el diff de ahora: si cambia, una excepción "commit" deja de valer.
    let diff = guard::collect_diff(&repo, stage, &rules, None)?;
    let exception = Exception {
        id: 0,
        rule: rule.into(),
        reason,
        scope,
        stage,
        candidate: diff.candidate,
        branch: diff.branch,
        user: guard::local_user(&repo),
        created_at: store::now(),
        revoked: false,
    };
    let id = open_store()?.add_exception(project, &exception)?;
    out.line(&format!(
        "{} Excepción #{id} registrada: {rule} · {} · «{}» ({})",
        out.paint("33", "↷"),
        scope.label(),
        exception.reason,
        exception.user
    ));
    Ok(0)
}

fn exceptions(out: &Out, project: &Path) -> Result<i32, String> {
    let list = open_store()?.exceptions(project)?;
    if list.is_empty() {
        out.line("Sin excepciones registradas.");
    }
    for e in list {
        let state = if e.revoked {
            out.paint("2", " [revocada]")
        } else {
            String::new()
        };
        out.line(&format!(
            "#{:<4} {:<18} {:<24} {} · {}{state}\n       «{}»",
            e.id,
            e.rule,
            e.scope.label(),
            e.user,
            store::ago_precise(e.created_at),
            e.reason
        ));
    }
    Ok(0)
}

fn revoke(out: &Out, project: &Path, id: Option<&str>) -> Result<i32, String> {
    let id: i64 = id
        .and_then(|i| i.trim_start_matches('#').parse().ok())
        .ok_or("uso: forge guard revoke <id>")?;
    match open_store()?.revoke_exception(project, id)? {
        true => out.line(&format!(
            "Excepción #{id} revocada (sigue en el historial)."
        )),
        false => return Err(format!("no hay excepción #{id} en este proyecto")),
    }
    Ok(0)
}

fn history(out: &Out, project: &Path) -> Result<i32, String> {
    let list = open_store()?.validations(project, 20)?;
    if list.is_empty() {
        out.line("Sin validaciones todavía (forge guard commit).");
    }
    for (id, r) in list {
        let candidate = r
            .candidate
            .as_ref()
            .map_or("—", |c| c.id.as_str())
            .to_string();
        out.line(&format!(
            "#{id:<4} {:<12} {:<26} {:<24} {}",
            r.stage.label(),
            forge_core::evidence::verdict_label(r.verdict, r.stage),
            candidate,
            store::ago_precise(r.finished_at)
        ));
    }
    Ok(0)
}

fn report(
    out: &Out,
    project: &Path,
    id: Option<&str>,
    output: Option<String>,
) -> Result<i32, String> {
    let list = open_store()?.validations(project, 200)?;
    let wanted: Option<i64> = id
        .map(|i| {
            i.trim_start_matches('#')
                .parse()
                .map_err(|_| format!("id inválido: {i}"))
        })
        .transpose()?;
    let (_, r) = list
        .into_iter()
        .find(|(vid, _)| wanted.is_none_or(|w| w == *vid))
        .ok_or("no hay esa validación (forge guard history)")?;
    let md = r.markdown();
    match output {
        Some(path) => {
            std::fs::write(&path, &md).map_err(|e| format!("{path}: {e}"))?;
            out.line(&format!("Reporte guardado en {path}"));
        }
        None => out.line(&md),
    }
    Ok(0)
}

fn project_agents(project: &Path) -> Vec<forge_core::agents::AgentSpec> {
    forge_core::agents::load(project).unwrap_or_else(|_| forge_core::agents::builtins())
}

fn agent_list(out: &Out, project: &Path) -> Result<i32, String> {
    let agents = forge_core::agents::load(project)?;
    let programs: Vec<String> = agents.iter().map(|a| a.program().to_string()).collect();
    let found = forge_core::agents::detect(&programs);
    let default = forge_core::agents::default_agent(&agents, &found).map(|a| a.id.clone());
    for a in agents.iter().filter(|a| a.enabled) {
        let d = found.get(a.program()).cloned().unwrap_or_default();
        let star = if default.as_deref() == Some(a.id.as_str()) {
            " ★"
        } else {
            ""
        };
        match &d.path {
            Some(path) => out.line(&format!(
                "{} {:<12} {:<22} {}{star}",
                out.paint("32", "●"),
                a.id,
                d.version.unwrap_or_default(),
                path.display()
            )),
            None => out.line(&format!(
                "{} {:<12} {}",
                out.paint("2", "○"),
                a.id,
                out.paint(
                    "2",
                    &format!(
                        "no instalado — {}",
                        a.install_hint.clone().unwrap_or_default()
                    )
                )
            )),
        }
        for cap in a.capabilities() {
            out.line(&out.paint("2", &format!("      {cap}")));
        }
    }
    Ok(0)
}

/// Sustituye este proceso por el agente, en la raíz del proyecto y con su entorno.
fn agent_open(project: &Path, id: Option<&str>, resume: bool) -> Result<i32, String> {
    use std::os::unix::process::CommandExt;
    let id = id.ok_or("uso: forge agent open <id> [--resume]")?;
    let agents = forge_core::agents::load(project)?;
    let agent = agents
        .iter()
        .find(|a| a.id == id)
        .ok_or(format!("agente desconocido \"{id}\" (forge agent list)"))?;
    let command = match (&agent.resume, resume) {
        (Some(r), true) => r.clone(),
        (None, true) => return Err(format!("{} no permite reanudar sesiones", agent.name)),
        _ => agent.command.clone(),
    };
    let root = Project::load(project)
        .map(|p| p.root())
        .unwrap_or_else(|_| project.to_path_buf());
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let err = std::process::Command::new(shell)
        .args(["-l", "-i", "-c", &format!("exec {command}")])
        .current_dir(root)
        .envs(&agent.environment)
        .exec();
    Err(format!("no se pudo abrir {}: {err}", agent.name))
}

fn doctor(out: &Out, project: &Path) -> Result<i32, String> {
    let ok = |cond: bool, text: &str| {
        let mark = if cond {
            out.paint("32", "✓")
        } else {
            out.paint("31", "✕")
        };
        out.line(&format!("{mark} {text}"));
        cond
    };
    let mut healthy = true;
    let git = std::process::Command::new("git").arg("--version").output();
    healthy &= ok(
        git.as_ref().is_ok_and(|o| o.status.success()),
        "git instalado",
    );
    healthy &= ok(
        guard::repo_root(project).is_ok(),
        &format!("{} es un repositorio Git", project.display()),
    );
    match open_store() {
        Ok(_) => ok(
            true,
            &format!(
                "base de datos: {}",
                Store::default_path().unwrap_or_default().display()
            ),
        ),
        Err(e) => {
            healthy = false;
            ok(false, &format!("base de datos: {e}"))
        }
    };
    for (name, result) in [
        (
            "project.toml / layouts.toml / agents.toml",
            Project::load(project).map(drop),
        ),
        ("rules.toml", Rules::load(project).map(drop)),
    ] {
        match result {
            Ok(()) => ok(true, &format!("configuración válida: {name}")),
            Err(e) => {
                healthy = false;
                ok(false, &e)
            }
        };
    }
    match hooks::status(project) {
        Ok(list) => {
            for (name, state) in list {
                let text = match state {
                    HookState::Installed => format!("hook {name} instalado"),
                    HookState::Missing => format!("hook {name} no instalado (forge hooks install)"),
                    HookState::Foreign => {
                        format!("hook {name}: hay otro hook (Forge lo encadenará)")
                    }
                };
                out.line(&format!(
                    "{} {text}",
                    if state == HookState::Installed {
                        out.paint("32", "✓")
                    } else {
                        out.paint("33", "○")
                    }
                ));
            }
        }
        Err(e) => out.line(&format!("{} hooks: {e}", out.paint("33", "○"))),
    }
    let agents = project_agents(project);
    let programs: Vec<String> = agents.iter().map(|a| a.program().to_string()).collect();
    let found = forge_core::agents::detect(&programs);
    let installed: Vec<String> = agents
        .iter()
        .filter(|a| found.get(a.program()).is_some_and(|d| d.path.is_some()))
        .map(|a| a.name.clone())
        .collect();
    healthy &= ok(
        !installed.is_empty(),
        &format!(
            "agentes instalados: {}",
            if installed.is_empty() {
                "ninguno".into()
            } else {
                installed.join(", ")
            }
        ),
    );
    Ok(if healthy { 0 } else { 1 })
}

fn memory_cli(
    out: &Out,
    project: &Path,
    action: &str,
    rest: &[&str],
    args: &[String],
) -> Result<i32, String> {
    let store = open_store()?;
    let kind = flag(args, "--kind");
    let found = match action {
        "search" => store.search_memories(project, &rest.join(" "), kind.as_deref(), 20)?,
        "list" => store.list_memories(project, kind.as_deref(), 30)?,
        "add" => {
            let [k, title, body @ ..] = rest else {
                return Err("uso: forge memory add <tipo> \"título\" \"texto\"".into());
            };
            let tags = flag(args, "--tags").unwrap_or_default();
            let id = store.add_memory(project, k, title, &body.join(" "), &tags, "usuario")?;
            out.line(&format!("Guardada como #{id}."));
            return Ok(0);
        }
        "delete" => {
            let id: i64 = rest
                .first()
                .and_then(|i| i.trim_start_matches('#').parse().ok())
                .ok_or("uso: forge memory delete <id>")?;
            return match store.delete_memory(project, id)? {
                true => {
                    out.line(&format!("Memoria #{id} borrada."));
                    Ok(0)
                }
                false => Err(format!("no existe la memoria #{id}")),
            };
        }
        _ => {
            return Err(format!(
                "acción desconocida \"{action}\": search, add, list o delete"
            ));
        }
    };
    if found.is_empty() {
        out.line("Sin resultados.");
    }
    for m in found {
        out.line(&forge_core::memory::render(&m));
        out.line("");
    }
    Ok(0)
}

/// Explica la decisión del router para el cambio actual, sin llamar a ningún modelo.
fn ai_route(out: &Out, project: &Path, stage: Stage) -> Result<i32, String> {
    use forge_core::router;
    let config = router::RouterConfig::load(project)?;
    let rules = Rules::load(project)?.unwrap_or_default();
    let repo = guard::repo_root(project)?;
    let diff = guard::collect_diff(&repo, stage, &rules, None)?;
    let risk = router::assess(&guard::counted_files(&rules, &diff.files), 0);
    let local = router::local_available();
    out.line(&out.paint(
        "1",
        &format!(
            "Riesgo {} ({} puntos) · {} archivos",
            risk.level.label(),
            risk.score,
            diff.files.len()
        ),
    ));
    for signal in &risk.signals {
        out.line(&format!("  • {signal}"));
    }
    out.line(&format!(
        "Perfil {:?} · local {} · escalado {}",
        config.profile,
        if local {
            "disponible (ollama)"
        } else {
            "no disponible"
        },
        if config.allow_escalation {
            "permitido"
        } else {
            "desactivado"
        }
    ));
    let security = rules.stage(stage).require_security_review;
    for task in router::plan(&config, &risk, false) {
        out.line(&format!(
            "  {}",
            router::route(&config, task, local).label()
        ));
    }
    if security {
        out.line(&format!(
            "  seguridad: {}",
            router::route(&config, router::Task::Security, local).label()
        ));
    }
    out.line(&out.paint(
        "2",
        "El nivel 3 también se activa si hay poca confianza o desacuerdo entre revisores.",
    ));
    if rules.stage(stage).require_ai_review {
        let reviewers = forge_core::reviewers::load(project)?;
        let (active, skipped) = forge_core::reviewers::activate(
            &reviewers,
            &guard::counted_files(&rules, &diff.files),
            &risk,
        );
        out.line(&out.paint("1", "Revisores especializados"));
        for a in &active {
            let kind = if a.reviewer.blocking {
                "bloqueante"
            } else {
                "informativo"
            };
            out.line(&format!(
                "  ✓ {} ({kind}) · {} · {} archivos",
                a.reviewer.name,
                router::reviewer_route(&config, &a.reviewer, local).label(),
                a.files.len()
            ));
        }
        for (name, why) in skipped {
            out.line(&out.paint("2", &format!("  ○ {name}: {why}")));
        }
    }
    let spent = open_store()?.month_spent(project)?;
    out.line(&format!(
        "Gasto del mes: ${spent:.2} de ${:.2}",
        config.monthly_budget_usd
    ));
    Ok(0)
}

fn ai_usage(out: &Out, project: &Path) -> Result<i32, String> {
    let store = open_store()?;
    let rows = store.month_usage(project)?;
    if rows.is_empty() {
        out.line("Sin consumo de modelos este mes.");
    }
    for (provider, model, task, calls, input, output, cost) in rows {
        out.line(&format!("{provider:<10} {model:<14} {task:<15} {calls:>3} llamadas  {input:>9} → {output:<7} tokens  ${cost:.3}"));
    }
    let budget = forge_core::router::RouterConfig::load(project)?.monthly_budget_usd;
    out.line(&format!(
        "Total de pago: ${:.2} de ${budget:.2}",
        store.month_spent(project)?
    ));
    Ok(0)
}

fn hooks_install(out: &Out, project: &Path) -> Result<i32, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let project = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    for line in hooks::install(&project, &exe)? {
        out.line(&line);
    }
    out.line(&out.paint("2", &format!("Los hooks llaman a {}", exe.display())));
    Ok(0)
}

fn hooks_status(out: &Out, project: &Path) -> Result<i32, String> {
    for (name, state) in hooks::status(project)? {
        let text = match state {
            HookState::Installed => out.paint("32", "instalado"),
            HookState::Missing => out.paint("2", "no instalado"),
            HookState::Foreign => out.paint("33", "hay otro hook (Forge lo encadenará)"),
        };
        out.line(&format!("{name}: {text}"));
    }
    Ok(0)
}

fn status(out: &Out, project: &Path) -> Result<i32, String> {
    out.line(&out.paint("1", &format!("Proyecto: {}", project.display())));
    let rules_file = project.join(".forge/rules.toml");
    match Rules::load(project)? {
        Some(_) => out.line(&format!("Reglas: {}", rules_file.display())),
        None => out.line("Reglas: sin .forge/rules.toml (valores por defecto, sin checks)"),
    }
    if let Err(e) = hooks_status(out, project) {
        out.line(&format!("Hooks: {e}"));
    }
    let active = open_store()?
        .exceptions(project)?
        .into_iter()
        .filter(|e| {
            !e.revoked
                && (e.scope != Scope::Session || store::now() - e.created_at < guard::SESSION_SECS)
        })
        .count();
    out.line(&format!(
        "Excepciones activas: {active} (forge guard exceptions)"
    ));
    Ok(0)
}

fn explain(out: &Out, project: &Path, only: Option<Stage>) -> Result<i32, String> {
    let rules = Rules::load(project)?.unwrap_or_default();
    let q = &rules.quality;
    out.line(&out.paint("1", "Calidad"));
    out.line(&format!(
        "  líneas: advertencia > {}, bloqueo > {} · archivos: máximo {}",
        guard::thousands(q.warning_changed_lines),
        guard::thousands(q.max_changed_lines),
        q.max_files_changed
    ));
    out.line(&format!("  no cuentan: {}", q.lines.exclude.join(", ")));
    out.line(&format!(
        "  prohibidos: {}{}",
        if q.block_env_files {
            ".env, .env.*, "
        } else {
            ""
        },
        q.forbidden_files.join(", ")
    ));
    out.line(&format!(
        "  secretos: {}",
        if q.block_secrets {
            "bloquean"
        } else {
            "no se revisan"
        }
    ));
    for stage in Stage::ALL
        .into_iter()
        .filter(|s| only.is_none_or(|o| o == *s))
    {
        let s = rules.stage(stage);
        out.line(&out.paint("1", stage.label()));
        if !s.enabled {
            out.line("  desactivado");
            continue;
        }
        let (checks, missing) = rules.checks_for(stage);
        for c in checks {
            out.line(&format!(
                "  {} — {} (máx. {} s)",
                c.id, c.command, c.timeout_seconds
            ));
        }
        for m in missing {
            out.line(&format!(
                "  {} require_{m} activo pero falta [[checks]] id = \"{m}\"",
                out.paint("31", "✕")
            ));
        }
        if s.require_ai_review {
            out.line("  revisión de IA (pendiente: aún no disponible)");
        }
        if s.require_security_review {
            out.line("  revisión de seguridad (pendiente: aún no disponible)");
        }
    }
    Ok(0)
}
