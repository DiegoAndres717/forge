// Project Guard: reglas deterministas antes de commit, push y pull request.
// Independiente de la UI: la app y (en la Fase 6) los hooks de Git usan el mismo motor.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::candidate::{self, Candidate, Snapshot};
use crate::evidence::{Evidence, Report, ReportCheck, ReportItem};
use crate::reviewers::Activation;
use crate::router::{self, AiOutcome, AiVerdict, ReviewContext, RouterConfig, Usage};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------- reglas

#[derive(Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Rules {
    pub quality: Quality,
    pub commit: StageRules,
    pub push: StageRules,
    pub pull_request: StageRules,
    pub checks: Vec<Check>,
}

#[derive(Deserialize, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Quality {
    pub max_changed_lines: usize,
    pub warning_changed_lines: usize,
    pub max_files_changed: usize,
    pub block_secrets: bool,
    pub block_env_files: bool,
    /// Archivos que nunca deben entrar al repositorio.
    pub forbidden_files: Vec<String>,
    pub lines: LineRules,
}

impl Default for Quality {
    fn default() -> Self {
        Self {
            max_changed_lines: 4000,
            warning_changed_lines: 2000,
            max_files_changed: 50,
            block_secrets: true,
            block_env_files: true,
            forbidden_files: [
                "*.pem",
                "*.key",
                "*.p12",
                "*.pfx",
                "id_rsa",
                "id_ed25519",
                "credentials.json",
                "secrets/**",
            ]
            .map(String::from)
            .to_vec(),
            lines: LineRules::default(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct LineRules {
    /// Archivos que no cuentan para el límite de líneas (lockfiles, generados…).
    pub exclude: Vec<String>,
}

impl Default for LineRules {
    fn default() -> Self {
        Self {
            exclude: [
                "package-lock.json",
                "pnpm-lock.yaml",
                "yarn.lock",
                "bun.lockb",
                "bun.lock",
                "Cargo.lock",
                "*.min.js",
                "*.generated.*",
                "coverage/**",
                "dist/**",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct StageRules {
    pub enabled: bool,
    pub require_lint: bool,
    pub require_typecheck: bool,
    pub require_tests: bool,
    pub require_build: bool,
    pub require_ai_review: bool,
    pub require_security_review: bool,
    /// Rama base para pull requests (por defecto la rama principal del remoto).
    pub base: Option<String>,
}

impl Default for StageRules {
    fn default() -> Self {
        Self {
            enabled: true,
            require_lint: false,
            require_typecheck: false,
            require_tests: false,
            require_build: false,
            require_ai_review: false,
            require_security_review: false,
            base: None,
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub id: String,
    pub name: Option<String>,
    pub command: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub severity: Severity,
    /// Etapas extra en las que corre, además de las que lo exigen con `require_*`.
    #[serde(default)]
    pub stages: Vec<Stage>,
}

fn default_timeout() -> u64 {
    300
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Blocking,
    Warning,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Commit,
    Push,
    PullRequest,
}

impl Stage {
    pub const ALL: [Stage; 3] = [Stage::Commit, Stage::Push, Stage::PullRequest];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Commit => "Commit",
            Stage::Push => "Push",
            Stage::PullRequest => "Pull request",
        }
    }
}

impl Rules {
    /// Lee `.forge/rules.toml`. `Ok(None)` si no existe.
    pub fn load(project: &Path) -> Result<Option<Self>, String> {
        let path = project.join(".forge/rules.toml");
        let text = match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
            Ok(text) => text,
        };
        let rules: Rules = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut seen = std::collections::HashSet::new();
        if let Some(c) = rules.checks.iter().find(|c| !seen.insert(&c.id)) {
            return Err(format!(
                "{}: el check \"{}\" está repetido",
                path.display(),
                c.id
            ));
        }
        Ok(Some(rules))
    }

    pub fn stage(&self, stage: Stage) -> &StageRules {
        match stage {
            Stage::Commit => &self.commit,
            Stage::Push => &self.push,
            Stage::PullRequest => &self.pull_request,
        }
    }

    /// Checks de la etapa y los `require_*` que no tienen `[[checks]]` definido.
    pub fn checks_for(&self, stage: Stage) -> (Vec<&Check>, Vec<String>) {
        let s = self.stage(stage);
        let required = [
            (s.require_lint, "lint"),
            (s.require_typecheck, "typecheck"),
            (s.require_tests, "tests"),
            (s.require_build, "build"),
        ];
        let mut checks = Vec::new();
        let mut missing = Vec::new();
        for (_, id) in required.into_iter().filter(|(on, _)| *on) {
            match self.checks.iter().find(|c| c.id == id) {
                Some(c) => checks.push(c),
                None => missing.push(id.to_string()),
            }
        }
        for c in &self.checks {
            if c.stages.contains(&stage) && !checks.iter().any(|x| x.id == c.id) {
                checks.push(c);
            }
        }
        (checks, missing)
    }
}

/// Crea `.forge/rules.toml` con checks detectados (scripts de npm o proyecto Rust).
pub fn write_template(project: &Path) -> Result<(), String> {
    let file = project.join(".forge/rules.toml");
    if file.exists() {
        return Err(format!("{} ya existe", file.display()));
    }
    std::fs::create_dir_all(project.join(".forge")).map_err(|e| e.to_string())?;
    std::fs::write(&file, template(&detect_checks(project)))
        .map_err(|e| format!("{}: {e}", file.display()))
}

fn detect_checks(project: &Path) -> Vec<(&'static str, &'static str, String)> {
    if project.join("Cargo.toml").exists() {
        return vec![
            (
                "lint",
                "Clippy",
                "cargo clippy --all-targets -- -D warnings".into(),
            ),
            (
                "typecheck",
                "cargo check",
                "cargo check --all-targets".into(),
            ),
            ("tests", "Tests", "cargo test".into()),
            ("build", "Build", "cargo build --release".into()),
        ];
    }
    let map = [
        ("lint", "Lint"),
        ("typecheck", "TypeScript"),
        ("test", "Tests"),
        ("build", "Build"),
    ];
    let commands = crate::project::detect_commands(project);
    map.iter()
        .filter_map(|(script, name)| {
            let c = commands.iter().find(|c| c.name == *script)?;
            let id = if *script == "test" { "tests" } else { *script };
            Some((id, *name, c.command.clone()))
        })
        .collect()
}

fn template(checks: &[(&str, &str, String)]) -> String {
    let has = |id: &str| checks.iter().any(|(i, _, _)| *i == id);
    let mut out = String::from(
        "# Project Guard: controles antes de commit, push y pull request.\n\n\
         [quality]\nmax_changed_lines = 4000\nwarning_changed_lines = 2000\nmax_files_changed = 50\n\
         block_secrets = true\nblock_env_files = true\n\n\
         [quality.lines]\n# Archivos que no cuentan para el límite de líneas.\n\
         exclude = [\"package-lock.json\", \"pnpm-lock.yaml\", \"yarn.lock\", \"Cargo.lock\", \"*.generated.ts\", \"drizzle/meta/**\", \"coverage/**\", \"dist/**\"]\n\n",
    );
    let stage = |name: &str, build: bool| {
        format!(
            "[{name}]\nenabled = true\nrequire_lint = {}\nrequire_typecheck = {}\nrequire_tests = {}\nrequire_build = {}\n\n",
            has("lint"),
            has("typecheck"),
            has("tests"),
            build && has("build"),
        )
    };
    out += &stage("commit", false);
    out += &stage("push", true);
    out += &stage("pull_request", true);
    for (id, name, command) in checks {
        out += &format!(
            "[[checks]]\nid = {id:?}\nname = {name:?}\ncommand = {command:?}\ntimeout_seconds = 300\nseverity = \"blocking\"\n\n"
        );
    }
    if checks.is_empty() {
        out += "# [[checks]]\n# id = \"tests\"\n# name = \"Tests\"\n# command = \"npm test\"\n# timeout_seconds = 300\n# severity = \"blocking\"   # o \"warning\"\n";
    }
    out
}

// ---------------------------------------------------------------- patrones de rutas

/// Glob estilo gitignore: sin `/` compara el nombre del archivo en cualquier carpeta;
/// `*` y `?` dentro de un segmento, `**` cualquier número de carpetas, `dir/` = `dir/**`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches("./");
    let pattern = match pattern.strip_suffix('/') {
        Some(dir) => format!("{dir}/**"),
        None => pattern.to_string(),
    };
    if !pattern.contains('/') {
        return segment_match(
            pattern.as_bytes(),
            path.rsplit('/').next().unwrap_or(path).as_bytes(),
        );
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    segments_match(&p, &s)
}

fn segments_match(p: &[&str], s: &[&str]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(&"**") => (0..=s.len()).any(|i| segments_match(&p[1..], &s[i..])),
        Some(seg) => {
            !s.is_empty()
                && segment_match(seg.as_bytes(), s[0].as_bytes())
                && segments_match(&p[1..], &s[1..])
        }
    }
}

fn segment_match(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            segment_match(&p[1..], s) || (!s.is_empty() && segment_match(p, &s[1..]))
        }
        (Some(b'?'), Some(_)) => segment_match(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => segment_match(&p[1..], &s[1..]),
        _ => false,
    }
}

// ---------------------------------------------------------------- diff

#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
    /// El archivo ya no existe en el árbol (se está borrando).
    pub removed: bool,
}

#[derive(Debug, Default)]
pub struct Diff {
    pub files: Vec<FileChange>,
    /// Qué se evaluó, para mostrarlo ("cambios preparados", "commits sin subir vs origin/main"...).
    pub description: String,
    /// Comando para ver este mismo diff en un terminal.
    pub command: String,
    /// Líneas añadidas (ruta, número de línea, texto) para buscar secretos.
    pub added_lines: Vec<(String, usize, String)>,
    /// En la etapa commit: no había nada preparado y se evaluó todo el árbol de trabajo.
    pub unstaged: bool,
    /// Identidad del contenido evaluado (sha256 del patch). Cambia si cambia cualquier línea.
    pub candidate: String,
    /// Argumentos de `git diff` que producen este diff (para pedir el mismo a un modelo).
    pub range: Vec<String>,
    pub branch: String,
}

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("no se pudo ejecutar git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub fn repo_root(path: &Path) -> Result<PathBuf, String> {
    git(path, &["rev-parse", "--show-toplevel"])
        .map(|s| PathBuf::from(s.trim()))
        .map_err(|_| "no es un repositorio Git".to_string())
}

fn has_head(repo: &Path) -> bool {
    git(repo, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok()
}

/// Rama principal del remoto (origin/HEAD), o main/master locales.
fn default_branch(repo: &Path) -> Result<String, String> {
    if let Ok(r) = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        return Ok(r.trim().to_string());
    }
    for candidate in ["origin/main", "origin/master", "main", "master"] {
        if git(repo, &["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return Ok(candidate.to_string());
        }
    }
    Err("no se encontró la rama base (configura `base` en [pull_request])".into())
}

/// `push_range`: rango exacto que se va a subir (lo da el hook pre-push).
pub fn collect_diff(
    repo: &Path,
    stage: Stage,
    rules: &Rules,
    push_range: Option<&str>,
) -> Result<Diff, String> {
    let mut diff = Diff::default();
    let range: Vec<String> = match stage {
        Stage::Commit => {
            let staged = git(
                repo,
                &["diff", "--cached", "--numstat", "-z", "--no-renames"],
            )?;
            if !staged.is_empty() {
                diff.description = "cambios preparados (git add)".into();
                diff.command = "git diff --cached".into();
                vec!["--cached".into()]
            } else {
                diff.unstaged = true;
                diff.description = "nada preparado: se evalúan todos los cambios sin commit".into();
                diff.command = "git status --short && git diff HEAD".into();
                let base = if has_head(repo) { "HEAD" } else { EMPTY_TREE };
                vec![base.into()]
            }
        }
        Stage::Push if push_range.is_some() => {
            let range = push_range.unwrap_or_default();
            diff.description = format!("commits a subir ({range})");
            diff.command = format!("git log --oneline {range} && git diff {range}");
            vec![range.to_string()]
        }
        Stage::Push | Stage::PullRequest => {
            if !has_head(repo) {
                return Err("el repositorio no tiene commits todavía".into());
            }
            let base = match (stage, &rules.pull_request.base) {
                (Stage::PullRequest, Some(base)) => base.clone(),
                (Stage::Push, _) => match git(
                    repo,
                    &[
                        "rev-parse",
                        "--abbrev-ref",
                        "--symbolic-full-name",
                        "@{upstream}",
                    ],
                ) {
                    Ok(upstream) => upstream.trim().to_string(),
                    Err(_) => default_branch(repo)?,
                },
                _ => default_branch(repo)?,
            };
            diff.description = match stage {
                Stage::Push => format!("commits sin subir (vs {base})"),
                _ => format!("cambios de la rama vs {base}"),
            };
            diff.command = format!("git diff {base}...HEAD");
            vec![format!("{base}...HEAD")]
        }
    };
    diff.range = range.clone();
    let range: Vec<&str> = range.iter().map(String::as_str).collect();

    let numstat = git(
        repo,
        &[&["diff", "--numstat", "-z", "--no-renames"], &range[..]].concat(),
    )?;
    diff.files = parse_numstat(&numstat);
    let patch = git(
        repo,
        &[
            &["diff", "-U0", "--no-color", "--no-renames", "--no-ext-diff"],
            &range[..],
        ]
        .concat(),
    )?;
    diff.added_lines = parse_added_lines(&patch);
    let mut hasher = Sha256::new();
    hasher.update(format!("{stage:?}\0").as_bytes());
    hasher.update(patch.as_bytes());

    // Archivos nuevos sin seguimiento (solo al evaluar el árbol de trabajo).
    if diff.unstaged {
        let untracked = git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for path in untracked.split('\0').filter(|p| !p.is_empty()) {
            let (lines, binary) = read_text(&repo.join(path));
            hasher.update(format!("\0nuevo:{path}\0{}", lines.join("\n")).as_bytes());
            diff.files.push(FileChange {
                path: path.into(),
                added: lines.len(),
                deleted: 0,
                binary,
                removed: false,
            });
            diff.added_lines.extend(
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, l)| (path.to_string(), i + 1, l)),
            );
        }
    }
    for f in &mut diff.files {
        f.removed = !repo.join(&f.path).exists();
    }
    diff.candidate = format!("sha256:{}", hex(&hasher.finalize()));
    diff.branch = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|b| b.trim().to_string())
        .unwrap_or_default();
    Ok(diff)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Líneas de un archivo de texto (vacío si es binario o mayor de 1 MB).
fn read_text(path: &Path) -> (Vec<String>, bool) {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() <= 1 << 20 && !bytes.contains(&0) => (
            String::from_utf8_lossy(&bytes)
                .lines()
                .map(String::from)
                .collect(),
            false,
        ),
        Ok(_) => (Vec::new(), true),
        Err(_) => (Vec::new(), false),
    }
}

/// `git diff --numstat -z`: "añadidas\tborradas\truta\0" ("-" en binarios).
fn parse_numstat(text: &str) -> Vec<FileChange> {
    text.split('\0')
        .filter_map(|record| {
            let mut parts = record.splitn(3, '\t');
            let (added, deleted, path) = (parts.next()?, parts.next()?, parts.next()?);
            let binary = added == "-";
            Some(FileChange {
                path: path.trim_start_matches('\n').to_string(),
                added: added.parse().unwrap_or(0),
                deleted: deleted.parse().unwrap_or(0),
                binary,
                removed: false,
            })
        })
        .collect()
}

/// Líneas añadidas de un patch unificado con su ruta y número de línea nuevo.
fn parse_added_lines(patch: &str) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let mut file: Option<String> = None;
    let mut line_no = 0;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            file = path.strip_prefix("b/").map(String::from);
        } else if let Some(hunk) = line.strip_prefix("@@ ") {
            // @@ -a,b +c,d @@
            line_no = hunk
                .split_whitespace()
                .find_map(|p| p.strip_prefix('+'))
                .and_then(|p| p.split(',').next()?.parse().ok())
                .unwrap_or(0);
        } else if let (Some(text), Some(f)) = (line.strip_prefix('+'), &file) {
            out.push((f.clone(), line_no, text.to_string()));
            line_no += 1;
        } else if line.starts_with(' ') {
            line_no += 1;
        }
    }
    out
}

