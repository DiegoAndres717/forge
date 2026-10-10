// Router de modelos: decide qué modelo revisa un cambio según su riesgo, escalando
// de barato a potente solo cuando hace falta, con presupuesto y registro de consumo.
// Los modelos se llaman a través de los CLIs del usuario (claude -p, codex exec, opencode
// run, ollama), así que no hacen falta claves de API.
use crate::tr;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::guard::{FileChange, Rules, Stage};

// ---------------------------------------------------------------- configuración

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Economy,
    #[default]
    Balanced,
    Quality,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
struct RoutingFile {
    routing: RouterConfig,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RouterConfig {
    pub profile: Profile,
    pub prefer_local: bool,
    pub monthly_budget_usd: f64,
    pub allow_escalation: bool,
    /// Tope por llamada (se pasa a `claude --max-budget-usd`).
    pub max_cost_per_call_usd: f64,
    pub timeout_seconds: u64,
    pub tasks: HashMap<String, TaskRoute>,
    /// Proveedores propios o cambios a los conocidos.
    pub providers: HashMap<String, ProviderDef>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            profile: Profile::Balanced,
            prefer_local: true,
            monthly_budget_usd: 20.0,
            allow_escalation: true,
            max_cost_per_call_usd: 0.50,
            timeout_seconds: 240,
            tasks: HashMap::new(),
            providers: HashMap::new(),
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskRoute {
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderDef {
    /// Plantilla de comando (sh). `{model}` y `{effort}` se sustituyen; el prompt llega por stdin.
    pub command: String,
    #[serde(default)]
    pub format: OutputFormat,
    /// Coste fijo estimado por llamada cuando el proveedor no lo informa.
    #[serde(default)]
    pub cost_per_call_usd: f64,
    /// Proveedor local: no cuenta para el presupuesto.
    #[serde(default)]
    pub local: bool,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    #[default]
    Text,
    ClaudeJson,
    CodexJson,
}

impl RouterConfig {
    /// Lee `.forge/routing.toml`; sin archivo, valores por defecto.
    pub fn load(project: &Path) -> Result<Self, String> {
        let path = project.join(".forge/routing.toml");
        match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
            Ok(text) => toml::from_str::<RoutingFile>(&text)
                .map(|f| f.routing)
                .map_err(|e| format!("{}: {e}", path.display())),
        }
    }

    fn provider(&self, name: &str) -> Option<ProviderDef> {
        if let Some(p) = self.providers.get(name) {
            return Some(p.clone());
        }
        let builtin = |command: &str, format, local| ProviderDef {
            command: command.into(),
            format,
            cost_per_call_usd: 0.0,
            local,
        };
        Some(match name {
            "anthropic" | "claude" => builtin(
                "claude -p --model {model} --effort {effort} --output-format json --no-session-persistence --tools '' --max-budget-usd {max_cost}",
                OutputFormat::ClaudeJson,
                false,
            ),
            "openai" | "codex" => builtin(
                "codex exec --json -s read-only --skip-git-repo-check -m {model} -c model_reasoning_effort={effort} -",
                OutputFormat::CodexJson,
                false,
            ),
            "opencode" => builtin(
                "opencode run -m {model} --variant {effort} \"$(cat)\"",
                OutputFormat::Text,
                false,
            ),
            "local" | "ollama" => builtin("ollama run {model}", OutputFormat::Text, true),
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------- tareas y rutas

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Task {
    Classification,
    CodeReview,
    Architecture,
    Security,
}

impl Task {
    pub fn key(self) -> &'static str {
        match self {
            Task::Classification => "classification",
            Task::CodeReview => "code_review",
            Task::Architecture => "architecture",
            Task::Security => "security",
        }
    }

    /// Nivel del plan (14.1): 1 barato, 2 intermedio, 3 potente.
    pub fn tier(self) -> u8 {
        match self {
            Task::Classification => 1,
            Task::CodeReview => 2,
            Task::Architecture | Task::Security => 3,
        }
    }

    /// Caracteres de diff que recibe cada nivel (reducción de contexto).
    fn context_budget(self) -> usize {
        match self.tier() {
            1 => 20_000,
            2 => 60_000,
            _ => 120_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub task: Task,
    pub provider: String,
    pub model: String,
    pub effort: String,
}

impl Route {
    pub fn label(&self) -> String {
        format!(
            "nivel {} · {} {} ({})",
            self.task.tier(),
            self.provider,
            self.model,
            self.effort
        )
    }
}

/// Ruta de una tarea: la de routing.toml o la por defecto del perfil. Con `prefer_local`
/// y ollama disponible, la clasificación va a un modelo local.
pub fn route(config: &RouterConfig, task: Task, local_available: bool) -> Route {
    if let Some(t) = config.tasks.get(task.key()) {
        let local = matches!(t.provider.as_str(), "local" | "ollama");
        if !local || local_available {
            return Route {
                task,
                provider: t.provider.clone(),
                model: t.model.clone(),
                effort: t.effort.clone().unwrap_or_else(|| "medium".into()),
            };
        }
    }
    let (provider, model, effort) = match (config.profile, task) {
        (_, Task::Classification) if config.prefer_local && local_available => {
            ("local", "qwen2.5-coder", "low")
        }
        (_, Task::Classification) => ("anthropic", "haiku", "low"),
        (Profile::Quality, Task::CodeReview) => ("anthropic", "sonnet", "medium"),
        (_, Task::CodeReview) => ("anthropic", "haiku", "medium"),
        (Profile::Economy, _) => ("anthropic", "haiku", "high"),
        (Profile::Quality, _) => ("anthropic", "opus", "high"),
        (Profile::Balanced, _) => ("anthropic", "sonnet", "high"),
    };
    Route {
        task,
        provider: provider.into(),
        model: model.into(),
        effort: effort.into(),
    }
}

// ---------------------------------------------------------------- riesgo

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn label(self) -> &'static str {
        crate::i18n::t(match self {
            RiskLevel::Low => "bajo",
            RiskLevel::Medium => "medio",
            RiskLevel::High => "alto",
            RiskLevel::Critical => "crítico",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Risk {
    pub score: u32,
    pub level: RiskLevel,
    pub signals: Vec<String>,
    /// Hay señales de seguridad (auth, permisos, pagos, cripto, entorno).
    pub security: bool,
}

/// Áreas críticas (plan 14.2): (nombre, palabras en la ruta, peso, es de seguridad).
/// Palabras en rutas que indican un área de seguridad (también las usa el revisor de seguridad).
pub fn security_words() -> Vec<&'static str> {
    AREAS
        .iter()
        .filter(|a| a.3)
        .flat_map(|a| a.1.iter().copied())
        .collect()
}

const AREAS: [(&str, &[&str], u32, bool); 8] = [
    (
        "autenticación",
        &["auth", "login", "session", "oauth", "sso"],
        3,
        true,
    ),
    (
        "permisos",
        &[
            "permission",
            "rbac",
            "acl",
            "policy",
            "policies",
            "role",
            "rls",
        ],
        3,
        true,
    ),
    (
        "pagos",
        &[
            "payment", "billing", "checkout", "stripe", "wompi", "invoice",
        ],
        3,
        true,
    ),
    (
        "criptografía",
        &["crypto", "jwt", "token", "secret", "encrypt", "hash"],
        3,
        true,
    ),
    (
        "migraciones",
        &["migration", "drizzle", "schema.sql", "prisma/schema"],
        2,
        false,
    ),
    (
        "infraestructura",
        &[
            "dockerfile",
            "docker-compose",
            "terraform",
            ".github/workflows",
            "k8s",
            "helm",
            "nginx",
        ],
        2,
        false,
    ),
    (
        "variables de entorno",
        &[".env", "config/env", "env.ts", "settings."],
        2,
        true,
    ),
    (
        "API pública",
        &["routes/", "/api/", "openapi", "graphql", "proto"],
        1,
        false,
    ),
];

pub fn assess(files: &[FileChange], failed_checks: usize) -> Risk {
    let mut score = 0;
    let mut signals = Vec::new();
    let mut security = false;
    let lines: usize = files.iter().map(|f| f.added + f.deleted).sum();
    if files.len() > 20 {
        score += 2;
        signals.push(tr!("{p0} archivos cambiados", p0 = files.len()));
    }
    match lines {
        2001.. => {
            score += 3;
            signals.push(tr!("{lines} líneas", lines = lines));
        }
        501.. => {
            score += 2;
            signals.push(tr!("{lines} líneas", lines = lines));
        }
        _ => {}
    }
    for (name, words, weight, sec) in AREAS {
        let hits: Vec<&str> = files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| {
                let p = p.to_lowercase();
                words.iter().any(|w| p.contains(w))
            })
            .collect();
        if let Some(first) = hits.first() {
            score += weight;
            security |= sec;
            let more = if hits.len() > 1 {
                tr!(" y {p0} más", p0 = hits.len() - 1)
            } else {
                String::new()
            };
            signals.push(tr!(
                "toca {name} ({first}{more})",
                name = crate::i18n::t(name),
                first = first,
                more = more
            ));
        }
    }
    if failed_checks > 0 {
        score += 2;
        signals.push(format!("{failed_checks} checks fallidos"));
    }
    let level = match score {
        0..=1 => RiskLevel::Low,
        2..=3 => RiskLevel::Medium,
        4..=6 => RiskLevel::High,
        _ => RiskLevel::Critical,
    };
    Risk {
        score,
        level,
        signals,
        security,
    }
}

/// Tareas a ejecutar según perfil y riesgo (el escalado real depende además de las respuestas).
pub fn plan(config: &RouterConfig, risk: &Risk, security_required: bool) -> Vec<Task> {
    if security_required {
        return vec![Task::Security];
    }
    let mut tasks = Vec::new();
    // Quality no se fía de la clasificación barata: revisa siempre con el nivel 2.
    if config.profile != Profile::Quality {
        tasks.push(Task::Classification);
    }
    tasks.push(Task::CodeReview);
    let threshold = if config.profile == Profile::Quality {
        RiskLevel::Medium
    } else {
        RiskLevel::High
    };
    if config.allow_escalation && config.profile != Profile::Economy && risk.level >= threshold {
        tasks.push(if risk.security {
            Task::Security
        } else {
            Task::Architecture
        });
    }
    tasks
}

// ---------------------------------------------------------------- contexto

/// Pathspecs de git que excluyen archivos generados y sensibles del contexto del modelo.
fn exclude_pathspecs(rules: &Rules) -> Vec<String> {
    let q = &rules.quality;
    let sensitive = [
        ".env",
        ".env.*",
        "*.pem",
        "*.key",
        "credentials.json",
        "secrets/**",
        "id_rsa*",
    ];
    q.lines
        .exclude
        .iter()
        .chain(q.forbidden_files.iter())
        .map(String::as_str)
        .chain(sensitive)
        .map(|p| {
            let p = p.trim_start_matches("./");
            if p.contains('/') {
                format!(":(exclude,glob){p}")
            } else {
                format!(":(exclude,glob)**/{p}")
            }
        })
        .collect()
}

/// Diff con el contexto de cada función tocada (`-W`), sin generados ni sensibles,
/// con los secretos ocultos y recortado al presupuesto del nivel.
/// `only`: limitar a esos archivos (revisores especializados); vacío = todo el diff.
pub fn reduced_diff(
    repo: &Path,
    range: &[String],
    rules: &Rules,
    budget: usize,
    only: &[String],
) -> Result<String, String> {
    let mut args: Vec<String> = [
        "-c",
        "core.quotepath=off",
        "diff",
        "-W",
        "--no-color",
        "--no-ext-diff",
    ]
    .map(String::from)
    .to_vec();
    args.extend(range.iter().cloned());
    args.push("--".into());
    if only.is_empty() {
        args.push(".".into());
    } else {
        args.extend(only.iter().map(|p| format!(":(literal){p}")));
    }
    args.extend(exclude_pathspecs(rules));
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let diff = crate::guard::mask_secrets(&String::from_utf8_lossy(&out.stdout));
    Ok(truncate(&diff, budget))
}

fn truncate(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    tr!(
        "{p0}\n[… recortado: {p1} caracteres omitidos para ahorrar contexto]",
        p0 = &text[..end],
        p1 = text.len() - end
    )
}

/// Contexto común a todas las revisiones (resultado determinista + diff reducido).
pub struct ReviewContext {
    pub project: String,
    pub stage: Stage,
    pub risk: Risk,
    /// Resumen de los controles deterministas ("✓ Lint", "✓ Tests"…).
    pub deterministic: Vec<String>,
    pub files: Vec<FileChange>,
    pub repo: std::path::PathBuf,
    pub range: Vec<String>,
}

fn prompt(task: Task, ctx: &ReviewContext, diff: &str) -> String {
    prompt_with(role(task), ctx, diff)
}

fn role(task: Task) -> &'static str {
    match task {
        Task::Classification => "Clasifica el riesgo de este cambio. Sé breve.",
        Task::CodeReview => {
            "Revisa este cambio buscando errores, regresiones y problemas de calidad concretos."
        }
        Task::Architecture => {
            "Revisa la arquitectura de este cambio: diseño, acoplamiento, riesgos de mantenimiento y regresiones."
        }
        Task::Security => {
            "Revisa la seguridad de este cambio: autenticación, permisos, inyección, secretos, datos sensibles y pagos."
        }
    }
}

fn prompt_with(role: &str, ctx: &ReviewContext, diff: &str) -> String {
    let files: Vec<String> = ctx
        .files
        .iter()
        .map(|f| format!("- {} (+{} −{})", f.path, f.added, f.deleted))
        .collect();
    format!(
        "Eres un revisor de código senior. {role}\n\
         Proyecto: {} · etapa: {}.\n\
         Riesgo determinista: {} (puntos {}). Señales: {}.\n\
         Controles deterministas ya superados: {}.\n\
         Archivos cambiados:\n{}\n\n\
         Diff (contexto de las funciones tocadas; archivos generados y sensibles excluidos; secretos ocultos):\n\
         ```diff\n{diff}\n```\n\n\
         Responde SOLO con un objeto JSON, sin texto adicional, con esta forma:\n\
         {{\"verdict\": \"approve\" | \"changes\" | \"block\", \"confidence\": 0.0-1.0, \"risk\": \"low\" | \"medium\" | \"high\", \
         \"summary\": \"una o dos frases en español\", \
         \"findings\": [{{\"severity\": \"info\" | \"warning\" | \"blocking\", \"file\": \"ruta\", \"line\": 0, \"message\": \"en español\"}}]}}\n\
         Usa \"block\" solo para defectos graves (seguridad, pérdida de datos, roturas seguras).",
        ctx.project,
        ctx.stage.label(),
        ctx.risk.level.label(),
        ctx.risk.score,
        if ctx.risk.signals.is_empty() {
            "ninguna".into()
        } else {
            ctx.risk.signals.join("; ")
        },
        if ctx.deterministic.is_empty() {
            "ninguno".into()
        } else {
            ctx.deterministic.join(", ")
        },
        files.join("\n"),
    )
}

// ---------------------------------------------------------------- ejecución

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: u64,
    #[serde(default)]
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelReview {
    pub verdict: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub risk: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub task: Task,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub local: bool,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub route: String,
    pub task: Task,
    pub review: Option<ModelReview>,
    pub error: Option<String>,
    pub usage: Option<Usage>,
    pub seconds: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AiVerdict {
    Approve,
    Changes,
    Block,
    /// No se pudo completar (presupuesto, error del proveedor, respuesta inválida).
    Incomplete,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiOutcome {
    pub verdict: AiVerdict,
    pub summary: String,
    pub findings: Vec<Finding>,
    pub steps: Vec<Step>,
    pub reason: Option<String>,
}

impl AiOutcome {
    pub fn cost(&self) -> f64 {
        self.steps
            .iter()
            .filter_map(|s| s.usage.as_ref())
            .map(|u| u.cost_usd)
            .sum()
    }

    /// Líneas para mostrar (rastro de escalado y hallazgos).
    pub fn details(&self) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.steps {
            let cost = s
                .usage
                .as_ref()
                .map_or(String::new(), |u| format!(" · ${:.3}", u.cost_usd));
            match (&s.review, &s.error) {
                (Some(r), _) => out.push(format!(
                    "{}: {} (confianza {:.2}){cost} — {}",
                    s.route, r.verdict, r.confidence, r.summary
                )),
                (None, Some(e)) => out.push(tr!("{p0}: error — {e}", p0 = s.route, e = e)),
                _ => {}
            }
        }
        for f in &self.findings {
            let at = if f.file.is_empty() {
                String::new()
            } else {
                format!("{}:{} ", f.file, f.line)
            };
            out.push(format!("[{}] {at}{}", f.severity, f.message));
        }
        if let Some(r) = &self.reason {
            out.push(r.clone());
        }
        out
    }
}

/// Saca el primer objeto JSON del texto (los modelos a veces lo envuelven en ```json).
pub fn parse_review(text: &str) -> Option<ModelReview> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let review: ModelReview = serde_json::from_str(text.get(start..=end)?).ok()?;
    matches!(review.verdict.as_str(), "approve" | "changes" | "block").then_some(review)
}

/// Texto y consumo según el formato del proveedor.
fn parse_output(
    format: OutputFormat,
    stdout: &str,
) -> Result<(String, u64, u64, Option<f64>), String> {
    match format {
        OutputFormat::Text => Ok((stdout.to_string(), 0, 0, None)),
        OutputFormat::ClaudeJson => {
            let v: serde_json::Value = serde_json::from_str(stdout.trim())
                .map_err(|e| tr!("salida de claude no válida: {e}", e = e))?;
            if v["is_error"].as_bool() == Some(true) {
                return Err(v["result"]
                    .as_str()
                    .unwrap_or(tr!("error de claude"))
                    .to_string());
            }
            let u = &v["usage"];
            let input = u["input_tokens"].as_u64().unwrap_or(0)
                + u["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                + u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            Ok((
                v["result"].as_str().unwrap_or_default().to_string(),
                input,
                u["output_tokens"].as_u64().unwrap_or(0),
                v["total_cost_usd"].as_f64(),
            ))
        }
        OutputFormat::CodexJson => {
            // Eventos JSONL: el último mensaje del agente y el uso del turno.
            let (mut text, mut input, mut output) = (String::new(), 0, 0);
            for line in stdout.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                if let Some(t) = v["item"]["text"]
                    .as_str()
                    .filter(|_| v["item"]["type"] == "agent_message")
                {
                    text = t.to_string();
                }
                if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                    input += u["input_tokens"].as_u64().unwrap_or(0);
                    output += u["output_tokens"].as_u64().unwrap_or(0);
                }
            }
            if text.is_empty() {
                return Err(tr!("codex no devolvió ningún mensaje").into());
            }
            Ok((text, input, output, None))
        }
    }
}

/// Ejecuta una tarea con su proveedor: el prompt va por stdin, con tiempo límite.
fn call(
    config: &RouterConfig,
    route: &Route,
    prompt: &str,
    cwd: &Path,
) -> Result<(ModelReview, Usage), String> {
    let provider = config
        .provider(&route.provider)
        .ok_or(format!("proveedor desconocido \"{}\"", route.provider))?;
    let command = provider
        .command
        .replace("{model}", &shell_word(&route.model))
        .replace("{effort}", &shell_word(&route.effort))
        .replace(
            "{max_cost}",
            &format!("{:.2}", config.max_cost_per_call_usd),
        );
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut child = Command::new(shell)
        .args(["-l", "-c", &command])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or(tr!("sin stdin"))?;
    let input = prompt.to_string();
    std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take().ok_or(tr!("sin stdout"))?;
    let mut stderr = child.stderr.take().ok_or(tr!("sin stderr"))?;
    let out_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut s);
        s
    });
    let deadline = Instant::now() + Duration::from_secs(config.timeout_seconds.max(5));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(tr!("sin respuesta en {p0} s", p0 = config.timeout_seconds));
            }
        }
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if !status.success() && stdout.trim().is_empty() {
        let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
        return Err(tr!(
            "el proveedor falló: {p0}",
            p0 = tail.into_iter().rev().collect::<Vec<_>>().join(" ")
        ));
    }
    let (text, input_tokens, output_tokens, cost) = parse_output(provider.format, &stdout)?;
    let usage = Usage {
        task: route.task,
        provider: route.provider.clone(),
        model: route.model.clone(),
        input_tokens,
        output_tokens,
        cost_usd: cost.unwrap_or(provider.cost_per_call_usd),
        local: provider.local,
        created_at: crate::store::now(),
    };
    let review =
        parse_review(&text).ok_or_else(|| tr!("la respuesta no es el JSON pedido").to_string());
    review.map(|r| (r, usage))
}

