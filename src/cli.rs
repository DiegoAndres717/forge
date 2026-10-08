// Línea de comandos: `forge guard …` y `forge hooks …` (sin abrir la ventana).
use forge_core::tr;
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
  forge ideas [--all] [--general]          ideas y pendientes (fuera del repositorio)
  forge ideas add \"título\" [\"nota\"] [--general]
  forge ideas done|doing|pending|delete <id> [--general]
  forge mcp                             servidor MCP (memoria y contexto) para agentes

Opciones:
  --project <carpeta>   proyecto (por defecto: la carpeta con .forge/ más cercana o la raíz del repo)
  --hook                modo hook de Git (lo usan los hooks instalados)
";

/// Ayuda en el idioma actual.
pub fn usage() -> &'static str {
    match forge_core::i18n::lang() {
        forge_core::i18n::Lang::En => USAGE_EN,
        forge_core::i18n::Lang::Es => USAGE,
    }
}

pub const USAGE_EN: &str = "\
Forge — project workspace with terminals, agents and Project Guard

Usage:
  forge [folder]                        opens the app (and that project)
  forge guard commit|push|pr            checks whether the project is ready
  forge guard status                    project rules, hooks and exceptions
  forge guard explain [commit|push|pr]  what each stage requires
  forge guard allow <rule> --reason \"reason\" [--scope commit|pull-request|session] [--stage commit|push|pr]
  forge guard exceptions                exception history
  forge guard revoke <id>               disables an exception
  forge guard history                   latest project validations
  forge guard report [id] [--output f]  Markdown report (latest by default)
  forge hooks install|uninstall|status  pre-commit and pre-push hooks
  forge agent list                      detected agents, version and capabilities
  forge agent open <id> [--resume]      opens the project agent in this terminal
  forge doctor                          checks git, database, settings, hooks and agents
  forge ai route [commit|push|pr]       change risk and which models would review it (free)
  forge ai usage                        model usage this month
  forge ai init                         creates .forge/routing.toml
  forge memory search <text> [--kind k]    searches the project memory
  forge memory add <kind> \"title\" \"text\" [--tags a,b]
  forge memory list [--kind k] · forge memory delete <id>
  forge ideas [--all] [--general]          ideas and to-dos (outside the repository)
  forge ideas add \"title\" [\"note\"] [--general]
  forge ideas done|doing|pending|delete <id> [--general]
  forge mcp                             MCP server (memory and context) for agents

