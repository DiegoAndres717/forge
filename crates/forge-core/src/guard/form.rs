// Formulario de Ajustes para `.forge/rules.toml`: lo que se edita sin tocar el archivo.
// Al guardar se cambian solo esos valores, así que los comentarios se conservan.
use super::*;
use crate::tr;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

/// Comprobaciones que las casillas de cada etapa piden por su id.
pub const STANDARD_CHECKS: [&str; 4] = ["lint", "typecheck", "tests", "build"];

/// Qué se exige en una etapa (commit, push o pull request).
#[derive(Clone, Debug, PartialEq)]
pub struct StageForm {
    pub enabled: bool,
    pub lint: bool,
    pub typecheck: bool,
    pub tests: bool,
    pub build: bool,
    pub ai_review: bool,
}

impl StageForm {
    fn from(s: &StageRules) -> Self {
        Self {
            enabled: s.enabled,
            lint: s.require_lint,
            typecheck: s.require_typecheck,
            tests: s.require_tests,
            build: s.require_build,
            ai_review: s.require_ai_review,
        }
    }

    /// Ids de las comprobaciones que pide esta etapa.
    pub fn required(&self) -> Vec<&'static str> {
        [self.lint, self.typecheck, self.tests, self.build]
            .iter()
            .zip(STANDARD_CHECKS)
            .filter(|(on, _)| **on)
            .map(|(_, id)| id)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckForm {
    pub id: String,
    pub name: String,
    pub command: String,
    /// Si falla, no deja seguir (si no, solo avisa).
    pub blocking: bool,
    pub timeout_seconds: u64,
    /// Etapas en las que corre siempre (para las que no son de las casillas).
    pub stages: Vec<Stage>,
}

impl CheckForm {
    pub fn new() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            command: String::new(),
            blocking: true,
            timeout_seconds: default_timeout(),
            stages: Vec::new(),
        }
    }
}