fn shell_word(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// ¿Hay modelo local? (ollama en el PATH del shell de login).
pub fn local_available() -> bool {
    crate::agents::detect(&["ollama".into()])
        .get("ollama")
        .is_some_and(|d| d.path.is_some())
}

/// Revisión con escalado (plan 14.1). `month_spent`: gasto del mes en proveedores de pago.
/// `on_step` informa del avance ("nivel 2 · anthropic haiku…").
pub fn review(
    config: &RouterConfig,
    rules: &Rules,
    ctx: &ReviewContext,
    security_required: bool,
    month_spent: f64,
    local: bool,
    on_step: &dyn Fn(&str),
) -> AiOutcome {
    let mut spent = month_spent;
    let mut steps: Vec<Step> = Vec::new();
    let mut last: Option<ModelReview> = None;
    let tasks = plan(config, &ctx.risk, security_required);
    let mut escalate_extra = false;

    for (i, task) in tasks.iter().copied().enumerate() {
        let chosen = route(config, task, local);
        // El nivel 3 solo si el riesgo lo pide o los niveles previos no convencen.
        if task.tier() == 3 && !security_required {
            let convinced = last
                .as_ref()
                .is_some_and(|r| r.verdict == "approve" && r.confidence >= 0.7);
            if convinced && ctx.risk.level < RiskLevel::High {
                break;
            }
        }
        let paid = config.provider(&chosen.provider).is_some_and(|p| !p.local);
        if paid && spent + config.max_cost_per_call_usd > config.monthly_budget_usd {
            return finish(
                steps,
                Some(format!(
                    "presupuesto mensual agotado (${spent:.2} de ${:.2}); sube monthly_budget_usd o usa un modelo local",
                    config.monthly_budget_usd
                )),
            );
        }
        on_step(&chosen.label());
        let diff = match reduced_diff(&ctx.repo, &ctx.range, rules, task.context_budget(), &[]) {
            Ok(d) => d,
            Err(e) => return finish(steps, Some(tr!("no se pudo preparar el diff: {e}", e = e))),
        };
        let started = Instant::now();
        let result = call(config, &chosen, &prompt(task, ctx, &diff), &ctx.repo);
        let seconds = started.elapsed().as_secs_f64();
        match result {
            Ok((review, usage)) => {
                if !usage.local {
                    spent += usage.cost_usd;
                }
                // Desacuerdo entre niveles o poca confianza → se pide el nivel siguiente.
                if let Some(prev) = &last
                    && prev.verdict != review.verdict
                {
                    escalate_extra = true;
                }
                if review.confidence < 0.6 {
                    escalate_extra = true;
                }
                steps.push(Step {
                    route: chosen.label(),
                    task,
                    review: Some(review.clone()),
                    error: None,
                    usage: Some(usage),
                    seconds,
                });
                last = Some(review);
            }
            Err(e) => {
                steps.push(Step {
                    route: chosen.label(),
                    task,
                    review: None,
                    error: Some(e),
                    usage: None,
                    seconds,
                });
                escalate_extra = true;
            }
        }
        // Escalado extra (fuera del plan inicial) por desacuerdo, error o baja confianza.
        let is_last = i + 1 == tasks.len();
        if is_last
            && escalate_extra
            && config.allow_escalation
            && config.profile != Profile::Economy
            && task.tier() < 3
        {
            let extra = if ctx.risk.security {
                Task::Security
            } else {
                Task::Architecture
            };
            let route = route(config, extra, local);
            let paid = config.provider(&route.provider).is_some_and(|p| !p.local);
            if paid && spent + config.max_cost_per_call_usd > config.monthly_budget_usd {
                return finish(
                    steps,
                    Some(tr!("no se escaló: presupuesto mensual agotado").into()),
                );
            }
            on_step(&route.label());
            let diff = reduced_diff(&ctx.repo, &ctx.range, rules, extra.context_budget(), &[])
                .unwrap_or_default();
            let started = Instant::now();
            match call(config, &route, &prompt(extra, ctx, &diff), &ctx.repo) {
                Ok((review, usage)) => {
                    steps.push(Step {
                        route: route.label(),
                        task: extra,
                        review: Some(review.clone()),
                        error: None,
                        usage: Some(usage),
                        seconds: started.elapsed().as_secs_f64(),
                    });
                    last = Some(review);
                }
                Err(e) => steps.push(Step {
                    route: route.label(),
                    task: extra,
                    review: None,
                    error: Some(e),
                    usage: None,
                    seconds: started.elapsed().as_secs_f64(),
                }),
            }
        }
    }
    finish(steps, None)
}

/// Ruta de un revisor especializado: su nivel y, si los fija, su proveedor/modelo/esfuerzo.
pub fn reviewer_route(
    config: &RouterConfig,
    reviewer: &crate::reviewers::ReviewerSpec,
    local: bool,
) -> Route {
    use crate::reviewers::Tier;
    let task = match reviewer.tier {
        Tier::Cheap => Task::Classification,
        Tier::Balanced => Task::CodeReview,
        Tier::Powerful if reviewer.id == "security" => Task::Security,
        Tier::Powerful => Task::Architecture,
    };
    let mut r = route(config, task, local);
    if let Some(p) = &reviewer.provider {
        r.provider = p.clone();
    }
    if let Some(m) = &reviewer.model {
        r.model = m.clone();
    }
    if let Some(e) = &reviewer.effort {
        r.effort = e.clone();
    }
    r
}

/// Una revisión especializada (sin escalado): solo los archivos de su área y su enfoque.
pub fn specialist(
    config: &RouterConfig,
    rules: &Rules,
    ctx: &ReviewContext,
    activation: &crate::reviewers::Activation,
    month_spent: f64,
    local: bool,
) -> AiOutcome {
    let reviewer = &activation.reviewer;
    let chosen = reviewer_route(config, reviewer, local);
    let incomplete = |steps: Vec<Step>, reason: String| AiOutcome {
        verdict: AiVerdict::Incomplete,
        summary: String::new(),
        findings: Vec::new(),
        steps,
        reason: Some(reason),
    };
    let paid = config.provider(&chosen.provider).is_some_and(|p| !p.local);
    if paid && month_spent + config.max_cost_per_call_usd > config.monthly_budget_usd {
        return incomplete(Vec::new(), "presupuesto mensual agotado".into());
    }
    let diff = match reduced_diff(
        &ctx.repo,
        &ctx.range,
        rules,
        chosen.task.context_budget(),
        &activation.files,
    ) {
        Ok(d) => d,
        Err(e) => return incomplete(Vec::new(), tr!("no se pudo preparar el diff: {e}", e = e)),
    };
    let role = tr!(
        "Eres el {p0}. Revisa SOLO {p1}. Ignora lo que no sea de tu área y no repitas observaciones generales.",
        p0 = reviewer.name.to_lowercase(),
        p1 = reviewer.focus
    );
    let started = Instant::now();
    let label = format!("{} · {} {}", reviewer.name, chosen.provider, chosen.model);
    match call(config, &chosen, &prompt_with(&role, ctx, &diff), &ctx.repo) {
        Ok((review, usage)) => {
            let verdict = match review.verdict.as_str() {
                "approve" => AiVerdict::Approve,
                "block" => AiVerdict::Block,
                _ => AiVerdict::Changes,
            };
            let step = Step {
                route: label,
                task: chosen.task,
                review: Some(review.clone()),
                error: None,
                usage: Some(usage),
                seconds: started.elapsed().as_secs_f64(),
            };
            AiOutcome {
                verdict,
                summary: review.summary,
                findings: review.findings,
                steps: vec![step],
                reason: None,
            }
        }
        Err(e) => {
            let step = Step {
                route: label,
                task: chosen.task,
                review: None,
                error: Some(e.clone()),
                usage: None,
                seconds: started.elapsed().as_secs_f64(),
            };
            incomplete(vec![step], tr!("el revisor no respondió: {e}", e = e))
        }
    }
}

/// Decide la revisión de nivel 2 o 3 más alta que respondió: una clasificación de nivel 1
/// sola nunca aprueba un cambio.
fn finish(steps: Vec<Step>, reason: Option<String>) -> AiOutcome {
    let decided = steps
        .iter()
        .rev()
        .find(|s| s.task.tier() >= 2)
        .and_then(|s| s.review.clone());
    match decided {
        Some(r) => AiOutcome {
            verdict: match r.verdict.as_str() {
                "approve" => AiVerdict::Approve,
                "block" => AiVerdict::Block,
                _ => AiVerdict::Changes,
            },
            summary: r.summary,
            findings: r.findings,
            steps,
            reason,
        },
        None => AiOutcome {
            verdict: AiVerdict::Incomplete,
            summary: String::new(),
            findings: Vec::new(),
            steps,
            reason: reason.or_else(|| {
                Some(tr!("ningún revisor de nivel 2 o 3 dio una respuesta válida").into())
            }),
        },
    }
}

/// Plantilla de `.forge/routing.toml` (formato del plan 9.4).
pub fn write_template(project: &Path) -> Result<(), String> {
    let file = project.join(".forge/routing.toml");
    if file.exists() {
        return Err(tr!("{p0} ya existe", p0 = file.display()));
    }
    std::fs::create_dir_all(project.join(".forge")).map_err(|e| e.to_string())?;
    std::fs::write(&file, TEMPLATE).map_err(|e| format!("{}: {e}", file.display()))
}

const TEMPLATE: &str = "# Router de modelos: qué modelo revisa según la tarea y el riesgo.\n\
                [routing]\n\
                profile = \"balanced\"          # economy | balanced | quality\n\
                prefer_local = true             # clasificación con ollama si está instalado\n\
                monthly_budget_usd = 20.00\n\
                allow_escalation = true\n\
                max_cost_per_call_usd = 0.50\n\n\
                # Cada tarea puede fijar proveedor (anthropic, openai, opencode, local), modelo y esfuerzo.\n\
                # [routing.tasks.classification]\n# provider = \"local\"\n# model = \"qwen2.5-coder\"\n# effort = \"low\"\n\n\
                # [routing.tasks.code_review]\n# provider = \"anthropic\"\n# model = \"haiku\"\n# effort = \"medium\"\n\n\
                # [routing.tasks.architecture]\n# provider = \"anthropic\"\n# model = \"sonnet\"\n# effort = \"high\"\n\n\
                # [routing.tasks.security]\n# provider = \"openai\"\n# model = \"gpt-5-codex\"\n# effort = \"high\"\n";

/// Ajustes → Revisión con IA: lo que se cambia sin tocar el archivo.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutingForm {
    pub profile: Profile,
    pub prefer_local: bool,
    pub monthly_budget_usd: f64,
    pub allow_escalation: bool,
    pub max_cost_per_call_usd: f64,
}