Options:
  --project <folder>    project (default: nearest folder with .forge/ or the repo root)
  --hook                Git hook mode (used by the installed hooks)
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
        .ok_or_else(|| tr!("no se encontró $HOME").to_string())
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
        (Some("ideas"), action) => {
            ideas_cli(&out, &project, action.unwrap_or("list"), &pos[2..], args)
        }
        (Some("ai"), Some("init")) => forge_core::router::write_template(&project).map(|_| {
            out.line(&format!(
                "Creado {}",
                project.join(".forge/routing.toml").display()
            ));
            0
        }),
        _ => {
            out.line(usage());
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
        out.line(&tr!(
            "Forge Guard: {p0} desactivado en .forge/rules.toml",
            p0 = stage.label()
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
            out.line(&tr!(
                "{p0} sin excepciones ni evidencias guardadas: {e}",
                p0 = out.paint("33", "⚠"),
                e = e
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
                        tr!(" · checks en copia aislada del candidato")
                    } else {
                        ""
                    };
                    out.line(&out.paint(
                        "2",
                        &tr!("  candidato {p0}{place}", p0 = c.id, place = place),
                    ));
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
                    out.line(&out.paint("2", &tr!("  … ejecutando {p0}", p0 = names.join(", "))));
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
                    &tr!("LISTO PARA {p0}", p0 = stage.label().to_uppercase()),
                ),
                0,
            ),
            Verdict::Warnings => (out.paint("33;1", tr!("LISTO CON ADVERTENCIAS")), 0),
            Verdict::NothingToCheck => (out.paint("2", tr!("SIN CAMBIOS QUE EVALUAR")), 0),
            Verdict::Pending => (
                out.paint(
                    "33;1",
                    tr!("PENDIENTE: faltan revisiones que aún no están disponibles"),
                ),
                1,
            ),
            Verdict::Blocked | Verdict::Running => (out.paint("31;1", tr!("BLOQUEADO")), 1),
        };
        out.line(&tr!(
            "{text}  ({done} de {total} controles)",
            text = text,
            done = done,
            total = total
        ));
        if code != 0 {
            let what = match stage {
                Stage::Commit => tr!("El commit no se creó."),
                Stage::Push => tr!("El push no se envió."),
                Stage::PullRequest => tr!("No crees el pull request todavía."),
            };
            if hook {
                out.line(what);
            }
            let rules = r.blocking_rules();
            if !rules.is_empty() {
                out.line(&out.paint("2", tr!("Para omitir una regla, con motivo registrado:")));
                for rule in rules {
                    out.line(&out.paint(
                        "2",
                        &tr!(
                            "  forge guard allow {rule} --stage {p0} --reason \"motivo\"",
                            p0 = stage.as_str(),
                            rule = rule
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
            tr!(
                "evidencia reutilizada ({p0})",
                p0 = store::ago_precise(c.reused.unwrap_or_default())
            )
        }
        CheckState::Passed => secs,
        CheckState::Failed(code) => tr!("código {code} · {secs}", code = code, secs = secs),
        CheckState::TimedOut => format!("tiempo agotado ({} s)", c.check.timeout_seconds),
        CheckState::Cancelled => tr!("cancelado").into(),
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
            false => Err(tr!("no hay revisor \"{id}\" (forge ai route)", id = id)),
        };
    }
    if let Some(id) = rule.strip_prefix("check:") {
        return match rules.checks.iter().any(|c| c.id == id) {
            true => Ok(()),
            false => Err(tr!("no hay [[checks]] con id = \"{id}\"", id = id)),
        };
    }
    Err(tr!(
        "regla desconocida \"{rule}\". Válidas: {p0}, check:<id>, missing:<id>, reviewer:<id>",
        p0 = fixed.join(", "),
        rule = rule
    ))
}

fn allow(out: &Out, project: &Path, args: &[String], rule: Option<&str>) -> Result<i32, String> {
    let rule = rule.ok_or(tr!(
        "falta la regla: forge guard allow <regla> --reason \"motivo\""
    ))?;
    let reason = flag(args, "--reason")
        .map(|r| r.trim().to_string())
        .unwrap_or_default();
    if reason.chars().count() < 5 {
        return Err(tr!(
            "toda excepción necesita un motivo: --reason \"…\" (al menos 5 caracteres)"
        )
        .into());
    }
    let scope = match flag(args, "--scope") {
        Some(s) => Scope::parse(&s).ok_or(tr!(
            "--scope inválido \"{s}\": commit, pull-request o session",
            s = s
        ))?,
        None => Scope::Commit,
    };
    let stage = match flag(args, "--stage") {
        Some(s) => {
            Stage::parse(&s).ok_or(tr!("--stage inválido \"{s}\": commit, push o pr", s = s))?
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
    out.line(&tr!(
        "{p0} Excepción #{id} registrada: {rule} · {p1} · «{p2}» ({p3})",
        p0 = out.paint("33", "↷"),
        p1 = scope.label(),
        p2 = exception.reason,
        p3 = exception.user,
        id = id,
        rule = rule
    ));
    Ok(0)
}

fn exceptions(out: &Out, project: &Path) -> Result<i32, String> {
    let list = open_store()?.exceptions(project)?;
    if list.is_empty() {
        out.line(tr!("Sin excepciones registradas."));
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
        .ok_or(tr!("uso: forge guard revoke <id>"))?;
    match open_store()?.revoke_exception(project, id)? {
        true => out.line(&tr!(
            "Excepción #{id} revocada (sigue en el historial).",
            id = id
        )),
        false => return Err(tr!("no hay excepción #{id} en este proyecto", id = id)),
    }
    Ok(0)
}

fn history(out: &Out, project: &Path) -> Result<i32, String> {
    let list = open_store()?.validations(project, 20)?;
    if list.is_empty() {
        out.line(tr!("Sin validaciones todavía (forge guard commit)."));
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
                .map_err(|_| tr!("id inválido: {i}", i = i))
        })
        .transpose()?;
    let (_, r) = list
        .into_iter()
        .find(|(vid, _)| wanted.is_none_or(|w| w == *vid))
        .ok_or(tr!("no hay esa validación (forge guard history)"))?;
    let md = r.markdown();
    match output {
        Some(path) => {
            std::fs::write(&path, &md).map_err(|e| format!("{path}: {e}"))?;
            out.line(&tr!("Reporte guardado en {path}", path = path));
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
                    &tr!(
                        "no instalado — {p0}",
                        p0 = a.install_hint.clone().unwrap_or_default()
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
    let id = id.ok_or(tr!("uso: forge agent open <id> [--resume]"))?;
    let agents = forge_core::agents::load(project)?;
    let agent = agents
        .iter()
        .find(|a| a.id == id)
        .ok_or(format!("agente desconocido \"{id}\" (forge agent list)"))?;
    let command = match (&agent.resume, resume) {
        (Some(r), true) => r.clone(),
        (None, true) => return Err(tr!("{p0} no permite reanudar sesiones", p0 = agent.name)),
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
    Err(tr!(
        "no se pudo abrir {p0}: {err}",
        p0 = agent.name,
        err = err
    ))
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
        tr!("git instalado"),
    );
    healthy &= ok(
        guard::repo_root(project).is_ok(),
        &tr!("{p0} es un repositorio Git", p0 = project.display()),
    );
    match open_store() {
        Ok(_) => ok(
            true,
            &tr!(
                "base de datos: {p0}",
                p0 = Store::default_path().unwrap_or_default().display()
            ),
        ),
        Err(e) => {
            healthy = false;
            ok(false, &tr!("base de datos: {e}", e = e))
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
            Ok(()) => ok(true, &tr!("configuración válida: {name}", name = name)),
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
                    HookState::Installed => tr!("hook {name} instalado", name = name),
                    HookState::Missing => tr!(
                        "hook {name} no instalado (forge hooks install)",
                        name = name
                    ),
                    HookState::Foreign => {
                        tr!(
                            "hook {name}: hay otro hook (Forge lo encadenará)",
                            name = name
                        )
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
        Err(e) => out.line(&tr!("{p0} hooks: {e}", p0 = out.paint("33", "○"), e = e)),
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
        &tr!(
            "agentes instalados: {p0}",
            p0 = if installed.is_empty() {
                "ninguno".into()
            } else {
                installed.join(", ")
            }
        ),
    );
    Ok(if healthy { 0 } else { 1 })
}

fn ideas_cli(
    out: &Out,
    project: &Path,
    action: &str,
    rest: &[&str],
    args: &[String],
) -> Result<i32, String> {
    let store = open_store()?;
    let list = (!args.iter().any(|a| a == "--general")).then_some(project);
    let id = || -> Result<i64, String> {
        rest.first()
            .and_then(|i| i.trim_start_matches('#').parse().ok())
            .ok_or_else(|| tr!("uso: forge ideas {action} <id>", action = action))
    };
    match action {
        "list" => {
            let ideas = store.list_ideas(list, args.iter().any(|a| a == "--all"))?;
            if ideas.is_empty() {
                out.line(tr!("No hay ideas pendientes."));
            }
            for idea in ideas {
                out.line(&forge_core::ideas::render(&idea));
            }
        }
        "add" => {
            let [title, note @ ..] = rest else {
                return Err(tr!("uso: forge ideas add \"título\" [\"nota\"]").into());
            };
            let id = store.add_idea(list, title, &note.join(" "), "usuario")?;
            out.line(&format!("Anotada como idea #{id}."));
        }
        "done" | "doing" | "pending" => {
            let id = id()?;
            if !store.update_idea(list, id, Some(action), None, None, "usuario")? {
                return Err(tr!("no existe la idea #{id} en esta lista", id = id));
            }
            out.line(&format!("Idea #{id}: {action}."));
        }
        "delete" => {
            let id = id()?;
            if !store.delete_idea(list, id)? {
                return Err(tr!("no existe la idea #{id} en esta lista", id = id));
            }
            out.line(&format!("Idea #{id} borrada."));
        }
        _ => {
            return Err(tr!(
                "acción desconocida \"{action}\": list, add, done, doing, pending o delete",
                action = action
            ));
        }
    }
    Ok(0)
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
                return Err(tr!("uso: forge memory add <tipo> \"título\" \"texto\"").into());
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
                .ok_or(tr!("uso: forge memory delete <id>"))?;
            return match store.delete_memory(project, id)? {
                true => {
                    out.line(&tr!("Memoria #{id} borrada.", id = id));
                    Ok(0)
                }
                false => Err(tr!("no existe la memoria #{id}", id = id)),
            };
        }
        _ => {
            return Err(tr!(
                "acción desconocida \"{action}\": search, add, list o delete",
                action = action
            ));
        }
    };
    if found.is_empty() {
        out.line(tr!("Sin resultados."));
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
        &tr!(
            "Riesgo {p0} ({p1} puntos) · {p2} archivos",
            p0 = risk.level.label(),
            p1 = risk.score,
            p2 = diff.files.len()
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
            tr!("no disponible")
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
        tr!("El nivel 3 también se activa si hay poca confianza o desacuerdo entre revisores."),
    ));
    if rules.stage(stage).require_ai_review {
        let reviewers = forge_core::reviewers::load(project)?;
        let (active, skipped) = forge_core::reviewers::activate(
            &reviewers,
            &guard::counted_files(&rules, &diff.files),
            &risk,
        );
        out.line(&out.paint("1", tr!("Revisores especializados")));
        for a in &active {
            let kind = if a.reviewer.blocking {
                "bloqueante"
            } else {
                "informativo"
            };
            out.line(&tr!(
                "  ✓ {p0} ({kind}) · {p1} · {p2} archivos",
                p0 = a.reviewer.name,
                p1 = router::reviewer_route(&config, &a.reviewer, local).label(),
                p2 = a.files.len(),
                kind = kind
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
        out.line(tr!("Sin consumo de modelos este mes."));
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
    out.line(&out.paint("2", &tr!("Los hooks llaman a {p0}", p0 = exe.display())));
    Ok(0)
}

fn hooks_status(out: &Out, project: &Path) -> Result<i32, String> {
    for (name, state) in hooks::status(project)? {
        let text = match state {
            HookState::Installed => out.paint("32", "instalado"),
            HookState::Missing => out.paint("2", tr!("no instalado")),
            HookState::Foreign => out.paint("33", tr!("hay otro hook (Forge lo encadenará)")),
        };
        out.line(&format!("{name}: {text}"));
    }
    Ok(0)
}

fn status(out: &Out, project: &Path) -> Result<i32, String> {
    out.line(&out.paint("1", &tr!("Proyecto: {p0}", p0 = project.display())));
    let rules_file = project.join(".forge/rules.toml");
    match Rules::load(project)? {
        Some(_) => out.line(&tr!("Reglas: {p0}", p0 = rules_file.display())),
        None => out.line(tr!(
            "Reglas: sin .forge/rules.toml (valores por defecto, sin checks)"
        )),
    }
    if let Err(e) = hooks_status(out, project) {
        out.line(&tr!("Hooks: {e}", e = e));
    }
    let active = open_store()?
        .exceptions(project)?
        .into_iter()
        .filter(|e| {
            !e.revoked
                && (e.scope != Scope::Session || store::now() - e.created_at < guard::SESSION_SECS)
        })
        .count();
    out.line(&tr!(
        "Excepciones activas: {active} (forge guard exceptions)",
        active = active
    ));
    Ok(0)
}

fn explain(out: &Out, project: &Path, only: Option<Stage>) -> Result<i32, String> {
    let rules = Rules::load(project)?.unwrap_or_default();
    let q = &rules.quality;
    out.line(&out.paint("1", "Calidad"));
    out.line(&tr!(
        "  líneas: advertencia > {p0}, bloqueo > {p1} · archivos: máximo {p2}",
        p0 = guard::thousands(q.warning_changed_lines),
        p1 = guard::thousands(q.max_changed_lines),
        p2 = q.max_files_changed
    ));
    out.line(&tr!("  no cuentan: {p0}", p0 = q.lines.exclude.join(", ")));
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
            tr!("no se revisan")
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
            out.line(&tr!(
                "  {p0} — {p1} (máx. {p2} s)",
                p0 = c.id,
                p1 = c.command,
                p2 = c.timeout_seconds
            ));
        }
        for m in missing {
            out.line(&tr!(
                "  {p0} require_{m} activo pero falta [[checks]] id = \"{m}\"",
                p0 = out.paint("31", "✕"),
                m = m
            ));
        }
        if s.require_ai_review {
            out.line(tr!("  revisión de IA (pendiente: aún no disponible)"));
        }
        if s.require_security_review {
            out.line(tr!(
                "  revisión de seguridad (pendiente: aún no disponible)"
            ));
        }
    }
    Ok(0)
}
