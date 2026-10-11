// Con qué modelo y esfuerzo se abren los agentes desde Forge: un valor general por
// agente y, si se quiere, otro para un proyecto. Vacío = lo que tenga el agente en su
// propia configuración (Forge no lo toca).
use std::path::Path;

use crate::store::Store;

/// Niveles de esfuerzo que entiende cada agente (de menos a más).
pub fn efforts(program: &str) -> &'static [&'static str] {
    match program {
        "claude" => &["low", "medium", "high", "xhigh", "max"],
        "codex" => &["low", "medium", "high", "xhigh"],
        _ => &[],
    }
}

/// Modelo y esfuerzo que tiene Codex en su propia configuración (`config.toml` de su
/// carpeta), para decir en Ajustes qué significa «Tu configuración».
pub fn codex_defaults(codex_home: &Path) -> (String, String) {
    let table: toml::Table = std::fs::read_to_string(codex_home.join("config.toml"))
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    let get = |k: &str| {
        table
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    (get("model"), get("model_reasoning_effort"))
}

/// `model` o `effort`.
fn key(program: &str, what: &str, project: Option<&str>) -> String {
    match project {
        Some(p) => format!("launch:{program}:{what}:{p}"),
        None => format!("launch:{program}:{what}"),
    }
}

impl Store {
    /// El valor guardado (general si `project` es `None`); vacío si no hay.
    pub fn launch_pref(&self, program: &str, what: &str, project: Option<&Path>) -> String {
        let project = project.map(|p| self.project_key(p));
        let saved = self
            .setting(&key(program, what, project.as_deref()))
            .ok()
            .flatten();
        // El modelo de Claude se guardaba antes como `claude_model`.
        let legacy = || {
            (program == "claude" && what == "model" && project.is_none())
                .then(|| self.setting("claude_model").ok().flatten())
                .flatten()
        };
        saved.or_else(legacy).unwrap_or_default()
    }

    pub fn set_launch_pref(
        &self,
        program: &str,
        what: &str,
        project: Option<&Path>,
        value: &str,
    ) -> Result<(), String> {
        let project = project.map(|p| self.project_key(p));
        self.set_setting(&key(program, what, project.as_deref()), value.trim())
    }

    /// Modo «Primero el test» en el proyecto: el suyo («on»/«off») si lo tiene, si no
    /// el general.
    pub fn tests_first(&self, project: &Path) -> bool {
        let own = self.launch_pref("forge", "tests_first", Some(project));
        let value = if own.is_empty() {
            self.launch_pref("forge", "tests_first", None)
        } else {
            own
        };
        value == "on"
    }

    /// Lo que se usa al abrir el agente en el proyecto: lo del proyecto si lo hay, si no
    /// lo general. (modelo, esfuerzo).
    pub fn launch_for(&self, program: &str, project: &Path) -> (String, String) {
        let pick = |what: &str| {
            let own = self.launch_pref(program, what, Some(project));
            if own.is_empty() {
                self.launch_pref(program, what, None)
            } else {
                own
            }
        };
        (pick("model"), pick("effort"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_first_project_overrides_general() {
        let store = Store::in_memory().unwrap();
        let (a, b) = (Path::new("/p/a"), Path::new("/p/b"));
        assert!(!store.tests_first(a));
        store
            .set_launch_pref("forge", "tests_first", None, "on")
            .unwrap();
        store
            .set_launch_pref("forge", "tests_first", Some(a), "off")
            .unwrap();
        assert!(!store.tests_first(a), "el proyecto manda");
        assert!(store.tests_first(b), "otro proyecto: lo general");
    }

    #[test]
    fn project_overrides_general_and_legacy_model_still_counts() {
        let store = Store::in_memory().unwrap();
        let (a, b) = (Path::new("/p/a"), Path::new("/p/b"));
        store.set_setting("claude_model", "opus").unwrap();
        assert_eq!(
            store.launch_for("claude", a),
            ("opus".into(), String::new())
        );
        store
            .set_launch_pref("claude", "effort", None, "medium")
            .unwrap();
        store
            .set_launch_pref("claude", "effort", Some(a), "max")
            .unwrap();
        assert_eq!(store.launch_for("claude", a).1, "max");
        assert_eq!(
            store.launch_for("claude", b).1,
            "medium",
            "otro proyecto: lo general"
        );
        store
            .set_launch_pref("claude", "effort", Some(a), "")
            .unwrap();
        assert_eq!(
            store.launch_for("claude", a).1,
            "medium",
            "vacío vuelve a lo general"
        );
        store
            .set_launch_pref("codex", "model", None, "gpt-5")
            .unwrap();
        assert_eq!(
            store.launch_for("codex", b),
            ("gpt-5".into(), String::new())
        );
        assert!(efforts("codex").contains(&"xhigh") && !efforts("codex").contains(&"max"));

        let dir = std::env::temp_dir().join(format!("forge-codex-defaults-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "model = \"gpt-5\"\nmodel_reasoning_effort = \"medium\"\n[x]\ny = 1\n",
        )
        .unwrap();
        assert_eq!(codex_defaults(&dir), ("gpt-5".into(), "medium".into()));
        assert_eq!(
            codex_defaults(Path::new("/nonexistent")),
            (String::new(), String::new())
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
