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
    /// Si la revisión no se puede completar (sin proveedor, sin respuesta, sin presupuesto),
    /// queda como aviso en vez de pendiente: no bloquea.
    pub best_effort: bool,
    /// Profundidad de la revisión por lentes (plan #42): según el riesgo o fija.
    pub review_depth: crate::reviewers::DepthMode,
    /// Lentes generales que se usan (de `reviewers::LENSES`).
    pub lenses: Vec<String>,
    /// Otro modelo intenta desmentir los hallazgos que bloquearían.
    pub refuter: bool,
    /// Modelo de los dos jueces del Juicio (con Claude).
    pub judge_model: String,
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
            best_effort: false,
            review_depth: crate::reviewers::DepthMode::Risk,
            lenses: crate::reviewers::LENSES.map(String::from).to_vec(),
            refuter: true,
            judge_model: "sonnet".into(),
        }
    }
}

/// ¿Paga este revisor? Los de un proveedor local no gastan presupuesto.
pub fn reviewer_is_paid(
    config: &RouterConfig,
    reviewer: &crate::reviewers::ReviewerSpec,
    local: bool,
) -> bool {
    let chosen = reviewer_route(config, reviewer, local);
    config.provider(&chosen.provider).is_some_and(|p| !p.local)
}

/// Cuántos de `n` revisores en paralelo caben en lo que queda del mes, contando el tope de
/// cada llamada (todos parten del mismo gasto: sin esto se podía pasar en (n − 1) topes).
pub fn affordable_reviewers(config: &RouterConfig, spent: f64, n: usize) -> usize {
    let room = config.monthly_budget_usd - spent;
    if config.max_cost_per_call_usd <= 0.0 {
        return if room > 0.0 { n } else { 0 };
    }
    ((room / config.max_cost_per_call_usd).floor().max(0.0) as usize).min(n)
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
            // Lee el proyecto (solo lectura) para ver el código que el diff llama o toca, como
            // los jueces de gentle-ai; los secretos, nunca (el diff ya los oculta). Sin los MCP,
            // skills, plugins ni hooks del usuario: con ellos cada llamada cargaba ~400 k
            // tokens (0,51 $ con Haiku frente a 0,001 $), y los hooks no pintan nada aquí.
            "anthropic" | "claude" => builtin(
                "claude -p --model {model} --effort {effort} --output-format json --no-session-persistence \
                 --strict-mcp-config --disable-slash-commands --setting-sources '' \
                 --tools 'Read,Grep,Glob' --allowedTools 'Read,Grep,Glob' \
                 --disallowedTools 'Read(**/.env*)' 'Read(**/*.pem)' 'Read(**/*.key)' 'Read(~/.ssh/**)' \
                 --max-budget-usd {max_cost}",
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
    // Una ruta elegida se respeta siempre: una local sin Ollama falla (queda incompleta, o
    // como aviso con best_effort) en vez de mandar el código a la nube sin decirlo.
    if let Some(t) = config.tasks.get(task.key()) {
        let _ = local_available;
        return Route {
            task,
            provider: t.provider.clone(),
            model: t.model.clone(),
            effort: t.effort.clone().unwrap_or_else(|| "medium".into()),
        };
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
         Si puedes leer archivos del proyecto, abre los archivos que el diff llama o toca (y sus \
         tests) antes de afirmar un fallo, y no inventes lo que no hayas visto.\n\
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

/// Saca el objeto JSON de la revisión del texto (los modelos a veces lo envuelven en ```json).
pub fn parse_review(text: &str) -> Option<ModelReview> {
    // Probando desde cada `{`: puede haber alguna suelta antes (el perfil de la shell).
    let end = text.rfind('}')?;
    text.match_indices('{')
        .filter(|(start, _)| *start < end)
        .find_map(|(start, _)| serde_json::from_str::<ModelReview>(&text[start..=end]).ok())
        .filter(|r| matches!(r.verdict.as_str(), "approve" | "changes" | "block"))
}

/// Texto y consumo según el formato del proveedor.
fn parse_output(
    format: OutputFormat,
    stdout: &str,
) -> Result<(String, u64, u64, Option<f64>), String> {
    match format {
        OutputFormat::Text => Ok((stdout.to_string(), 0, 0, None)),
        OutputFormat::ClaudeJson => {
            // El JSON va en la última línea, desde su primera `{`: lo de antes puede ser lo que
            // imprime el perfil de la shell de inicio de sesión (neofetch y sus códigos de
            // escape, avisos de nvm…).
            let v: serde_json::Value = stdout
                .lines()
                .rev()
                .filter_map(|l| l.find('{').map(|i| &l[i..]))
                .find_map(|l| {
                    serde_json::from_str(l.trim())
                        .ok()
                        .filter(|v: &serde_json::Value| v.is_object())
                })
                .map_or_else(|| serde_json::from_str(stdout.trim()), Ok)
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
    let mut child = crate::shell_command(&command)
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

/// Filtro de precisión (idea de gentle-ai): solo para los revisores informativos, que si no
/// llenan la revisión de ruido. Los que bloquean (seguridad…) no se callan por dudar.
fn precision_gate() -> String {
    tr!("Reporta un hallazgo solo si es un defecto real, que afecta a quien usa el código y que puedes defender con evidencia concreta del diff. Si dudas, no lo reportes. El estilo no es un hallazgo salvo que esconda un defecto.").to_string()
}

fn specialist_role(reviewer: &crate::reviewers::ReviewerSpec) -> String {
    let role = tr!(
        "Eres el {p0}. Revisa SOLO {p1}. Ignora lo que no sea de tu área y no repitas observaciones generales.",
        p0 = reviewer.name.to_lowercase(),
        p1 = reviewer.focus
    );
    if reviewer.blocking {
        role.to_string()
    } else {
        format!("{role} {}", precision_gate())
    }
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
    let role = specialist_role(reviewer);
    let started = Instant::now();
    let label = format!("{} · {} {}", reviewer.name, chosen.provider, chosen.model);
    match call(config, &chosen, &prompt_with(&role, ctx, &diff), &ctx.repo) {
        Ok((review, usage)) => {
            let verdict = match review.verdict.as_str() {
                "approve" => AiVerdict::Approve,
                "block" => AiVerdict::Block,
                _ => AiVerdict::Changes,
            };
            let spent = month_spent + usage.cost_usd;
            let step = Step {
                route: label,
                task: chosen.task,
                review: Some(review.clone()),
                error: None,
                usage: Some(usage),
                seconds: started.elapsed().as_secs_f64(),
            };
            let mut steps = vec![step];
            let mut findings = review.findings;
            let mut verdict = verdict;
            // Solo lo que bloquearía pasa por el refutador (idea de gentle-ai): menos
            // bloqueos falsos sin gastar en lo informativo.
            // Bloquear sin hallazgos concretos no tiene nada que desmentir: sigue bloqueando.
            let had_blocking = findings.iter().any(|f| f.severity == "blocking");
            if config.refuter && reviewer.blocking && verdict == AiVerdict::Block && had_blocking {
                findings = refute(
                    config, ctx, reviewer, &diff, findings, spent, local, &mut steps,
                );
                if !findings.iter().any(|f| f.severity == "blocking") {
                    verdict = AiVerdict::Changes;
                }
            }
            AiOutcome {
                verdict,
                summary: review.summary,
                findings,
                steps,
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

/// Hallazgos graves que se revisan como mucho por revisión (cada uno es una llamada).
// ponytail: tope fijo; configurable en Ajustes si hiciera falta (plan #42, fase 3).
// El presupuesto se comprueba por revisor (los de Guard corren en paralelo con el gasto de
// partida), así que el mes puede pasarse como mucho MAX_REFUTED llamadas por revisor que
// bloquea; si molesta, reservarlas en `affordable_reviewers`.
const MAX_REFUTED: usize = 3;
/// Confianza mínima para que un desmentido cuente.
const MIN_REFUTE_CONFIDENCE: f64 = 0.7;

/// Otro modelo (la ruta potente) intenta desmentir cada hallazgo bloqueante. Desmentido:
/// queda como nota; confirmado, dudoso, sin respuesta o sin presupuesto: se mantiene.
#[allow(clippy::too_many_arguments)]
fn refute(
    config: &RouterConfig,
    ctx: &ReviewContext,
    reviewer: &crate::reviewers::ReviewerSpec,
    diff: &str,
    findings: Vec<Finding>,
    mut spent: f64,
    local: bool,
    steps: &mut Vec<Step>,
) -> Vec<Finding> {
    let task = if reviewer.id == "security" {
        Task::Security
    } else {
        Task::Architecture
    };
    let chosen = route(config, task, local);
    let paid = config.provider(&chosen.provider).is_some_and(|p| !p.local);
    let mut tried = 0;
    findings
        .into_iter()
        .map(|f| {
            if f.severity != "blocking" || tried == MAX_REFUTED {
                return f;
            }
            if paid && spent + config.max_cost_per_call_usd > config.monthly_budget_usd {
                return f;
            }
            tried += 1;
            let prompt = format!(
                "Eres un refutador. Otro revisor ({}) marcó este hallazgo como bloqueante. \
                 Intenta desmentirlo con el diff: ¿es un defecto real que se demuestra con el código mostrado?\n\
                 Hallazgo: {}:{} {}\n\n\
                 Diff:\n```diff\n{diff}\n```\n\n\
                 Responde SOLO con un objeto JSON: {{\"verdict\": \"block\" si el hallazgo es real, \
                 \"approve\" si es falso o no se sostiene, \"changes\" si el diff no basta para decidir, \
                 \"confidence\": 0.0-1.0, \"summary\": \"una frase en español\", \"findings\": []}}",
                reviewer.name, f.file, f.line, f.message
            );
            let started = Instant::now();
            let label = tr!("{p0} · refutador", p0 = chosen.label());
            match call(config, &chosen, &prompt, &ctx.repo) {
                Ok((r, usage)) => {
                    spent += usage.cost_usd;
                    // El diff puede intentar convencerlo: solo cuenta un desmentido seguro.
                    let refuted = r.verdict == "approve" && r.confidence >= MIN_REFUTE_CONFIDENCE;
                    steps.push(Step {
                        route: label,
                        task,
                        review: Some(r),
                        error: None,
                        usage: Some(usage),
                        seconds: started.elapsed().as_secs_f64(),
                    });
                    if refuted {
                        Finding {
                            severity: "info".into(),
                            message: tr!("Desmentido por el refutador: {m}", m = f.message),
                            ..f
                        }
                    } else {
                        f
                    }
                }
                Err(e) => {
                    steps.push(Step {
                        route: label,
                        task,
                        review: None,
                        error: Some(e),
                        usage: None,
                        seconds: started.elapsed().as_secs_f64(),
                    });
                    f
                }
            }
        })
        .collect()
}

/// Ruta de los jueces: la de nivel 3 con el modelo de los jueces (con Claude), salvo que el
/// usuario fijara una en routing.toml, que se respeta siempre.
fn judge_route(config: &RouterConfig, local: bool) -> Route {
    let mut chosen = route(config, Task::Architecture, local);
    if !config.tasks.contains_key(Task::Architecture.key())
        && matches!(chosen.provider.as_str(), "anthropic" | "claude")
    {
        chosen.model = config.judge_model.clone();
    }
    chosen
}

/// Juicio: dos jueces a ciegas, en paralelo.
pub fn judgment(
    config: &RouterConfig,
    rules: &Rules,
    ctx: &ReviewContext,
    month_spent: f64,
    local: bool,
    on_step: &dyn Fn(&str),
) -> AiOutcome {
    let chosen = judge_route(config, local);
    let paid = config.provider(&chosen.provider).is_some_and(|p| !p.local);
    if paid && month_spent + 2.0 * config.max_cost_per_call_usd > config.monthly_budget_usd {
        return finish(
            Vec::new(),
            Some(format!(
                "presupuesto mensual agotado (${month_spent:.2} de ${:.2}); sube monthly_budget_usd o usa un modelo local",
                config.monthly_budget_usd
            )),
        );
    }
    on_step(&tr!("2 jueces · {p0}", p0 = chosen.label()));
    let task = Task::Architecture;
    let diff = match reduced_diff(&ctx.repo, &ctx.range, rules, task.context_budget(), &[]) {
        Ok(d) => d,
        Err(e) => {
            return finish(
                Vec::new(),
                Some(tr!("no se pudo preparar el diff: {e}", e = e)),
            );
        }
    };
    // A ciegas: el mismo diff y criterio, cada uno sin ver al otro.
    let steps: Vec<Step> = std::thread::scope(|scope| {
        let handles: Vec<_> = ["A", "B"]
            .map(|judge| {
                let (chosen, diff) = (&chosen, &diff);
                scope.spawn(move || {
                    let started = Instant::now();
                    let prompt = prompt_with(&judge_role(judge), ctx, diff);
                    let result = call(config, chosen, &prompt, &ctx.repo);
                    let (review, error, usage) = match result {
                        Ok((r, u)) => (Some(r), None, Some(u)),
                        Err(e) => (None, Some(e), None),
                    };
                    Step {
                        route: tr!("{p0} · juez {judge}", p0 = chosen.label(), judge = judge),
                        task,
                        review,
                        error,
                        usage,
                        seconds: started.elapsed().as_secs_f64(),
                    }
                })
            })
            .into_iter()
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| Step {
                    route: chosen.label(),
                    task,
                    review: None,
                    error: Some(tr!("el juez falló").into()),
                    usage: None,
                    seconds: 0.0,
                })
            })
            .collect()
    });
    let (verdict, findings) = merge_judges(steps[0].review.as_ref(), steps[1].review.as_ref());
    let summary = steps
        .iter()
        .filter_map(|s| s.review.as_ref())
        .map(|r| r.summary.clone())
        .collect::<Vec<_>>()
        .join(" · ");
    let reason = match steps.iter().filter(|s| s.review.is_none()).count() {
        0 => None,
        1 => Some(tr!("un juez no respondió: no se puede aprobar con uno solo").to_string()),
        _ => Some(tr!("ningún juez dio una respuesta válida").to_string()),
    };
    AiOutcome {
        verdict,
        summary,
        findings,
        steps,
        reason,
    }
}

/// Instrucciones de cada juez: adversarial y con los casos que hay que recorrer (las que
/// hacen buena la revisión de gentle-ai: leer el proyecto y asumir que hay fallos).
fn judge_role(judge: &str) -> String {
    format!(
        "Eres el Juez {judge} de un Juicio: otro juez revisa el mismo cambio a ciegas y solo \
         cuenta lo que veáis los dos. Sé exhaustivo y adversarial: asume que el cambio tiene \
         fallos hasta demostrar lo contrario. Recorre, uno por uno, los casos que apliquen:\n\
         - Corrección: ¿hace lo que pretende? Caminos felices y no felices.\n\
         - Interacción: doble clic o envío doble, clic mientras carga, no hacer nada, cancelar a \
           medias, volver atrás, recargar.\n\
         - Estados: vacío, cargando, error, sin permisos, sin conexión, datos a medias.\n\
         - Datos: nulos o ausentes, vacíos, muy largos, duplicados, formato inválido, límites \
           (0, 1, máximo), zonas horarias y monedas.\n\
         - Errores: excepciones sin capturar, errores tragados, mensajes al usuario, reintentos.\n\
         - Concurrencia: dos peticiones a la vez, condiciones de carrera, orden de respuestas.\n\
         - Seguridad: autorización en el servidor, inyección, datos sensibles, secretos.\n\
         - Rendimiento: consultas N+1, bucles costosos, renders o peticiones de más.\n\
         - Tests: ¿cubren estos casos? Falta de tests para lo nuevo.\n\
         - Convenciones del proyecto.\n\
         Cada hallazgo con archivo y línea, y la evidencia concreta (qué pasa y cuándo). \
         \"blocking\" para lo que rompe algo a quien lo usa; \"warning\" para lo que debería \
         arreglarse; \"info\" para el resto."
    )
}

/// Junta los dos jueces del Juicio (como Judgment Day de gentle-ai): un hallazgo que los dos
/// ven (mismo archivo, líneas a 5 o menos) se confirma y bloquea si es grave; lo que ve uno
/// solo es sospecha: avisa, nunca bloquea. Con un juez caído no se aprueba.
pub fn merge_judges(a: Option<&ModelReview>, b: Option<&ModelReview>) -> (AiVerdict, Vec<Finding>) {
    let (a, b) = match (a, b) {
        (None, None) => return (AiVerdict::Incomplete, Vec::new()),
        (Some(x), None) | (None, Some(x)) => (x, None),
        (Some(a), Some(b)) => (a, Some(b)),
    };
    // Sin texto no hay hallazgo (un juez de verdad devolvió alguno vacío).
    let said = |v: &[Finding]| -> Vec<Finding> {
        v.iter()
            .filter(|f| !f.message.trim().is_empty())
            .cloned()
            .collect()
    };
    let mine = said(&a.findings);
    let others = b.map_or_else(Vec::new, |b| said(&b.findings));
    // Solo con ubicación: dos hallazgos sin archivo («faltan tests», «sin autorización») no
    // son el mismo.
    let near = |x: &Finding, y: &Finding| {
        !x.file.is_empty() && x.file == y.file && x.line.abs_diff(y.line) <= 5
    };
    let rank = |s: &str| match s {
        "blocking" => 2,
        "warning" => 1,
        _ => 0,
    };
    let (mut confirmed, mut suspects) = (Vec::new(), Vec::new());
    let mut matched = vec![false; others.len()];
    for x in &mine {
        // Uno a uno: cada hallazgo del otro juez confirma uno solo.
        match (0..others.len()).find(|&i| !matched[i] && near(x, &others[i])) {
            Some(i) => {
                matched[i] = true;
                let y = &others[i];
                let mut f = x.clone();
                if rank(&y.severity) > rank(&x.severity) {
                    f.severity = y.severity.clone();
                }
                f.message = tr!(
                    "{p0} (2 de 2 jueces; el otro: {p1})",
                    p0 = x.message,
                    p1 = y.message
                );
                confirmed.push(f);
            }
            None => suspects.push(x.clone()),
        }
    }
    suspects.extend(
        others
            .iter()
            .zip(&matched)
            .filter(|(_, m)| !**m)
            .map(|(y, _)| y.clone()),
    );
    for f in &mut suspects {
        if rank(&f.severity) == 2 {
            f.severity = "warning".into();
        }
        f.message = tr!("Sospecha (1 de 2 jueces): {p0}", p0 = f.message);
    }
    let verdict = if confirmed.iter().any(|f| rank(&f.severity) == 2) {
        AiVerdict::Block
    } else if b.is_none()
        // Un juez que no aprueba, aunque no dé hallazgos, no deja aprobar.
        || a.verdict != "approve"
        || b.is_some_and(|b| b.verdict != "approve")
        || confirmed
            .iter()
            .chain(&suspects)
            .any(|f| rank(&f.severity) >= 1)
    {
        AiVerdict::Changes
    } else {
        AiVerdict::Approve
    };
    confirmed.extend(suspects);
    (verdict, confirmed)
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
                max_cost_per_call_usd = 0.50\n\
                best_effort = false             # true: sin proveedor o sin respuesta, avisa y no bloquea\n\
                review_depth = \"risk\"           # risk | light | full | judgment (2 jueces): cuántas lentes según el riesgo\n\
                judge_model = \"sonnet\"         # modelo de los jueces del Juicio (con Claude)\n\
                lenses = [\"reliability\", \"readability\", \"resilience\"]\n\
                refuter = true                  # otro modelo intenta desmentir lo que bloquearía\n\n\
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
    /// Sin proveedor o sin respuesta, la revisión avisa y no bloquea.
    pub best_effort: bool,
    /// Revisión de código, arquitectura y seguridad con un modelo local (Ollama): el código no
    /// sale del ordenador.
    pub local_reviews: bool,
    pub local_model: String,
    pub review_depth: crate::reviewers::DepthMode,
    pub lenses: Vec<String>,
    pub refuter: bool,
    /// Modelo de los jueces del Juicio.
    pub judge_model: String,
}

/// Las tareas de nivel 2–3 que «Revisar en local» lleva a Ollama.
const LOCAL_TASKS: [&str; 3] = ["code_review", "architecture", "security"];
/// Modelo local propuesto si no hay ninguno elegido.
const LOCAL_MODEL: &str = "qwen2.5-coder";

impl RoutingForm {
    pub fn load(project: &Path) -> Result<Self, String> {
        let c = RouterConfig::load(project)?;
        Ok(Self {
            profile: c.profile,
            prefer_local: c.prefer_local,
            monthly_budget_usd: c.monthly_budget_usd,
            allow_escalation: c.allow_escalation,
            max_cost_per_call_usd: c.max_cost_per_call_usd,
            best_effort: c.best_effort,
            // En local si las tres tareas van a Ollama (con el mismo modelo).
            local_reviews: LOCAL_TASKS
                .iter()
                .all(|t| c.tasks.get(*t).is_some_and(|r| r.provider == "local")),
            local_model: c
                .tasks
                .get(LOCAL_TASKS[0])
                .filter(|r| r.provider == "local" && !r.model.is_empty())
                .map_or_else(|| LOCAL_MODEL.into(), |r| r.model.clone()),
            review_depth: c.review_depth,
            lenses: c.lenses,
            refuter: c.refuter,
            judge_model: c.judge_model.clone(),
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
        r["best_effort"] = toml_edit::value(self.best_effort);
        r["review_depth"] = toml_edit::value(match self.review_depth {
            crate::reviewers::DepthMode::Risk => "risk",
            crate::reviewers::DepthMode::Light => "light",
            crate::reviewers::DepthMode::Full => "full",
            crate::reviewers::DepthMode::Judgment => "judgment",
        });
        r["judge_model"] = toml_edit::value(match self.judge_model.trim() {
            "" => "sonnet",
            m => m,
        });
        r["lenses"] = toml_edit::value(self.lenses.iter().collect::<toml_edit::Array>());
        r["refuter"] = toml_edit::value(self.refuter);
        let model = match self.local_model.trim() {
            "" => LOCAL_MODEL,
            m => m,
        };
        if r.get("tasks").is_none_or(|t| !t.is_table_like()) {
            r["tasks"] = toml_edit::table();
        }
        for task in LOCAL_TASKS {
            let field = |k: &str| {
                r["tasks"]
                    .get(task)
                    .and_then(|t| t.get(k))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            };
            let local = field("provider").as_deref() == Some("local");
            if self.local_reviews && !local {
                // Solo las que aún no van en local: lo afinado a mano en una local se queda.
                let mut t = toml_edit::Table::new();
                t["provider"] = toml_edit::value("local");
                t["model"] = toml_edit::value(model);
                r["tasks"][task] = toml_edit::Item::Table(t);
            } else if !self.local_reviews && local && field("model").as_deref() == Some(model) {
                // Solo se quita lo que puso esta opción (su modelo); lo propio se queda.
                if let Some(tasks) = r["tasks"].as_table_like_mut() {
                    tasks.remove(task);
                }
            }
        }
        let out = doc.to_string();
        toml::from_str::<RoutingFile>(&out).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(project.join(".forge")).map_err(|e| e.to_string())?;
        std::fs::write(&file, out).map_err(|e| format!("{}: {e}", file.display()))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parallel_reviewers_fit_the_remaining_budget() {
        let c = RouterConfig {
            monthly_budget_usd: 1.0,
            max_cost_per_call_usd: 0.3,
            ..Default::default()
        };
        assert_eq!(affordable_reviewers(&c, 0.2, 5), 2, "0,80 / 0,30");
        assert_eq!(affordable_reviewers(&c, 0.0, 2), 2);
        assert_eq!(affordable_reviewers(&c, 1.5, 3), 0);
        let file: RoutingFile = toml::from_str("[routing]\nbest_effort = true").unwrap();
        assert!(file.routing.best_effort);
        // Incompleta: con best_effort avisa y deja pasar; sin él, queda pendiente.
        use crate::guard::{Level, incomplete_level};
        assert_eq!(incomplete_level(true, true), Level::Warn);
        assert_eq!(incomplete_level(false, true), Level::Pending);
        assert_eq!(incomplete_level(false, false), Level::Info, "informativo");
    }

    use super::*;

    #[test]
    fn a_local_route_never_falls_back_to_the_cloud() {
        // «Revisar en local (privado)» sin Ollama: falla (o avisa), pero el código no sube.
        let mut config = RouterConfig::default();
        config.tasks.insert(
            "security".into(),
            TaskRoute {
                provider: "local".into(),
                model: "qwen2.5-coder".into(),
                effort: None,
            },
        );
        assert_eq!(route(&config, Task::Security, false).provider, "local");
    }

    #[test]
    fn the_form_can_send_reviews_to_a_local_model() {
        // Niveles 2–3 en local: el código no sale del ordenador.
        let dir = std::env::temp_dir().join(format!("forge-routing-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut form = RoutingForm::load(&dir).unwrap();
        assert!(!form.local_reviews && !form.best_effort);
        form.local_reviews = true;
        form.local_model = "qwen2.5-coder:14b".into();
        form.best_effort = true;
        form.save(&dir).unwrap();
        let config = RouterConfig::load(&dir).unwrap();
        assert!(config.best_effort);
        for task in ["code_review", "architecture", "security"] {
            let route = &config.tasks[task];
            assert_eq!(route.provider, "local", "{task}");
            assert_eq!(route.model, "qwen2.5-coder:14b", "{task}");
        }
        let again = RoutingForm::load(&dir).unwrap();
        assert!(again.local_reviews && again.best_effort);
        assert_eq!(again.local_model, "qwen2.5-coder:14b");
        // Lo que el usuario afinó a mano en una tarea local no se pisa al guardar.
        let path = dir.join(".forge/routing.toml");
        let mut doc: toml_edit::DocumentMut =
            std::fs::read_to_string(&path).unwrap().parse().unwrap();
        doc["routing"]["tasks"]["security"]["model"] = toml_edit::value("llama-sec");
        doc["routing"]["tasks"]["security"]["effort"] = toml_edit::value("high");
        std::fs::write(&path, doc.to_string()).unwrap();
        let mut form = RoutingForm::load(&dir).unwrap();
        form.monthly_budget_usd = 7.0;
        form.save(&dir).unwrap();
        let security = RouterConfig::load(&dir).unwrap().tasks["security"].clone();
        assert_eq!(
            (security.model.as_str(), security.effort.as_deref()),
            ("llama-sec", Some("high"))
        );
        // Quitarlo deja de serie solo lo que puso la opción; la ruta propia se queda.
        let mut off = RoutingForm::load(&dir).unwrap();
        off.local_reviews = false;
        off.save(&dir).unwrap();
        let tasks = RouterConfig::load(&dir).unwrap().tasks;
        assert!(!tasks.contains_key("code_review"));
        assert_eq!(tasks["security"].model, "llama-sec");
    }

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
        // Las opciones de la revisión por lentes (plan #42).
        assert_eq!(
            (form.review_depth, form.refuter, form.lenses.len()),
            (crate::reviewers::DepthMode::Risk, true, 3),
            "de serie: según el riesgo, con refutador y las tres lentes"
        );
        form.review_depth = crate::reviewers::DepthMode::Full;
        form.refuter = false;
        form.lenses = vec!["reliability".into()];
        form.save(&dir).unwrap();
        let c = RouterConfig::load(&dir).unwrap();
        assert_eq!(
            (c.review_depth, c.refuter, c.lenses.clone()),
            (
                crate::reviewers::DepthMode::Full,
                false,
                vec!["reliability".to_string()]
            )
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

    /// Lee todo stdin en `$p` (PowerShell, el shell de `shell_command` en Windows), en UTF-8.
    const READ_PROMPT_PS: &str =
        "$p = [IO.StreamReader]::new([Console]::OpenStandardInput()).ReadToEnd()";

    /// Proveedor falso (sin gastar) en el shell de `shell_command`: lee el prompt de stdin, lo
    /// guarda en `save` si se pide y responde `reply` (sin comillas simples).
    fn fake_command(reply: &str, save: Option<&Path>) -> String {
        if cfg!(windows) {
            let save = save
                .map(|p| format!("; Set-Content -LiteralPath '{}' -Value $p", p.display()))
                .unwrap_or_default();
            format!("{READ_PROMPT_PS}{save}; echo '{reply}'")
        } else {
            let save = save.map_or("/dev/null".into(), |p| format!("'{}'", p.display()));
            format!("cat > {save}; echo '{reply}'")
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
    fn claude_reviewers_read_the_project_but_never_secrets() {
        let claude = RouterConfig::default()
            .provider("anthropic")
            .unwrap()
            .command;
        // Leen el código alrededor del diff (como los jueces de gentle-ai), sin escribir
        // ni ejecutar nada.
        assert!(claude.contains("--tools 'Read,Grep,Glob'"), "{claude}");
        assert!(
            claude.contains("--allowedTools 'Read,Grep,Glob'"),
            "{claude}"
        );
        // Sin los MCP, skills, plugins ni hooks del usuario (cientos de miles de tokens).
        for lean in [
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--setting-sources ''",
        ] {
            assert!(claude.contains(lean), "falta {lean}: {claude}");
        }
        // Los secretos no salen hacia el modelo, igual que en el diff.
        for secret in [
            "Read(**/.env*)",
            "Read(**/*.pem)",
            "Read(**/*.key)",
            "Read(~/.ssh/**)",
        ] {
            assert!(claude.contains(secret), "falta {secret}: {claude}");
        }
        // El prompt les dice que miren antes de opinar.
        let ctx = ReviewContext {
            project: "p".into(),
            stage: Stage::Commit,
            risk: assess(&[], 0),
            deterministic: vec![],
            files: vec![],
            repo: ".".into(),
            range: vec![],
        };
        assert!(prompt_with("Revisa.", &ctx, "").contains("abre los archivos"));
    }

    #[test]
    fn judgment_runs_two_blind_judges_in_parallel() {
        let dir = std::env::temp_dir().join(format!("forge-judgment-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ok = Command::new("sh")
            .args(["-c", "git init -q -b main && git config user.email t@t && git config user.name t && echo a > a.txt && git add . && git commit -qm x && printf 'a\\nb\\nc\\n' > a.txt && git add a.txt"])
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(ok.success());
        // Cada juez responde según su letra; los dos ven el mismo fallo en a.txt.
        let json = |line: u32| {
            format!(
                "{{\"verdict\":\"block\",\"confidence\":0.9,\"summary\":\"x\",\"findings\":[{{\"severity\":\"blocking\",\"file\":\"a.txt\",\"line\":{line},\"message\":\"doble clic\"}}]}}"
            )
        };
        let mut config = RouterConfig::default();
        config.providers.insert(
            "juez".into(),
            ProviderDef {
                command: if cfg!(windows) {
                    format!(
                        "{READ_PROMPT_PS}; if ($p.Contains('Juez A')) {{ echo '{}' }} elseif ($p.Contains('Juez B')) {{ echo '{}' }}",
                        json(2),
                        json(3)
                    )
                } else {
                    format!(
                        "p=$(cat); case \"$p\" in *'Juez A'*) echo '{}';; *'Juez B'*) echo '{}';; esac",
                        json(2),
                        json(3)
                    )
                },
                format: OutputFormat::Text,
                cost_per_call_usd: 0.10,
                local: false,
            },
        );
        config.tasks.insert(
            "architecture".into(),
            TaskRoute {
                provider: "juez".into(),
                model: "m".into(),
                effort: None,
            },
        );
        let ctx = ReviewContext {
            project: "x".into(),
            stage: Stage::Commit,
            risk: assess(&[], 0),
            deterministic: vec![],
            files: vec![],
            repo: dir.clone(),
            range: vec!["--cached".into()],
        };
        let outcome = judgment(&config, &Rules::default(), &ctx, 0.0, false, &|_| {});
        assert_eq!(outcome.steps.len(), 2, "{:?}", outcome.details());
        assert_eq!(outcome.verdict, AiVerdict::Block, "{:?}", outcome.details());
        assert_eq!(outcome.findings.len(), 1);
        assert!((outcome.cost() - 0.20).abs() < 1e-9);
        // Sin presupuesto para los dos jueces: incompleto, sin llamar a nadie.
        config.monthly_budget_usd = 0.5;
        let poor = judgment(&config, &Rules::default(), &ctx, 0.2, false, &|_| {});
        assert_eq!(poor.verdict, AiVerdict::Incomplete);
        assert!(poor.steps.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_judges_confirm_what_both_see_and_suspect_the_rest() {
        let f = |severity: &str, file: &str, line: u64, message: &str| Finding {
            severity: severity.into(),
            file: file.into(),
            line,
            message: message.into(),
        };
        let judge = |verdict: &str, findings| ModelReview {
            verdict: verdict.into(),
            confidence: 0.8,
            risk: None,
            summary: String::new(),
            findings,
        };
        // Los dos ven el doble clic (líneas cercanas): confirmado, bloquea. Solo uno ve lo
        // del estado vacío: sospecha, que avisa pero no bloquea.
        let a = judge(
            "block",
            vec![
                f("blocking", "Button.tsx", 12, "doble clic envía dos veces"),
                f("blocking", "List.tsx", 40, "sin estado vacío"),
            ],
        );
        let b = judge(
            "block",
            vec![f(
                "blocking",
                "Button.tsx",
                14,
                "no se desactiva mientras carga",
            )],
        );
        let (verdict, findings) = merge_judges(Some(&a), Some(&b));
        assert_eq!(verdict, AiVerdict::Block);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert_eq!(findings[0].severity, "blocking");
        assert!(findings[0].message.contains("doble clic"));
        assert!(
            findings[0].message.contains("2 de 2"),
            "{}",
            findings[0].message
        );
        assert_eq!(findings[1].severity, "warning");
        assert!(
            findings[1].message.contains("1 de 2"),
            "{}",
            findings[1].message
        );

        // Solo sospechas: pide cambios, no bloquea.
        let (verdict, _) = merge_judges(Some(&a), Some(&judge("approve", vec![])));
        assert_eq!(verdict, AiVerdict::Changes);
        // Los dos aprueban sin hallazgos: aprobado.
        let clean = judge("approve", vec![]);
        assert_eq!(
            merge_judges(Some(&clean), Some(&clean)).0,
            AiVerdict::Approve
        );
        // Un juez falló: lo del otro es sospecha y no aprueba solo.
        assert_eq!(merge_judges(Some(&clean), None).0, AiVerdict::Changes);
        assert_eq!(merge_judges(None, None).0, AiVerdict::Incomplete);

        // Sin archivo no se emparejan: «faltan tests» y «sin autorización» son dos sospechas.
        let a = judge("block", vec![f("blocking", "", 0, "faltan tests")]);
        let b = judge("block", vec![f("blocking", "", 0, "sin autorización")]);
        let (verdict, findings) = merge_judges(Some(&a), Some(&b));
        assert_eq!(verdict, AiVerdict::Changes, "{findings:?}");
        assert!(findings.iter().all(|f| f.message.contains("1 de 2")));
        // Un hallazgo sin texto (visto con Sonnet de verdad) no se muestra.
        let blank = judge("approve", vec![f("warning", "x.md", 22, "  ")]);
        assert_eq!(
            merge_judges(Some(&blank), Some(&clean)),
            (AiVerdict::Approve, vec![])
        );
        // Un juez que no aprueba, aunque no dé hallazgos, no deja aprobar.
        let (verdict, _) = merge_judges(Some(&judge("block", vec![])), Some(&clean));
        assert_eq!(verdict, AiVerdict::Changes);
        // Emparejamiento uno a uno: dos hallazgos de A cerca de uno de B → uno confirmado y
        // una sospecha (B vio un solo problema).
        let a = judge(
            "block",
            vec![
                f("blocking", "x.ts", 10, "uno"),
                f("blocking", "x.ts", 12, "dos"),
            ],
        );
        let b = judge("block", vec![f("blocking", "x.ts", 11, "uno")]);
        let (_, findings) = merge_judges(Some(&a), Some(&b));
        let confirmed = findings
            .iter()
            .filter(|f| f.message.contains("2 de 2"))
            .count();
        assert_eq!((confirmed, findings.len()), (1, 2), "{findings:?}");
    }

    #[test]
    fn judges_use_the_judge_model_unless_the_user_pinned_one() {
        let mut config = RouterConfig::default();
        assert_eq!(judge_route(&config, false).model, "sonnet");
        config.judge_model = "opus".into();
        assert_eq!(judge_route(&config, false).model, "opus");
        // Una ruta elegida en routing.toml se respeta siempre.
        config.tasks.insert(
            "architecture".into(),
            TaskRoute {
                provider: "anthropic".into(),
                model: "claude-opus-4-9".into(),
                effort: None,
            },
        );
        assert_eq!(judge_route(&config, false).model, "claude-opus-4-9");
    }

    #[test]
    fn parses_outputs() {
        let claude = r#"{"type":"result","is_error":false,"result":"```json\n{\"verdict\":\"approve\",\"confidence\":0.9,\"summary\":\"ok\",\"findings\":[]}\n```","total_cost_usd":0.0151,"usage":{"input_tokens":2,"cache_creation_input_tokens":75633,"output_tokens":40}}"#;
        let (text, input, output, cost) = parse_output(OutputFormat::ClaudeJson, claude).unwrap();
        assert_eq!((input, output, cost), (75635, 40, Some(0.0151)));
        assert_eq!(parse_review(&text).unwrap().verdict, "approve");
        // Un perfil de la shell que imprime algo antes (neofetch, avisos de nvm, `{` sueltas):
        // el JSON de claude es la última línea.
        // neofetch deja sus códigos de escape finales en la misma línea que el JSON.
        let noisy = format!("\x1b[32m'c.\n OS: macOS {{x}}\n\n\x1b[?25h\x1b[?7h{claude}\n");
        let (text, ..) = parse_output(OutputFormat::ClaudeJson, &noisy).unwrap();
        assert_eq!(parse_review(&text).unwrap().verdict, "approve");

        let codex = "{\"type\":\"thread.started\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{\\\"verdict\\\":\\\"block\\\",\\\"confidence\\\":0.8}\"}}\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1200,\"output_tokens\":90}}";
        let (text, input, output, _) = parse_output(OutputFormat::CodexJson, codex).unwrap();
        assert_eq!((input, output), (1200, 90));
        assert_eq!(parse_review(&text).unwrap().verdict, "block");

        // Una `{` suelta antes del JSON (el perfil de la shell, texto del modelo).
        let text = "OS: {x}\n{\"verdict\":\"changes\",\"confidence\":0.7}";
        assert_eq!(parse_review(text).unwrap().verdict, "changes");
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
            command: fake_command(
                &format!(
                    "{{\"verdict\":\"{verdict}\",\"confidence\":{confidence},\"summary\":\"{verdict}\",\"findings\":[]}}"
                ),
                Some(&prompt_file),
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
        assert!(
            !sent.contains(&precision_gate()),
            "la revisión general (que puede bloquear) no se calla por dudar"
        );
        let specs = crate::reviewers::builtins();
        let role = |id: &str| specialist_role(specs.iter().find(|r| r.id == id).unwrap());
        assert!(role("readability").contains(&precision_gate()));
        assert!(
            !role("security").contains(&precision_gate()),
            "bloquea: mejor de más"
        );

        // Con el presupuesto casi agotado no se llama a proveedores de pago.
        config.monthly_budget_usd = 0.15;
        let outcome = review(&config, &rules, &ctx, false, 0.10, false, &|_| {});
        assert_eq!(outcome.verdict, AiVerdict::Incomplete);
        assert!(outcome.reason.unwrap().contains("presupuesto"));

        // Refutador: un revisor que bloquea ve su hallazgo grave revisado por otro modelo.
        config.monthly_budget_usd = 100.0;
        config.providers.insert(
            "revisor".into(),
            ProviderDef {
                command: fake_command("{\"verdict\":\"block\",\"confidence\":0.9,\"summary\":\"clave expuesta\",\"findings\":[{\"severity\":\"blocking\",\"file\":\"a.txt\",\"line\":2,\"message\":\"clave en el código\"},{\"severity\":\"warning\",\"file\":\"a.txt\",\"line\":1,\"message\":\"nombre poco claro\"}]}", None),
                format: OutputFormat::Text,
                cost_per_call_usd: 0.10,
                local: false,
            },
        );
        let mut security = specs.iter().find(|r| r.id == "security").unwrap().clone();
        security.provider = Some("revisor".into());
        let activation = crate::reviewers::Activation {
            reviewer: security,
            files: vec!["a.txt".into()],
        };
        // El refutador (la ruta de seguridad, «potente») lo confirma: sigue bloqueando.
        let outcome = specialist(&config, &rules, &ctx, &activation, 0.0, false);
        assert_eq!(outcome.verdict, AiVerdict::Block, "{:?}", outcome.details());
        assert_eq!(
            outcome.steps.len(),
            2,
            "revisión + una refutación (solo lo grave)"
        );
        // Lo desmiente: deja de bloquear, y se ve que fue desmentido.
        config
            .providers
            .insert("potente".into(), fake("approve", 0.9));
        let outcome = specialist(&config, &rules, &ctx, &activation, 0.0, false);
        assert_eq!(
            outcome.verdict,
            AiVerdict::Changes,
            "{:?}",
            outcome.details()
        );
        assert!(
            outcome
                .details()
                .iter()
                .any(|d| d.contains("Desmentido") && d.contains("clave en el código")),
            "{:?}",
            outcome.details()
        );
        assert!(
            outcome.findings.iter().all(|f| f.severity != "blocking"),
            "{:?}",
            outcome.findings
        );
        // Un refutador que no responde bien: el hallazgo se mantiene.
        config.providers.insert(
            "potente".into(),
            ProviderDef {
                command: fake_command("basura", None),
                format: OutputFormat::Text,
                cost_per_call_usd: 0.10,
                local: false,
            },
        );
        let outcome = specialist(&config, &rules, &ctx, &activation, 0.0, false);
        assert_eq!(outcome.verdict, AiVerdict::Block, "{:?}", outcome.details());
        // Con el refutador apagado en Ajustes no se gasta en refutar.
        config.refuter = false;
        let outcome = specialist(&config, &rules, &ctx, &activation, 0.0, false);
        assert_eq!(
            (outcome.verdict, outcome.steps.len()),
            (AiVerdict::Block, 1)
        );
        config.refuter = true;
        // Bloquea sin hallazgos concretos: no hay nada que desmentir, sigue bloqueando.
        config
            .providers
            .insert("potente".into(), fake("approve", 0.9));
        config
            .providers
            .insert("revisor".into(), fake("block", 0.9));
        let outcome = specialist(&config, &rules, &ctx, &activation, 0.0, false);
        assert_eq!(outcome.verdict, AiVerdict::Block, "{:?}", outcome.details());
    }
}
