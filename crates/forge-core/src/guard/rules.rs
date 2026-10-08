// Reglas (.forge/rules.toml), etapas y plantilla con checks detectados.
use super::*;
use crate::tr;

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

pub(crate) fn default_timeout() -> u64 {
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
            Stage::Commit => tr!("Commit"),
            Stage::Push => tr!("Push"),
            Stage::PullRequest => tr!("Pull request"),
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
            return Err(tr!(
                "{p0}: el check \"{p1}\" está repetido",
                p0 = path.display(),
                p1 = c.id
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
        return Err(tr!("{p0} ya existe", p0 = file.display()));
    }
    std::fs::create_dir_all(project.join(".forge")).map_err(|e| e.to_string())?;
    std::fs::write(&file, template(&detect_checks(project)))
        .map_err(|e| format!("{}: {e}", file.display()))
}

pub(crate) fn detect_checks(project: &Path) -> Vec<(&'static str, &'static str, String)> {
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

pub(crate) fn template(checks: &[(&str, &str, String)]) -> String {
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