// ---------------------------------------------------------------- secretos

static SECRET_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("clave privada", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
        ("clave de AWS", r"\b(AKIA|ASIA)[0-9A-Z]{16}\b"),
        ("token de GitHub", r"\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}\b|\bgithub_pat_[A-Za-z0-9_]{60,}"),
        ("clave de Anthropic", r"\bsk-ant-[A-Za-z0-9_-]{20,}"),
        ("clave de OpenAI", r"\bsk-(proj-)?[A-Za-z0-9_-]{32,}"),
        ("clave de Stripe", r"\b(sk|rk)_live_[0-9A-Za-z]{20,}"),
        ("token de Slack", r"\bxox[abposr]-[0-9A-Za-z-]{10,}"),
        ("clave de Google", r"\bAIza[0-9A-Za-z_-]{35}"),
        (
            "contraseña o clave en el código",
            r#"(?i)\b(password|passwd|secret|api_?key|access_?token|auth_?token|client_?secret)\b["']?\s*[:=]\s*["'][^"'\s]{8,}["']"#,
        ),
    ]
    .into_iter()
    .map(|(name, re)| (name, Regex::new(re).expect("patrón de secreto válido")))
    .collect()
});

#[derive(Debug, Clone, PartialEq)]
pub struct SecretFinding {
    pub path: String,
    pub line: usize,
    pub kind: &'static str,
    /// Fragmento enmascarado: nunca se muestra el secreto completo.
    pub preview: String,
}

