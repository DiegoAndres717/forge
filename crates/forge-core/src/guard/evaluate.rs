// Evaluación determinista del diff y excepciones con motivo.
use super::*;

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

pub(crate) fn item(
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
pub(crate) fn apply_exceptions(items: &mut [Item], exceptions: &[Exception]) {
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

pub(crate) fn env_allowed(path: &str) -> bool {
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