impl Default for CheckForm {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RulesForm {
    /// Ya hay `.forge/rules.toml` (si no, el formulario parte de la plantilla).
    pub exists: bool,
    pub max_changed_lines: usize,
    pub warning_changed_lines: usize,
    pub max_files_changed: usize,
    pub block_secrets: bool,
    pub block_env_files: bool,
    pub forbidden_files: Vec<String>,
    /// Commit, push y pull request (el orden de `Stage::ALL`).
    pub stages: [StageForm; 3],
    pub checks: Vec<CheckForm>,
}

fn file(project: &Path) -> PathBuf {
    project.join(".forge/rules.toml")
}

/// El texto actual del archivo o, si no existe, la plantilla con lo detectado.
fn source(project: &Path) -> Result<(bool, String), String> {
    match std::fs::read_to_string(file(project)) {
        Ok(text) => Ok((true, text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok((false, template(&detect_checks(project))))
        }
        Err(e) => Err(format!("{}: {e}", file(project).display())),
    }
}

impl RulesForm {
    pub fn load(project: &Path) -> Result<Self, String> {
        let (exists, text) = source(project)?;
        let rules: Rules =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", file(project).display()))?;
        let q = &rules.quality;
        Ok(Self {
            exists,
            max_changed_lines: q.max_changed_lines,
            warning_changed_lines: q.warning_changed_lines,
            max_files_changed: q.max_files_changed,
            block_secrets: q.block_secrets,
            block_env_files: q.block_env_files,
            forbidden_files: q.forbidden_files.clone(),
            stages: Stage::ALL.map(|s| StageForm::from(rules.stage(s))),
            checks: rules
                .checks
                .iter()
                .map(|c| CheckForm {
                    id: c.id.clone(),
                    name: c.name.clone().unwrap_or_default(),
                    command: c.command.clone(),
                    blocking: c.severity == Severity::Blocking,
                    timeout_seconds: c.timeout_seconds,
                    stages: c.stages.clone(),
                })
                .collect(),
        })
    }

    /// Ids que alguna etapa pide y que no tienen comprobación.
    pub fn missing_checks(&self) -> Vec<&'static str> {
        STANDARD_CHECKS
            .into_iter()
            .filter(|id| self.stages.iter().any(|s| s.required().contains(id)))
            .filter(|id| !self.checks.iter().any(|c| c.id == *id))
            .collect()
    }

    /// Añade las comprobaciones detectadas (npm o Cargo) que aún no estén. Cuántas añadió.
    pub fn detect(&mut self, project: &Path) -> usize {
        let mut added = 0;
        for (id, name, command) in detect_checks(project) {
            if !self.checks.iter().any(|c| c.id == id) {
                self.checks.push(CheckForm {
                    id: id.into(),
                    name: name.into(),
                    command,
                    ..CheckForm::new()
                });
                added += 1;
            }
        }
        added
    }

    /// Guarda en `.forge/rules.toml` (lo crea si no existe). Comprueba antes que lo que se
    /// escribe se puede leer, así nunca deja el archivo roto.
    pub fn save(&self, project: &Path) -> Result<(), String> {
        let (_, text) = source(project)?;
        let mut doc: DocumentMut = text
            .parse()
            .map_err(|e| format!("{}: {e}", file(project).display()))?;
        let list = |items: &[String]| Array::from_iter(items.iter().map(String::as_str));
        let q = &mut doc["quality"];
        q["max_changed_lines"] = value(self.max_changed_lines as i64);
        q["warning_changed_lines"] = value(self.warning_changed_lines as i64);
        q["max_files_changed"] = value(self.max_files_changed as i64);
        q["block_secrets"] = value(self.block_secrets);
        q["block_env_files"] = value(self.block_env_files);
        q["forbidden_files"] = value(list(&self.forbidden_files));
        for (key, s) in ["commit", "push", "pull_request"].iter().zip(&self.stages) {
            let t = &mut doc[key];
            t["enabled"] = value(s.enabled);
            t["require_lint"] = value(s.lint);
            t["require_typecheck"] = value(s.typecheck);
            t["require_tests"] = value(s.tests);
            t["require_build"] = value(s.build);
            if s.ai_review || t.get("require_ai_review").is_some() {
                t["require_ai_review"] = value(s.ai_review);
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut checks = ArrayOfTables::new();
        for c in &self.checks {
            let id = c.id.trim();
            if id.is_empty() || c.command.trim().is_empty() {
                return Err(tr!("Cada comprobación necesita un id y un comando.").into());
            }
            if !seen.insert(id) {
                return Err(tr!("Hay dos comprobaciones con el id «{id}».", id = id));
            }
            let mut t = Table::new();
            t["id"] = value(id);
            if !c.name.trim().is_empty() {
                t["name"] = value(c.name.trim());
            }
            t["command"] = value(c.command.trim());
            if !c.blocking {
                t["severity"] = value("warning");
            }
            if c.timeout_seconds != default_timeout() {
                t["timeout_seconds"] = value(c.timeout_seconds as i64);
            }
            if !c.stages.is_empty() {
                let stages = c.stages.iter().map(|s| match s {
                    Stage::Commit => "commit",
                    Stage::Push => "push",
                    Stage::PullRequest => "pull-request",
                });
                t["stages"] = value(Array::from_iter(stages));
            }
            checks.push(t);
        }
        if checks.is_empty() {
            doc.remove("checks");
        } else {
            doc["checks"] = Item::ArrayOfTables(checks);
        }
        let out = doc.to_string();
        toml::from_str::<Rules>(&out).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(project.join(".forge")).map_err(|e| e.to_string())?;
        std::fs::write(file(project), out).map_err(|e| format!("{}: {e}", file(project).display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_edits_rules_keeping_comments() {
        let dir = std::env::temp_dir().join(format!("forge-rules-form-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"lint":"eslint .","test":"vitest run"}}"#,
        )
        .unwrap();

        // Sin archivo: la plantilla con lo detectado.
        let mut form = RulesForm::load(&dir).unwrap();
        assert!(!form.exists);
        assert_eq!(form.checks.len(), 2);
        assert!(form.stages[0].tests && form.missing_checks().is_empty());

        // Cambios y guardado: crea el archivo y se puede volver a leer igual.
        form.max_files_changed = 80;
        form.stages[0].tests = false;
        form.stages[1].build = true;
        form.forbidden_files.push("*.sqlite".into());
        form.checks[0].blocking = false;
        form.checks.push(CheckForm {
            id: "e2e".into(),
            command: "npm run e2e".into(),
            stages: vec![Stage::PullRequest],
            ..CheckForm::new()
        });
        assert_eq!(form.missing_checks(), ["build"], "push pide build y no hay");
        form.save(&dir).unwrap();
        let again = RulesForm::load(&dir).unwrap();
        assert!(again.exists);
        assert_eq!(
            RulesForm {
                exists: false,
                ..again
            },
            RulesForm {
                exists: false,
                ..form.clone()
            }
        );
        let rules = Rules::load(&dir).unwrap().unwrap();
        let ids = |stage| -> Vec<String> {
            rules
                .checks_for(stage)
                .0
                .iter()
                .map(|c| c.id.clone())
                .collect()
        };
        assert!(
            ids(Stage::PullRequest).contains(&"e2e".to_string()),
            "e2e corre en PR"
        );
        assert!(!ids(Stage::Commit).contains(&"e2e".to_string()));

        // Los comentarios escritos a mano siguen ahí.
        let path = dir.join(".forge/rules.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("# mi nota\n{text}")).unwrap();
        form.max_changed_lines = 5000;
        form.save(&dir).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# mi nota") && text.contains("Project Guard"),
            "{text}"
        );
        assert!(text.contains("max_changed_lines = 5000"));

        // Lo inválido no se escribe.
        let mut bad = form.clone();
        bad.checks.push(CheckForm {
            id: "e2e".into(),
            command: "x".into(),
            ..CheckForm::new()
        });
        assert!(bad.save(&dir).unwrap_err().contains("e2e"));
        bad.checks.pop();
        bad.checks.push(CheckForm::new());
        assert!(bad.save(&dir).is_err());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("max_changed_lines = 5000")
        );

        // Detectar añade solo lo que falta.
        let mut fresh = RulesForm::load(&dir).unwrap();
        fresh.checks.retain(|c| c.id != "lint");
        assert_eq!(fresh.detect(&dir), 1);
        assert_eq!(fresh.detect(&dir), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