/// Marcador para líneas con datos de prueba que parecen secretos (como `gitleaks:allow`).
pub const ALLOW_SECRET: &str = "forge:allow-secret";

/// Sustituye los secretos detectables por un marcador (antes de enviar texto a un modelo).
pub fn mask_secrets(text: &str) -> String {
    let mut out = text.to_string();
    for (_, re) in SECRET_PATTERNS.iter() {
        out = re.replace_all(&out, "[SECRETO OCULTO]").into_owned();
    }
    out
}

pub fn scan_secrets(lines: &[(String, usize, String)]) -> Vec<SecretFinding> {
    lines
        .iter()
        .filter(|(_, _, text)| !text.contains(ALLOW_SECRET))
        .filter_map(|(path, line, text)| {
            SECRET_PATTERNS.iter().find_map(|(kind, re)| {
                let m = re.find(text)?;
                let preview = format!("{}••••", m.as_str().chars().take(6).collect::<String>());
                Some(SecretFinding {
                    path: path.clone(),
                    line: *line,
                    kind,
                    preview,
                })
            })
        })
        .collect()
}

// ---------------------------------------------------------------- evaluación

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Level {
    Pass,
    Warn,
    Block,
    Pending,
    Info,
    /// Fallaba, pero hay una excepción registrada con motivo.
    Excepted,
}

#[derive(Clone, Debug)]
pub struct Item {
    /// Regla estable para excepciones: `lines`, `files`, `forbidden`, `secrets`, `missing:<id>`…
    pub id: String,
    pub label: String,
    pub level: Level,
    pub details: Vec<String>,
}

fn item(
    id: impl Into<String>,
    level: Level,
    label: impl Into<String>,
    details: Vec<String>,
) -> Item {
    Item {
        id: id.into(),
        label: label.into(),
        level,
        details,
    }
}

// ---------------------------------------------------------------- excepciones

/// Alcance de una excepción: "este commit", "este pull request" (la rama) o "esta sesión".
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    Commit,
    PullRequest,
    Session,
}

/// Una sesión dura 8 horas.
pub const SESSION_SECS: i64 = 8 * 3600;

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Commit, Scope::PullRequest, Scope::Session];

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Commit => "commit",
            Scope::PullRequest => "pull-request",
            Scope::Session => "session",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Scope::ALL.into_iter().find(|x| x.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Commit => "solo este commit",
            Scope::PullRequest => "esta rama (pull request)",
            Scope::Session => "esta sesión (8 h)",
        }
    }
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Commit => "commit",
            Stage::Push => "push",
            Stage::PullRequest => "pr",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "commit" => Some(Stage::Commit),
            "push" => Some(Stage::Push),
            "pr" | "pull-request" => Some(Stage::PullRequest),
            _ => None,
        }
    }
}

/// Regla omitida con motivo. Nunca se borra: revocar la desactiva y queda en el historial.
#[derive(Clone, Debug, PartialEq)]
pub struct Exception {
    pub id: i64,
    pub rule: String,
    pub reason: String,
    pub scope: Scope,
    pub stage: Stage,
    /// Candidato (sha256 del diff) al que se ató la excepción.
    pub candidate: String,
    pub branch: String,
    pub user: String,
    pub created_at: i64,
    pub revoked: bool,
}

impl Exception {
    pub fn applies(
        &self,
        rule: &str,
        stage: Stage,
        candidate: &str,
        branch: &str,
        now: i64,
    ) -> bool {
        !self.revoked
            && self.rule == rule
            && match self.scope {
                Scope::Commit => self.stage == stage && self.candidate == candidate,
                Scope::PullRequest => !self.branch.is_empty() && self.branch == branch,
                Scope::Session => now - self.created_at < SESSION_SECS,
            }
    }

    pub fn summary(&self) -> String {
        format!(
            "omitida: «{}» — {}, {}",
            self.reason,
            self.user,
            crate::store::ago_precise(self.created_at)
        )
    }
}

/// Usuario local para registrar excepciones (git config user.name, si no $USER).
pub fn local_user(repo: &Path) -> String {
    git(repo, &["config", "user.name"])
        .map(|u| u.trim().to_string())
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "desconocido".into())
}

/// Marca como omitidos los controles fallidos que tienen excepción válida.
fn apply_exceptions(items: &mut [Item], exceptions: &[Exception]) {
    for item in items {
        if !matches!(item.level, Level::Block | Level::Warn | Level::Pending) {
            continue;
        }
        if let Some(e) = exceptions.iter().find(|e| e.rule == item.id) {
            item.level = Level::Excepted;
            item.details.insert(0, e.summary());
        }
    }
}

/// "1284" → "1.284"
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

fn env_allowed(path: &str) -> bool {
    [".example", ".sample", ".template", ".dist"]
        .iter()
        .any(|s| path.ends_with(s))
}

/// Archivos que cuentan para tamaño y riesgo (sin binarios ni excluidos como lockfiles).
pub fn counted_files(rules: &Rules, files: &[FileChange]) -> Vec<FileChange> {
    files
        .iter()
        .filter(|f| {
            !f.binary
                && !rules
                    .quality
                    .lines
                    .exclude
                    .iter()
                    .any(|p| glob_match(p, &f.path))
        })
        .cloned()
        .collect()
}

/// Controles deterministas sobre el diff (sin ejecutar nada).
pub fn evaluate(rules: &Rules, stage: Stage, diff: &Diff) -> Vec<Item> {
    let q = &rules.quality;
    let mut items = vec![item(
        "summary",
        Level::Info,
        format!("{} archivos analizados", diff.files.len()),
        vec![diff.description.clone()],
    )];

    // Tamaño, sin contar archivos excluidos ni binarios.
    let (excluded, counted): (Vec<&FileChange>, Vec<&FileChange>) = diff
        .files
        .iter()
        .partition(|f| f.binary || q.lines.exclude.iter().any(|p| glob_match(p, &f.path)));
    let lines: usize = counted.iter().map(|f| f.added + f.deleted).sum();
    let mut details = Vec::new();
    if !excluded.is_empty() {
        let names: Vec<&str> = excluded.iter().take(4).map(|f| f.path.as_str()).collect();
        let more = if excluded.len() > 4 {
            format!(" y {} más", excluded.len() - 4)
        } else {
            String::new()
        };
        details.push(format!(
            "sin contar {} archivos excluidos: {}{more}",
            excluded.len(),
            names.join(", ")
        ));
    }
    let mut biggest = counted.clone();
    biggest.sort_by_key(|f| std::cmp::Reverse(f.added + f.deleted));
    details.extend(
        biggest
            .iter()
            .take(3)
            .map(|f| format!("{}: +{} −{}", f.path, f.added, f.deleted)),
    );
    let level = if lines > q.max_changed_lines {
        Level::Block
    } else if lines > q.warning_changed_lines {
        Level::Warn
    } else {
        Level::Pass
    };
    items.push(item(
        "lines",
        level,
        format!(
            "{} líneas modificadas (límite {})",
            thousands(lines),
            thousands(q.max_changed_lines)
        ),
        details,
    ));

    let files = counted.len();
    let level = if files > q.max_files_changed {
        Level::Block
    } else {
        Level::Pass
    };
    items.push(item(
        "files",
        level,
        format!(
            "{files} archivos cambiados (límite {})",
            q.max_files_changed
        ),
        vec![],
    ));

    // Archivos prohibidos (borrarlos sí está permitido).
    let forbidden: Vec<String> = diff
        .files
        .iter()
        .filter(|f| !f.removed)
        .filter(|f| {
            let env = q.block_env_files
                && (glob_match(".env", &f.path) || glob_match(".env.*", &f.path))
                && !env_allowed(&f.path);
            env || q.forbidden_files.iter().any(|p| glob_match(p, &f.path))
        })
        .map(|f| f.path.clone())
        .collect();
    items.push(match forbidden.is_empty() {
        true => item("forbidden", Level::Pass, "Archivos permitidos", vec![]),
        false => item(
            "forbidden",
            Level::Block,
            format!("{} archivos prohibidos", forbidden.len()),
            forbidden,
        ),
    });

    if q.block_secrets {
        let found = scan_secrets(&diff.added_lines);
        items.push(match found.is_empty() {
            true => item("secrets", Level::Pass, "No se detectaron secretos", vec![]),
            false => item(
                "secrets",
                Level::Block,
                format!("{} posibles secretos", found.len()),
                found
                    .iter()
                    .map(|s| format!("{}:{} — {} ({})", s.path, s.line, s.kind, s.preview))
                    .chain([format!(
                        "Si es un dato de prueba, añade `{ALLOW_SECRET}` en esa línea."
                    )])
                    .collect(),
            ),
        });
    }

    let (_, missing) = rules.checks_for(stage);
    for id in missing {
        let flag = if id == "tests" {
            "tests".to_string()
        } else {
            id.clone()
        };
        items.push(item(
            format!("missing:{id}"),
            Level::Block,
            format!("require_{flag} activo pero no hay [[checks]] con id = \"{id}\""),
            vec![".forge/rules.toml".into()],
        ));
    }
    let s = rules.stage(stage);
    // Se ejecutan al final, solo si pasan los controles deterministas (router de modelos).
    let waiting = vec!["se ejecuta cuando pasen los controles deterministas".to_string()];
    if s.require_ai_review {
        items.push(item(
            "ai-review",
            Level::Pending,
            "Revisión de IA",
            waiting.clone(),
        ));
    }
    if s.require_security_review {
        items.push(item(
            "security-review",
            Level::Pending,
            "Revisión de seguridad",
            waiting,
        ));
    }
    items
}