impl RoutingForm {
    pub fn load(project: &Path) -> Result<Self, String> {
        let c = RouterConfig::load(project)?;
        Ok(Self {
            profile: c.profile,
            prefer_local: c.prefer_local,
            monthly_budget_usd: c.monthly_budget_usd,
            allow_escalation: c.allow_escalation,
            max_cost_per_call_usd: c.max_cost_per_call_usd,
        })
    }

    /// Guarda en `.forge/routing.toml` (lo crea con la plantilla si no existe) cambiando
    /// solo estos valores: los comentarios y las tareas propias se conservan.
    pub fn save(&self, project: &Path) -> Result<(), String> {
        let file = project.join(".forge/routing.toml");
        let text = std::fs::read_to_string(&file).unwrap_or_else(|_| TEMPLATE.to_string());
        let mut doc: toml_edit::DocumentMut = text
            .parse()
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let profile = match self.profile {
            Profile::Economy => "economy",
            Profile::Balanced => "balanced",
            Profile::Quality => "quality",
        };
        let r = &mut doc["routing"];
        r["profile"] = toml_edit::value(profile);
        r["prefer_local"] = toml_edit::value(self.prefer_local);
        r["monthly_budget_usd"] = toml_edit::value(self.monthly_budget_usd.max(0.0));
        r["allow_escalation"] = toml_edit::value(self.allow_escalation);
        r["max_cost_per_call_usd"] = toml_edit::value(self.max_cost_per_call_usd.max(0.0));
        let out = doc.to_string();
        toml::from_str::<RoutingFile>(&out).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(project.join(".forge")).map_err(|e| e.to_string())?;
        std::fs::write(&file, out).map_err(|e| format!("{}: {e}", file.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_routing_form_keeps_comments_and_tasks() {
        let dir = std::env::temp_dir().join(format!("forge-routing-form-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut form = RoutingForm::load(&dir).unwrap();
        assert_eq!(form.profile, Profile::Balanced);
        form.profile = Profile::Economy;
        form.monthly_budget_usd = 5.0;
        form.save(&dir).unwrap();
        let path = dir.join(".forge/routing.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("profile = \"economy\"") && text.contains("# Router de modelos"));
        std::fs::write(
            &path,
            format!(
                "{text}\n[routing.tasks.code_review]\nprovider = \"anthropic\"\nmodel = \"haiku\"\n"
            ),
        )
        .unwrap();
        form.max_cost_per_call_usd = 0.2;
        form.save(&dir).unwrap();
        let c = RouterConfig::load(&dir).unwrap();
        assert_eq!(
            (c.profile, c.monthly_budget_usd, c.max_cost_per_call_usd),
            (Profile::Economy, 5.0, 0.2)
        );
        assert!(
            c.tasks.contains_key("code_review"),
            "las tareas propias siguen"
        );
        assert_eq!(RoutingForm::load(&dir).unwrap(), form);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn file(path: &str, added: usize) -> FileChange {
        FileChange {
            path: path.into(),
            added,
            deleted: 0,
            binary: false,
            removed: false,
        }
    }

    #[test]
    fn risk_signals_and_levels() {
        let low = assess(&[file("src/utils/format.ts", 20)], 0);
        assert_eq!((low.level, low.signals.len()), (RiskLevel::Low, 0));
        let payments = assess(
            &[
                file("src/payments/wompi.ts", 80),
                file("src/auth/session.ts", 10),
            ],
            0,
        );
        assert!(
            payments.level >= RiskLevel::High && payments.security,
            "{payments:?}"
        );
        assert!(payments.signals.iter().any(|s| s.contains("pagos")));
        let migration = assess(&[file("drizzle/0003_vacunas.sql", 40)], 0);
        assert_eq!(
            (migration.level, migration.security),
            (RiskLevel::Medium, false)
        );
    }

    #[test]
    fn plans_by_profile() {
        let mut config = RouterConfig::default();
        let high = Risk {
            score: 5,
            level: RiskLevel::High,
            signals: vec![],
            security: true,
        };
        let low = Risk {
            score: 0,
            level: RiskLevel::Low,
            signals: vec![],
            security: false,
        };
        assert_eq!(
            plan(&config, &low, false),
            vec![Task::Classification, Task::CodeReview]
        );
        assert_eq!(
            plan(&config, &high, false),
            vec![Task::Classification, Task::CodeReview, Task::Security]
        );
        assert_eq!(plan(&config, &high, true), vec![Task::Security]);
        config.profile = Profile::Economy;
        assert_eq!(
            plan(&config, &high, false),
            vec![Task::Classification, Task::CodeReview],
            "economy no usa nivel 3"
        );
        config.profile = Profile::Quality;
        assert_eq!(plan(&config, &low, false), vec![Task::CodeReview]);

        let r = route(&RouterConfig::default(), Task::Classification, false);
        assert_eq!(
            (r.provider.as_str(), r.model.as_str()),
            ("anthropic", "haiku")
        );
        let r = route(&RouterConfig::default(), Task::Classification, true);
        assert_eq!(r.provider, "local");
    }

    #[test]
    fn parses_outputs() {
        let claude = r#"{"type":"result","is_error":false,"result":"```json\n{\"verdict\":\"approve\",\"confidence\":0.9,\"summary\":\"ok\",\"findings\":[]}\n```","total_cost_usd":0.0151,"usage":{"input_tokens":2,"cache_creation_input_tokens":75633,"output_tokens":40}}"#;
        let (text, input, output, cost) = parse_output(OutputFormat::ClaudeJson, claude).unwrap();
        assert_eq!((input, output, cost), (75635, 40, Some(0.0151)));
        assert_eq!(parse_review(&text).unwrap().verdict, "approve");

        let codex = "{\"type\":\"thread.started\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{\\\"verdict\\\":\\\"block\\\",\\\"confidence\\\":0.8}\"}}\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1200,\"output_tokens\":90}}";
        let (text, input, output, _) = parse_output(OutputFormat::CodexJson, codex).unwrap();
        assert_eq!((input, output), (1200, 90));
        assert_eq!(parse_review(&text).unwrap().verdict, "block");

        assert!(parse_review("no sé").is_none());
        assert!(parse_review("{\"verdict\":\"quizás\"}").is_none());
        assert_eq!(
            truncate("áéíóú", 3),
            "á\n[… recortado: 8 caracteres omitidos para ahorrar contexto]"
        );
    }

    /// Repo real + proveedores falsos (sin gastar): escalado por desacuerdo y presupuesto.
    #[test]
    fn escalation_with_fake_providers() {
        let dir = std::env::temp_dir().join(format!("forge-router-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |c: &str| {
            assert!(
                Command::new("sh")
                    .args(["-c", c])
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success(),
                "{c}"
            )
        };
        sh(
            "git init -q -b main && git config user.email t@t && git config user.name t && echo a > a.txt && git add . && git commit -qm x",
        );
        let key = ["sk-ant-", "api03-abcdefghijklmnopqrstuvwxyz"].concat();
        std::fs::write(dir.join("a.txt"), format!("a\nconst k = '{key}';\n")).unwrap();
        std::fs::write(dir.join(".env"), "SECRET=1\n").unwrap();
        sh("git add -f a.txt .env");
        // El proveedor guarda el prompt para comprobar la reducción de contexto.
        let prompt_file = dir.join("prompt.txt");
        let fake = |verdict: &str, confidence: f64| ProviderDef {
            command: format!(
                "cat > '{}'; echo '{{\"verdict\":\"{verdict}\",\"confidence\":{confidence},\"summary\":\"{verdict}\",\"findings\":[]}}'",
                prompt_file.display()
            ),
            format: OutputFormat::Text,
            cost_per_call_usd: 0.10,
            local: false,
        };
        let mut config = RouterConfig::default();
        config
            .providers
            .insert("barato".into(), fake("approve", 0.9));
        config
            .providers
            .insert("medio".into(), fake("changes", 0.8));
        config
            .providers
            .insert("potente".into(), fake("block", 0.95));
        let task = |p: &str| TaskRoute {
            provider: p.into(),
            model: "m".into(),
            effort: Some("low".into()),
        };
        config.tasks.insert("classification".into(), task("barato"));
        config.tasks.insert("code_review".into(), task("medio"));
        config.tasks.insert("architecture".into(), task("potente"));
        config.tasks.insert("security".into(), task("potente"));
        config.max_cost_per_call_usd = 0.10;

        let ctx = ReviewContext {
            project: "x".into(),
            stage: Stage::Commit,
            risk: assess(&[file("a.txt", 1)], 0),
            deterministic: vec!["✓ Tests".into()],
            files: vec![file("a.txt", 1)],
            repo: dir.clone(),
            range: vec!["--cached".into()],
        };
        let rules = Rules::default();
        // Nivel 1 aprueba y nivel 2 pide cambios: desacuerdo → escala al nivel 3, que decide.
        let outcome = review(&config, &rules, &ctx, false, 0.0, false, &|_| {});
        let tiers: Vec<u8> = outcome.steps.iter().map(|s| s.task.tier()).collect();
        assert_eq!(tiers, vec![1, 2, 3], "{:?}", outcome.details());
        assert_eq!(outcome.verdict, AiVerdict::Block);
        assert!((outcome.cost() - 0.30).abs() < 1e-9);
        let sent = std::fs::read_to_string(&prompt_file).unwrap();
        assert!(
            !sent.contains(&key),
            "el secreto no debe salir hacia el modelo"
        );
        assert!(!sent.contains("SECRET=1"), ".env no debe enviarse");

        // Con el presupuesto casi agotado no se llama a proveedores de pago.
        config.monthly_budget_usd = 0.15;
        let outcome = review(&config, &rules, &ctx, false, 0.10, false, &|_| {});
        assert_eq!(outcome.verdict, AiVerdict::Incomplete);
        assert!(outcome.reason.unwrap().contains("presupuesto"));
    }
}