// ---------------------------------------------------------------- ejecución

#[derive(Clone, Debug, PartialEq)]
pub enum CheckState {
    Waiting,
    Running,
    Passed,
    Failed(i32),
    TimedOut,
    Cancelled,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct CheckRun {
    pub check: Check,
    pub state: CheckState,
    /// Últimas líneas de la salida (sin colores).
    pub output: String,
    pub started: Option<Instant>,
    pub duration: Option<Duration>,
    /// No se ejecutó: había evidencia de este mismo candidato (marca de tiempo).
    pub reused: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Verdict {
    Ready,
    Warnings,
    Blocked,
    Running,
    Pending,
    NothingToCheck,
}

#[derive(Debug)]
pub struct GuardRun {
    pub stage: Stage,
    pub started_at: SystemTime,
    pub items: Vec<Item>,
    pub checks: Vec<CheckRun>,
    pub diff_command: String,
    pub unstaged: bool,
    pub analyzing: bool,
    pub error: Option<String>,
    pub candidate: String,
    pub branch: String,
    /// Excepciones válidas para este candidato.
    pub exceptions: Vec<Exception>,
    /// Candidato congelado (árbol exacto evaluado).
    pub frozen: Option<Candidate>,
    pub finished_at: Option<SystemTime>,
    pub project_name: String,
    /// Consumo de modelos de esta ejecución (la app/CLI lo guarda).
    pub usage: Vec<Usage>,
    /// Evidencias que no son checks (aprobaciones de IA del candidato).
    pub extra_evidence: Vec<Evidence>,
}

fn unix(t: SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl GuardRun {
    fn check_status(c: &CheckRun) -> String {
        match &c.state {
            CheckState::Waiting => "en espera".into(),
            CheckState::Running => "ejecutándose".into(),
            CheckState::Passed if c.reused.is_some() => "pasó (evidencia reutilizada)".into(),
            CheckState::Passed => "pasó".into(),
            CheckState::Failed(code) => format!("código {code}"),
            CheckState::TimedOut => format!("tiempo agotado ({} s)", c.check.timeout_seconds),
            CheckState::Cancelled => "cancelado".into(),
            CheckState::Error(e) => e.clone(),
        }
    }

    /// Reporte serializable (historial y exportación).
    pub fn report(&self) -> Report {
        Report {
            project: self.project_name.clone(),
            stage: self.stage,
            candidate: self.frozen.clone(),
            branch: self.branch.clone(),
            started_at: unix(self.started_at),
            finished_at: self
                .finished_at
                .map_or_else(|| unix(SystemTime::now()), unix),
            verdict: self.verdict(),
            progress: self.progress(),
            items: self
                .items
                .iter()
                .map(|i| ReportItem {
                    id: i.id.clone(),
                    label: i.label.clone(),
                    level: i.level,
                    details: i.details.clone(),
                })
                .collect(),
            checks: self
                .checks
                .iter()
                .map(|c| ReportCheck {
                    id: c.check.id.clone(),
                    name: c.check.name.clone().unwrap_or_else(|| c.check.id.clone()),
                    command: c.check.command.clone(),
                    level: self.check_level(c).unwrap_or(Level::Pending),
                    status: Self::check_status(c),
                    duration_ms: c.duration.map_or(0, |d| d.as_millis() as u64),
                    output: c.output.clone(),
                    reused_at: c.reused,
                })
                .collect(),
            error: self.error.clone(),
        }
    }

    /// Checks ejecutados ahora que pasaron: evidencia nueva para guardar.
    pub fn new_evidence(&self) -> Vec<Evidence> {
        let Some(frozen) = &self.frozen else {
            return Vec::new();
        };
        let now = unix(SystemTime::now());
        let extra = self.extra_evidence.iter().cloned();
        self.checks
            .iter()
            .filter(|c| c.state == CheckState::Passed && c.reused.is_none())
            .map(|c| Evidence {
                check_id: c.check.id.clone(),
                command: c.check.command.clone(),
                tree: frozen.tree.clone(),
                duration_ms: c.duration.map_or(0, |d| d.as_millis() as u64),
                output: c.output.clone(),
                created_at: now,
            })
            .chain(extra)
            .collect()
    }

    pub fn check_rule(c: &CheckRun) -> String {
        format!("check:{}", c.check.id)
    }

    pub fn exception_for(&self, rule: &str) -> Option<&Exception> {
        self.exceptions.iter().find(|e| e.rule == rule)
    }

    pub fn check_level(&self, c: &CheckRun) -> Option<Level> {
        let bad = match c.check.severity {
            Severity::Blocking => Level::Block,
            Severity::Warning => Level::Warn,
        };
        let bad = if self.exception_for(&Self::check_rule(c)).is_some() {
            Level::Excepted
        } else {
            bad
        };
        match &c.state {
            CheckState::Waiting | CheckState::Running => None,
            CheckState::Passed => Some(Level::Pass),
            CheckState::Cancelled => Some(Level::Pending),
            _ => Some(bad),
        }
    }

    /// Reglas que bloquean ahora mismo (para ofrecer excepciones).
    pub fn blocking_rules(&self) -> Vec<String> {
        let items = self
            .items
            .iter()
            .filter(|i| i.level == Level::Block)
            .map(|i| i.id.clone());
        let checks = self
            .checks
            .iter()
            .filter(|c| self.check_level(c) == Some(Level::Block))
            .map(Self::check_rule);
        items.chain(checks).collect()
    }

    pub fn verdict(&self) -> Verdict {
        if self.error.is_some() {
            return Verdict::Blocked;
        }
        if self.analyzing {
            return Verdict::Running;
        }
        let levels: Vec<Option<Level>> = self
            .items
            .iter()
            .filter(|i| i.level != Level::Info)
            .map(|i| Some(i.level))
            .chain(self.checks.iter().map(|c| self.check_level(c)))
            .collect();
        if self
            .items
            .first()
            .is_some_and(|i| i.label.starts_with("0 archivos"))
            && self.checks.is_empty()
        {
            return Verdict::NothingToCheck;
        }
        if levels.contains(&Some(Level::Block)) {
            Verdict::Blocked
        } else if levels.contains(&None) {
            Verdict::Running
        } else if levels.contains(&Some(Level::Pending)) {
            Verdict::Pending
        } else if levels.contains(&Some(Level::Warn)) {
            Verdict::Warnings
        } else {
            Verdict::Ready
        }
    }

    /// Sin análisis ni checks en curso.
    pub fn finished(&self) -> bool {
        self.finished_at.is_some()
            && !self.analyzing
            && !self
                .checks
                .iter()
                .any(|c| matches!(c.state, CheckState::Waiting | CheckState::Running))
    }

    /// (completados, total) de los controles con resultado.
    pub fn progress(&self) -> (usize, usize) {
        let items: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.level != Level::Info)
            .collect();
        let done_items = items.iter().filter(|i| i.level != Level::Pending).count();
        let done_checks = self
            .checks
            .iter()
            .filter(|c| self.check_level(c).is_some_and(|l| l != Level::Pending))
            .count();
        (done_items + done_checks, items.len() + self.checks.len())
    }
}

/// Ejecución en curso. Al soltarla se cancela (se matan los checks).
pub struct Handle {
    pub state: Arc<Mutex<GuardRun>>,
    cancel: Arc<AtomicBool>,
}

impl Handle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn snapshot<R>(&self, f: impl FnOnce(&GuardRun) -> R) -> R {
        f(&self.state.lock().unwrap())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Analiza el diff y ejecuta en paralelo los checks de la etapa. `notify` se llama
/// en cada avance (la app repinta; la CLI podrá imprimir).
pub struct RunOptions {
    pub env: std::collections::HashMap<String, String>,
    /// Excepciones del proyecto; se filtran por candidato, rama y etapa.
    pub exceptions: Vec<Exception>,
    pub push_range: Option<String>,
    /// Evidencias previas del proyecto (se usan solo las del mismo árbol y comando).
    pub evidence: Vec<Evidence>,
    /// Nombre del proyecto (para el id del candidato y el reporte).
    pub name: String,
    /// Router de modelos para las revisiones con IA (None: quedan pendientes).
    pub routing: Option<RouterConfig>,
    /// Gasto del mes en proveedores de pago (para respetar el presupuesto).
    pub month_spent: f64,
    /// Revisores especializados del proyecto.
    pub reviewers: Vec<crate::reviewers::ReviewerSpec>,
}

pub fn run(
    project: &Path,
    workdir: &Path,
    rules: Rules,
    stage: Stage,
    options: RunOptions,
    notify: impl Fn() + Send + Sync + 'static,
) -> Handle {
    let state = Arc::new(Mutex::new(GuardRun {
        stage,
        started_at: SystemTime::now(),
        items: Vec::new(),
        checks: Vec::new(),
        diff_command: String::new(),
        unstaged: false,
        analyzing: true,
        error: None,
        candidate: String::new(),
        branch: String::new(),
        exceptions: Vec::new(),
        frozen: None,
        finished_at: None,
        project_name: options.name.clone(),
        usage: Vec::new(),
        extra_evidence: Vec::new(),
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let (s, c, project, workdir) = (
        state.clone(),
        cancel.clone(),
        project.to_path_buf(),
        workdir.to_path_buf(),
    );
    let notify = Arc::new(notify);
    std::thread::spawn(move || {
        let range = options.push_range.as_deref();
        let fail = |e: String| {
            let mut st = s.lock().unwrap();
            st.error = Some(e);
            st.analyzing = false;
            st.finished_at = Some(SystemTime::now());
            drop(st);
            notify();
        };
        // Diff y candidato congelado: el árbol exacto que se va a commitear/subir.
        let analysis = repo_root(&project).and_then(|repo| {
            let diff = collect_diff(&repo, stage, &rules, range)?;
            let frozen = candidate::freeze(
                &repo,
                &options.name,
                stage,
                diff.unstaged,
                range,
                &diff.candidate,
            )?;
            Ok((repo, diff, frozen))
        });
        let (repo, diff, frozen) = match analysis {
            Ok(a) => a,
            Err(e) => return fail(e),
        };
        let now = crate::store::now();
        let exceptions: Vec<Exception> = options
            .exceptions
            .into_iter()
            .filter(|e| e.applies(&e.rule, stage, &diff.candidate, &diff.branch, now))
            .collect();
        let mut items = evaluate(&rules, stage, &diff);
        // Revisores especializados: solo los del área que toca el diff (y solo si la etapa usa IA).
        let mut activations = Vec::new();
        if rules.stage(stage).require_ai_review && !options.reviewers.is_empty() {
            let files = counted_files(&rules, &diff.files);
            let (active, skipped) =
                crate::reviewers::activate(&options.reviewers, &files, &router::assess(&files, 0));
            for a in &active {
                let details = vec![format!("archivos de su área: {}", a.files.join(", "))];
                items.push(item(
                    format!("reviewer:{}", a.reviewer.id),
                    Level::Pending,
                    a.reviewer.name.clone(),
                    details,
                ));
            }
            if !skipped.is_empty() {
                let label = format!(
                    "{} revisores no activados (el diff no toca su área)",
                    skipped.len()
                );
                items.push(item(
                    "reviewers-skipped",
                    Level::Info,
                    label,
                    skipped
                        .iter()
                        .map(|(n, why)| format!("{n}: {why}"))
                        .collect(),
                ));
            }
            activations = active;
        }
        apply_exceptions(&mut items, &exceptions);
        let (checks, _) = rules.checks_for(stage);
        let checks: Vec<Check> = checks.into_iter().cloned().collect();
        let reuse = |check: &Check| {
            options
                .evidence
                .iter()
                .filter(|e| {
                    e.tree == frozen.tree && e.check_id == check.id && e.command == check.command
                })
                .max_by_key(|e| e.created_at)
                .cloned()
        };
        {
            let mut st = s.lock().unwrap();
            st.items = items;
            st.diff_command = diff.command.clone();
            st.unstaged = diff.unstaged;
            st.candidate = diff.candidate.clone();
            st.branch = diff.branch.clone();
            st.exceptions = exceptions;
            st.frozen = Some(frozen.clone());
            st.analyzing = false;
            st.checks = checks
                .iter()
                .map(|check| match reuse(check) {
                    Some(e) => CheckRun {
                        check: check.clone(),
                        state: CheckState::Passed,
                        output: e.output,
                        started: None,
                        duration: Some(Duration::from_millis(e.duration_ms)),
                        reused: Some(e.created_at),
                    },
                    None => CheckRun {
                        check: check.clone(),
                        state: CheckState::Waiting,
                        output: String::new(),
                        started: None,
                        duration: None,
                        reused: None,
                    },
                })
                .collect();
        }
        notify();

        // Solo los checks sin evidencia válida se ejecutan.
        let pending: Vec<(usize, Check)> = checks
            .into_iter()
            .enumerate()
            .filter(|(_, c)| reuse(c).is_none())
            .collect();
        // Si el árbol de trabajo difiere del candidato, los checks corren en una copia aislada.
        let snapshot = match (frozen.isolated && !pending.is_empty())
            .then(|| Snapshot::create(&repo, &frozen.tree, &frozen.head))
        {
            Some(Err(e)) => return fail(format!("no se pudo crear la copia del candidato: {e}")),
            Some(Ok(snap)) => Some(snap),
            None => None,
        };
        let workdir = match &snapshot {
            Some(snap) => {
                let real = workdir.canonicalize().unwrap_or(workdir.clone());
                snap.path
                    .join(real.strip_prefix(&repo).unwrap_or(Path::new("")))
            }
            None => workdir.clone(),
        };

        let threads: Vec<_> = pending
            .into_iter()
            .map(|(i, check)| {
                let (s, c, workdir, notify, env) = (
                    s.clone(),
                    c.clone(),
                    workdir.clone(),
                    notify.clone(),
                    options.env.clone(),
                );
                std::thread::spawn(move || {
                    let started = Instant::now();
                    {
                        let mut st = s.lock().unwrap();
                        st.checks[i].state = CheckState::Running;
                        st.checks[i].started = Some(started);
                    }
                    notify();
                    let (result, output) = run_check(&check, &workdir, &env, &c);
                    let mut st = s.lock().unwrap();
                    st.checks[i].state = result;
                    st.checks[i].output = output;
                    st.checks[i].duration = Some(started.elapsed());
                    drop(st);
                    notify();
                })
            })
            .collect();
        for t in threads {
            let _ = t.join();
        }
        drop(snapshot); // elimina la copia aislada

        if let Some(config) = &options.routing {
            let n = notify.clone();
            let ai = AiRun {
                config,
                rules: &rules,
                stage,
                repo: &repo,
                diff: &diff,
                frozen: &frozen,
                notify: &*n,
                activations: &activations,
            };
            ai.run(&s, &options.evidence, options.month_spent, &options.name);
        }
        s.lock().unwrap().finished_at = Some(SystemTime::now());
        notify();
    });
    Handle { state, cancel }
}

/// Revisiones con IA de una ejecución del Guard.
struct AiRun<'a> {
    config: &'a RouterConfig,
    rules: &'a Rules,
    stage: Stage,
    repo: &'a Path,
    diff: &'a Diff,
    frozen: &'a Candidate,
    notify: &'a dyn Fn(),
    activations: &'a [Activation],
}

impl AiRun<'_> {
    fn run(
        &self,
        s: &Arc<Mutex<GuardRun>>,
        evidence: &[Evidence],
        month_spent: f64,
        project: &str,
    ) {
        let wanted: Vec<(usize, String)> = {
            let st = s.lock().unwrap();
            st.items
                .iter()
                .enumerate()
                .filter(|(_, i)| {
                    i.id == "ai-review"
                        || i.id == "security-review"
                        || i.id.starts_with("reviewer:")
                })
                .map(|(n, i)| (n, i.id.clone()))
                .collect()
        };
        if wanted.is_empty() {
            return;
        }
        // La IA nunca sustituye a lo determinista: si algo bloquea, no se gasta en revisar.
        let (blocked, deterministic) = {
            let st = s.lock().unwrap();
            let blocked = st.items.iter().any(|i| i.level == Level::Block)
                || st
                    .checks
                    .iter()
                    .any(|c| st.check_level(c) == Some(Level::Block));
            let passed = st
                .items
                .iter()
                .filter(|i| matches!(i.level, Level::Pass | Level::Excepted))
                .map(|i| format!("✓ {}", i.label))
                .chain(
                    st.checks
                        .iter()
                        .filter(|c| c.state == CheckState::Passed)
                        .map(|c| format!("✓ {}", c.check.id)),
                )
                .collect::<Vec<_>>();
            (blocked, passed)
        };
        if blocked {
            let mut st = s.lock().unwrap();
            for (n, _) in &wanted {
                st.items[*n].details =
                    vec!["en espera: primero hay que resolver los bloqueos deterministas".into()];
            }
            return;
        }
        let files = counted_files(self.rules, &self.diff.files);
        let risk = router::assess(&files, 0);
        let ctx = ReviewContext {
            project: project.to_string(),
            stage: self.stage,
            risk: risk.clone(),
            deterministic,
            files,
            repo: self.repo.to_path_buf(),
            range: self.diff.range.clone(),
        };
        let local = router::local_available();
        let mut spent = month_spent;
        for (n, rule) in wanted
            .iter()
            .filter(|(_, r)| !r.starts_with("reviewer:"))
            .cloned()
        {
            let security = rule == "security-review";
            let title = if security {
                "Revisión de seguridad"
            } else {
                "Revisión de IA"
            };
            // Firma de la ruta: si cambia el perfil o los modelos, la aprobación previa no vale.
            let signature = format!(
                "{:?}|{}",
                self.config.profile,
                router::plan(self.config, &risk, security)
                    .iter()
                    .map(|t| router::route(self.config, *t, local).label())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if let Some(e) = evidence
                .iter()
                .filter(|e| {
                    e.tree == self.frozen.tree && e.check_id == rule && e.command == signature
                })
                .max_by_key(|e| e.created_at)
            {
                let mut st = s.lock().unwrap();
                st.items[n].level = Level::Pass;
                st.items[n].label = format!("{title}: aprobada (evidencia del mismo candidato)");
                st.items[n].details = vec![e.output.clone()];
                continue;
            }
            let started = Instant::now();
            let on_step = |step: &str| {
                s.lock().unwrap().items[n].label = format!("{title} · {step}…");
                (self.notify)();
            };
            let outcome = router::review(
                self.config,
                self.rules,
                &ctx,
                security,
                spent,
                local,
                &on_step,
            );
            spent += outcome
                .steps
                .iter()
                .filter_map(|st| st.usage.as_ref())
                .filter(|u| !u.local)
                .map(|u| u.cost_usd)
                .sum::<f64>();

            let mut st = s.lock().unwrap();
            st.usage
                .extend(outcome.steps.iter().filter_map(|step| step.usage.clone()));
            let (level, word) = match outcome.verdict {
                AiVerdict::Approve => (Level::Pass, "aprobada"),
                AiVerdict::Changes => (Level::Warn, "pide cambios"),
                AiVerdict::Block => (Level::Block, "bloquea"),
                AiVerdict::Incomplete => (Level::Pending, "incompleta"),
            };
            let mut details = outcome.details();
            let level = match st.exception_for(&rule).cloned() {
                Some(e) if level != Level::Pass => {
                    details.insert(0, e.summary());
                    Level::Excepted
                }
                _ => level,
            };
            let cost = outcome.cost();
            st.items[n].level = level;
            st.items[n].label = format!("{title}: {word} · ${cost:.3}");
            st.items[n].details = details;
            if outcome.verdict == AiVerdict::Approve {
                st.extra_evidence.push(Evidence {
                    check_id: rule.clone(),
                    command: signature,
                    tree: self.frozen.tree.clone(),
                    duration_ms: started.elapsed().as_millis() as u64,
                    output: outcome.summary.clone(),
                    created_at: unix(SystemTime::now()),
                });
            }
            drop(st);
            (self.notify)();
        }
        self.run_reviewers(s, evidence, spent, &ctx, local);
    }

    /// Revisores especializados activados por el diff, en paralelo, y su comparación.
    fn run_reviewers(
        &self,
        s: &Arc<Mutex<GuardRun>>,
        evidence: &[Evidence],
        spent: f64,
        ctx: &ReviewContext,
        local: bool,
    ) {
        let jobs: Vec<(usize, Activation)> = {
            let st = s.lock().unwrap();
            st.items
                .iter()
                .enumerate()
                .filter_map(|(n, i)| {
                    let id = i.id.strip_prefix("reviewer:")?;
                    self.activations
                        .iter()
                        .find(|a| a.reviewer.id == id)
                        .map(|a| (n, a.clone()))
                })
                .collect()
        };
        if jobs.is_empty() {
            return;
        }
        let signature =
            |a: &Activation| router::reviewer_route(self.config, &a.reviewer, local).label();
        // Aprobaciones previas del mismo candidato (y misma ruta): no se repiten.
        let mut pending = Vec::new();
        for (n, a) in jobs {
            let rule = format!("reviewer:{}", a.reviewer.id);
            let reused = evidence
                .iter()
                .filter(|e| {
                    e.tree == self.frozen.tree && e.check_id == rule && e.command == signature(&a)
                })
                .max_by_key(|e| e.created_at);
            let mut st = s.lock().unwrap();
            match reused {
                Some(e) => {
                    st.items[n].level = Level::Pass;
                    st.items[n].label = format!(
                        "{}: aprueba (evidencia del mismo candidato)",
                        a.reviewer.name
                    );
                    st.items[n].details = vec![e.output.clone()];
                }
                None => {
                    st.items[n].label = format!(
                        "{} · revisando {} archivos…",
                        a.reviewer.name,
                        a.files.len()
                    );
                    pending.push((n, a));
                }
            }
        }
        (self.notify)();

        // ponytail: todos parten del mismo gasto del mes; con presupuesto muy justo se puede
        // superar como mucho en (revisores − 1) × max_cost_per_call_usd.
        let (config, rules) = (self.config, self.rules);
        let outcomes: Vec<(usize, Activation, AiOutcome)> = std::thread::scope(|scope| {
            let handles: Vec<_> = pending
                .into_iter()
                .map(|(n, a)| {
                    scope.spawn(move || {
                        let outcome = router::specialist(config, rules, ctx, &a, spent, local);
                        (n, a, outcome)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });

        let mut st = s.lock().unwrap();
        for (n, a, outcome) in &outcomes {
            let rule = format!("reviewer:{}", a.reviewer.id);
            st.usage
                .extend(outcome.steps.iter().filter_map(|step| step.usage.clone()));
            // Un revisor informativo nunca bloquea: sus objeciones quedan como aviso o nota.
            let (level, word) = match (outcome.verdict, a.reviewer.blocking) {
                (AiVerdict::Approve, _) => (Level::Pass, "aprueba"),
                (AiVerdict::Block, true) => (Level::Block, "bloquea"),
                (AiVerdict::Block, false) => (Level::Warn, "objeta (informativo)"),
                (AiVerdict::Changes, true) => (Level::Warn, "pide cambios"),
                (AiVerdict::Changes, false) => (Level::Info, "comenta"),
                (AiVerdict::Incomplete, true) => (Level::Pending, "incompleto"),
                (AiVerdict::Incomplete, false) => (Level::Info, "sin respuesta"),
            };
            let mut details = vec![format!("archivos: {}", a.files.join(", "))];
            details.extend(outcome.details());
            let level = match st.exception_for(&rule).cloned() {
                Some(e) if matches!(level, Level::Block | Level::Warn | Level::Pending) => {
                    details.insert(0, e.summary());
                    Level::Excepted
                }
                _ => level,
            };
            st.items[*n].level = level;
            st.items[*n].label = format!("{}: {word} · ${:.3}", a.reviewer.name, outcome.cost());
            st.items[*n].details = details;
            if outcome.verdict == AiVerdict::Approve {
                st.extra_evidence.push(Evidence {
                    check_id: rule,
                    command: signature(a),
                    tree: self.frozen.tree.clone(),
                    duration_ms: outcome
                        .steps
                        .iter()
                        .map(|s| (s.seconds * 1000.0) as u64)
                        .sum(),
                    output: outcome.summary.clone(),
                    created_at: unix(SystemTime::now()),
                });
            }
        }
        // Comparación entre revisores (plan §11): desacuerdos a la vista.
        if outcomes.len() > 1 {
            let summary: Vec<(String, AiVerdict, f64, String)> = outcomes
                .iter()
                .map(|(_, a, o)| {
                    let confidence = o
                        .steps
                        .iter()
                        .find_map(|s| s.review.as_ref())
                        .map_or(0.0, |r| r.confidence);
                    (
                        a.reviewer.name.clone(),
                        o.verdict,
                        confidence,
                        o.summary.clone(),
                    )
                })
                .collect();
            let (level, label, lines) = compare_reviewers(&summary);
            st.items
                .push(item("reviewers-compare", level, label, lines));
        }
        drop(st);
        (self.notify)();
    }
}

/// Compara a los revisores: si unos aprueban y otros objetan, se avisa.
fn compare_reviewers(
    outcomes: &[(String, AiVerdict, f64, String)],
) -> (Level, &'static str, Vec<String>) {
    let lines = outcomes
        .iter()
        .map(|(name, verdict, confidence, summary)| {
            format!("{name}: {verdict:?} (confianza {confidence:.2}) — {summary}")
        })
        .collect();
    let approves = outcomes.iter().any(|o| o.1 == AiVerdict::Approve);
    let objects = outcomes
        .iter()
        .any(|o| matches!(o.1, AiVerdict::Block | AiVerdict::Changes));
    if approves && objects {
        (
            Level::Warn,
            "Los revisores no coinciden: revisa sus objeciones",
            lines,
        )
    } else {
        (Level::Info, "Comparación de revisores: coinciden", lines)
    }
}

/// Ejecuta un check en un shell de login, en su propio grupo de procesos
/// (para poder matar todo al cancelar o al vencer el tiempo).
fn run_check(
    check: &Check,
    workdir: &Path,
    env: &std::collections::HashMap<String, String>,
    cancel: &AtomicBool,
) -> (CheckState, String) {
    use std::os::unix::process::CommandExt;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let child = Command::new(shell)
        .args(["-l", "-c", &format!("{{ {}\n}} 2>&1", check.command)])
        .current_dir(workdir)
        .envs(env)
        // Variables de los hooks: dentro de la copia aislada apuntarían al repositorio real.
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        // Modo no interactivo: vitest/jest no se quedan en "watch", sin colores ni prompts.
        .env("CI", "true")
        .env("FORCE_COLOR", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (CheckState::Error(e.to_string()), String::new()),
    };
    let mut stdout = child.stdout.take().expect("stdout con pipe");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(check.timeout_seconds.max(1));
    let pgid = child.id() as libc::pid_t;
    let state = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break CheckState::Passed,
            Ok(Some(status)) => break CheckState::Failed(status.code().unwrap_or(-1)),
            Err(e) => break CheckState::Error(e.to_string()),
            Ok(None) => {}
        }
        let stop = if cancel.load(Ordering::Relaxed) {
            Some(CheckState::Cancelled)
        } else if Instant::now() >= deadline {
            Some(CheckState::TimedOut)
        } else {
            None
        };
        if let Some(stop) = stop {
            // SAFETY: killpg solo envía señales al grupo creado con process_group(0).
            unsafe { libc::killpg(pgid, libc::SIGTERM) };
            std::thread::sleep(Duration::from_millis(500));
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
            let _ = child.wait();
            break stop;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = reader.join().unwrap_or_default();
    (
        state,
        tail(
            &crate::terminal::strip_ansi(&String::from_utf8_lossy(&output)),
            80,
        ),
    )
}

/// Últimas `n` líneas.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("package-lock.json", "web/package-lock.json"));
        assert!(glob_match("*.generated.ts", "src/api/types.generated.ts"));
        assert!(glob_match(
            "drizzle/meta/**",
            "drizzle/meta/0001_snapshot.json"
        ));
        assert!(!glob_match("drizzle/meta/**", "src/drizzle/meta/x.json"));
        assert!(glob_match("**/meta/*.json", "src/drizzle/meta/x.json"));
        assert!(glob_match("coverage/", "coverage/lcov/index.html"));
        assert!(glob_match(".env.*", "apps/api/.env.local"));
        assert!(!glob_match("*.key", "src/key.ts"));
        assert!(glob_match("id_rs?", "id_rsa"));
    }

    #[test]
    fn parses_git_output() {
        let files = parse_numstat("10\t2\tsrc/a.ts\0-\t-\tlogo.png\0");
        assert_eq!(
            files[0],
            FileChange {
                path: "src/a.ts".into(),
                added: 10,
                deleted: 2,
                binary: false,
                removed: false
            }
        );
        assert!(files[1].binary);

        let patch = "diff --git a/x b/x\n--- a/x\n+++ b/src/x.ts\n@@ -3,0 +4,2 @@\n+const a = 1;\n+const b = 2;\n@@ -10 +12 @@\n-old\n+new\n";
        let lines = parse_added_lines(patch);
        assert_eq!(
            lines,
            vec![
                ("src/x.ts".into(), 4, "const a = 1;".into()),
                ("src/x.ts".into(), 5, "const b = 2;".into()),
                ("src/x.ts".into(), 12, "new".into()),
            ]
        );
    }

    #[test]
    fn detects_and_masks_secrets() {
        let line = |t: &str| ("a.ts".to_string(), 1, t.to_string());
        let found = scan_secrets(&[
            line("const k = 'AKIAIOSFODNN7EXAMPLE';"), // forge:allow-secret
            line("-----BEGIN OPENSSH PRIVATE KEY-----"), // forge:allow-secret
            line("ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz"), // forge:allow-secret
            line(r#"password: "hunter2hunter2""#),     // forge:allow-secret
            line("const password = process.env.DB_PASSWORD;"),
            line("const apiKey = getKey();"),
        ]);
        let kinds: Vec<&str> = found.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            [
                "clave de AWS",
                "clave privada",
                "clave de Anthropic",
                "contraseña o clave en el código"
            ]
        );
        assert!(
            found
                .iter()
                .all(|f| f.preview.ends_with("••••") && f.preview.chars().count() <= 10)
        );
        let allowed = format!("const k = 'AKIAIOSFODNN7EXAMPLE'; // {ALLOW_SECRET}"); // forge:allow-secret
        assert!(scan_secrets(&[line(&allowed)]).is_empty());
    }

    fn rules(toml_text: &str) -> Rules {
        toml::from_str(toml_text).unwrap()
    }

    #[test]
    fn evaluation_levels() {
        let r = rules(
            "[quality]\nmax_changed_lines = 100\nwarning_changed_lines = 50\n[commit]\nrequire_tests = true\nrequire_ai_review = true\n",
        );
        let file = |path: &str, added| FileChange {
            path: path.into(),
            added,
            deleted: 0,
            binary: false,
            removed: false,
        };
        let diff = Diff {
            files: vec![
                file("src/a.ts", 60),
                file("package-lock.json", 5000),
                file(".env", 1),
                file(".env.example", 1),
            ],
            added_lines: vec![(
                "src/a.ts".into(),
                3,
                "token = 'ghp_abcdefghijklmnopqrstuvwxyz0123456789'".into(), // forge:allow-secret
            )],
            ..Default::default()
        };
        let items = evaluate(&r, Stage::Commit, &diff);
        let level = |prefix: &str| {
            items
                .iter()
                .find(|i| i.label.contains(prefix))
                .map(|i| i.level)
        };
        assert_eq!(level("líneas"), Some(Level::Warn)); // 62 > 50, el lockfile no cuenta
        assert_eq!(level("prohibidos"), Some(Level::Block)); // .env sí, .env.example no
        assert_eq!(
            items
                .iter()
                .find(|i| i.label.contains("prohibidos"))
                .unwrap()
                .details,
            vec![".env"]
        );
        assert_eq!(level("secretos"), Some(Level::Block));
        assert_eq!(level("require_tests"), Some(Level::Block)); // falta [[checks]] tests
        assert_eq!(level("Revisión de IA"), Some(Level::Pending));
        assert_eq!(thousands(1284), "1.284");
        assert_eq!(thousands(4000000), "4.000.000");
    }

    #[test]
    fn template_round_trips() {
        for (file, content, expected) in [
            ("Cargo.toml", "[package]\nname = \"x\"\n", 4),
            (
                "package.json",
                r#"{"scripts":{"lint":"eslint .","test":"vitest","dev":"vite"}}"#,
                2,
            ),
        ] {
            let dir =
                std::env::temp_dir().join(format!("forge-rules-{}-{file}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), content).unwrap();
            write_template(&dir).unwrap();
            let rules = Rules::load(&dir).unwrap().unwrap();
            assert_eq!(rules.checks.len(), expected, "{file}");
            let (checks, missing) = rules.checks_for(Stage::Commit);
            assert!(missing.is_empty(), "{file}: faltan {missing:?}");
            assert!(checks.iter().any(|c| c.id == "tests"));
            assert!(write_template(&dir).is_err(), "no debe sobrescribir");
        }
    }

    fn wait(handle: &Handle) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !handle.snapshot(GuardRun::finished) {
            assert!(Instant::now() < deadline, "el guard no terminó");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Fase 7: los checks prueban el candidato exacto y la evidencia solo vale para él.
    #[test]
    fn checks_run_on_frozen_candidate_and_reuse_evidence() {
        let dir = std::env::temp_dir().join(format!("forge-frozen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let sh = |cmd: &str| {
            assert!(
                Command::new("sh")
                    .args(["-c", cmd])
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success(),
                "{cmd}"
            )
        };
        sh("git init -q -b main && git config user.email t@t && git config user.name t");
        sh("echo v0 > a.txt && git add . && git commit -qm inicio");
        sh("echo v1 > a.txt && git add a.txt && echo 'v2 sin preparar' > a.txt");
        let counter = dir
            .parent()
            .unwrap()
            .join(format!("forge-frozen-count-{}", std::process::id()));
        let _ = std::fs::remove_file(&counter);
        let rules = |_: &Path| -> Rules {
            toml::from_str(&format!(
                "[commit]\nrequire_tests = true\n[[checks]]\nid = \"tests\"\ncommand = \"echo x >> {} && grep -q v1 a.txt\"\n",
                counter.display()
            ))
            .unwrap()
        };
        let options = |evidence| RunOptions {
            env: Default::default(),
            exceptions: vec![],
            push_range: None,
            evidence,
            name: "frozen".into(),
            routing: None,
            month_spent: 0.0,
            reviewers: vec![],
        };

        // 1. El árbol de trabajo tiene v2, pero el commit lleva v1: el check debe ver v1.
        let first = run(
            &dir,
            &dir,
            rules(&dir),
            Stage::Commit,
            options(vec![]),
            || {},
        );
        wait(&first);
        let evidence = first.snapshot(|r| {
            assert!(
                r.frozen.as_ref().is_some_and(|c| c.isolated),
                "debe aislarse"
            );
            assert_eq!(
                r.checks[0].state,
                CheckState::Passed,
                "salida: {}",
                r.checks[0].output
            );
            r.new_evidence()
        });
        assert_eq!(evidence.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "v2 sin preparar\n",
            "no se toca el árbol"
        );

        // 2. Mismo candidato: se reutiliza la evidencia y el check no se vuelve a ejecutar.
        let second = run(
            &dir,
            &dir,
            rules(&dir),
            Stage::Commit,
            options(evidence.clone()),
            || {},
        );
        wait(&second);
        second.snapshot(|r| {
            assert!(r.checks[0].reused.is_some(), "debía reutilizar");
            assert_eq!(r.verdict(), Verdict::Ready);
        });
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            1,
            "se ejecutó de nuevo"
        );

        // 3. Otro contenido preparado = otro candidato: la evidencia vieja no sirve.
        sh("echo v3 > a.txt && git add a.txt");
        let third = run(
            &dir,
            &dir,
            rules(&dir),
            Stage::Commit,
            options(evidence),
            || {},
        );
        wait(&third);
        third.snapshot(|r| {
            assert!(r.checks[0].reused.is_none(), "evidencia de otra versión");
            assert!(
                matches!(r.checks[0].state, CheckState::Failed(_)),
                "v3 no contiene v1"
            );
        });
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
    }

    #[test]
    fn reviewers_disagreement() {
        let o = |name: &str, verdict| (name.to_string(), verdict, 0.9, String::new());
        let (level, label, lines) = compare_reviewers(&[
            o("BD", AiVerdict::Approve),
            o("Seguridad", AiVerdict::Block),
        ]);
        assert_eq!(
            (level, label.contains("no coinciden"), lines.len()),
            (Level::Warn, true, 2)
        );
        let (level, _, _) =
            compare_reviewers(&[o("BD", AiVerdict::Block), o("UI", AiVerdict::Changes)]);
        assert_eq!(level, Level::Info);
    }

    /// Repo real: diff preparado, checks en paralelo, fallo, timeout y veredicto.
    #[test]
    fn end_to_end_on_real_repo() {
        let dir = std::env::temp_dir().join(format!("forge-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        let sh = |cmd: &str| {
            assert!(
                Command::new("sh")
                    .args(["-c", cmd])
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success(),
                "{cmd}"
            )
        };
        sh("git init -q && git config user.email t@t && git config user.name t");
        std::fs::write(dir.join("a.txt"), "uno\n").unwrap();
        sh("git add a.txt && git commit -qm inicio");
        std::fs::write(dir.join("a.txt"), "uno\ndos\ntres\n").unwrap();
        std::fs::write(dir.join("nuevo.txt"), "x\n").unwrap();
        sh("git add a.txt");

        let r = rules(
            "[commit]\nrequire_lint = true\nrequire_tests = true\n\
             [[checks]]\nid = \"lint\"\ncommand = \"echo lint ok\"\n\
             [[checks]]\nid = \"tests\"\ncommand = \"echo fallo de prueba; exit 4\"\n\
             [[checks]]\nid = \"lento\"\ncommand = \"sleep 30\"\ntimeout_seconds = 1\nseverity = \"warning\"\nstages = [\"commit\"]\n",
        );
        let diff = collect_diff(&dir, Stage::Commit, &r, None).unwrap();
        assert!(!diff.unstaged);
        assert_eq!(diff.files.len(), 1, "solo lo preparado: {:?}", diff.files);
        assert_eq!((diff.files[0].added, diff.files[0].deleted), (2, 0));

        let handle = run(
            &dir,
            &dir,
            r,
            Stage::Commit,
            RunOptions {
                env: Default::default(),
                exceptions: vec![],
                push_range: None,
                evidence: vec![],
                name: "test".into(),
                routing: None,
                month_spent: 0.0,
                reviewers: vec![],
            },
            || {},
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while !handle.snapshot(GuardRun::finished) {
            assert!(Instant::now() < deadline, "el guard no terminó");
            std::thread::sleep(Duration::from_millis(50));
        }
        handle.snapshot(|run| {
            let state = |id: &str| {
                run.checks
                    .iter()
                    .find(|c| c.check.id == id)
                    .unwrap()
                    .state
                    .clone()
            };
            assert_eq!(state("lint"), CheckState::Passed);
            assert_eq!(state("tests"), CheckState::Failed(4));
            assert_eq!(state("lento"), CheckState::TimedOut);
            let tests = run.checks.iter().find(|c| c.check.id == "tests").unwrap();
            assert!(tests.output.contains("fallo de prueba"));
            assert_eq!(run.verdict(), Verdict::Blocked);
        });

        // Nada preparado: evalúa todo el árbol de trabajo, incluidos los archivos nuevos.
        sh("git reset -q");
        let diff = collect_diff(&dir, Stage::Commit, &Rules::default(), None).unwrap();
        assert!(diff.unstaged);
        let mut paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["a.txt", "nuevo.txt"]);
    }
}

#[cfg(test)]
mod dogfood {
    use super::*;

    /// Guard sobre el propio repositorio de Forge (manual):
    /// `cargo test --release dogfood -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn guard_on_forge_itself() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let rules: Rules = toml::from_str(&template(&detect_checks(root))).unwrap();
        // Directorio de compilación aparte para no bloquear el `cargo test` que nos ejecuta.
        let target = std::env::temp_dir().join("forge-dogfood-target");
        let handle = run(
            root,
            root,
            rules,
            Stage::Commit,
            RunOptions {
                env: Default::default(),
                exceptions: vec![],
                push_range: None,
                evidence: vec![],
                name: "test".into(),
                routing: None,
                month_spent: 0.0,
                reviewers: vec![],
            },
            || {},
        );
        unsafe { std::env::set_var("CARGO_TARGET_DIR", &target) };
        while !handle.snapshot(GuardRun::finished) {
            std::thread::sleep(Duration::from_millis(200));
        }
        handle.snapshot(|r| {
            for i in &r.items {
                println!("{:?} {} {:?}", i.level, i.label, i.details);
            }
            for c in &r.checks {
                println!(
                    "{:?} {} {:?}\n{}",
                    c.state,
                    c.check.id,
                    c.duration,
                    tail(&c.output, 15)
                );
            }
            println!("VEREDICTO: {:?} {:?}", r.verdict(), r.progress());
        });
    }
}
