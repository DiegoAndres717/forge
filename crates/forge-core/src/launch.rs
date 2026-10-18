// Con qué modelo y esfuerzo se abren los agentes desde Forge: un valor general por
// agente y, si se quiere, otro para un proyecto. Vacío = lo que tenga el agente en su
// propia configuración (Forge no lo toca).
use std::path::Path;

use crate::store::Store;

/// Niveles de esfuerzo que entiende cada agente (de menos a más).
pub fn efforts(program: &str) -> &'static [&'static str] {
    match program {
        "claude" | "agy" => &["low", "medium", "high", "xhigh", "max"],
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

    /// Forge Jev, tareas de git a un subagente barato: con el interruptor general encendido,
    /// salvo que esta parte se apague. Apagado de serie.
    pub fn jev_chores(&self) -> bool {
        self.launch_pref("forge", "jev", None) == "on"
            && self.launch_pref("forge", "jev_chores", None) != "off"
    }

    /// Forge Jev, el modelo al abrir Claude con un pedido: con el interruptor general, salvo
    /// que esta parte se apague.
    pub fn jev_launch(&self) -> bool {
        self.launch_pref("forge", "jev", None) == "on"
            && self.launch_pref("forge", "jev_launch", None) != "off"
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

/// Palabras de la regla de Forge Jev. El mod de Claude Code (register.tsx) tiene las mismas
/// listas para sugerir el modelo dentro de la sesión; una prueba comprueba que coincidan.
pub const JEV_HARD_STEMS: &[&str] = &[
    "diseñ",
    "design",
    "architect",
    "arquitect",
    "investig",
    "audit",
    "migra",
    "refactor",
];
pub const JEV_HARD: &[&str] = &[
    "bug",
    "bugs",
    "crash",
    "falla",
    "fallan",
    "falló",
    "fallo",
    "fallos",
    "fails",
    "failing",
    "error",
    "errores",
    "broken",
    "roto",
    "rota",
    "regression",
    "regresión",
    "deadlock",
    "race",
    "plan",
    "plans",
    "planes",
    "planifica",
    "planificar",
    "why",
];
/// Haiku pide al menos dos distintas: «merge sort» o «checkout button» no son tareas de git.
pub const JEV_GIT: &[&str] = &[
    "commit",
    "push",
    "pull",
    "merge",
    "rebase",
    "rama",
    "branch",
    "pr",
    "changeset",
    "tag",
    "stash",
    "checkout",
];

/// Forge Jev: el modelo de Claude para un pedido. Diseño, bugs, investigar o planificar →
/// `opus`; una tarea corta de git (dos palabras de git o más) → `haiku`; lo demás → `sonnet`. Medido en sesiones reales,
/// esta regla da ~80 % del ahorro posible sin bajar nunca de lo que hacía falta (plan #38).
// ponytail: palabras clave; si la medición de la fase 7 muestra muchos fallos, un modelo.
pub fn jev_claude_model(prompt: &str) -> &'static str {
    let text = prompt.to_lowercase();
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let why = words
        .windows(2)
        .any(|w| w[0] == "por" && matches!(w[1], "qué" | "que"));
    let hard = words
        .iter()
        .any(|w| JEV_HARD.contains(w) || JEV_HARD_STEMS.iter().any(|s| w.starts_with(s)));
    if why || hard {
        return "opus";
    }
    let mut git: Vec<&&str> = words.iter().filter(|w| JEV_GIT.contains(w)).collect();
    git.sort();
    git.dedup();
    if prompt.chars().count() < 200 && git.len() >= 2 {
        return "haiku";
    }
    "sonnet"
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
    fn forge_jev_is_off_by_default_and_its_parts_need_the_master_switch() {
        let store = Store::in_memory().unwrap();
        assert!(!store.jev_chores());
        // Encender la parte sola no basta: manda el interruptor general.
        store
            .set_launch_pref("forge", "jev_chores", None, "on")
            .unwrap();
        assert!(!store.jev_chores());
        store.set_launch_pref("forge", "jev", None, "on").unwrap();
        assert!(store.jev_chores());
        // Con Forge Jev encendido, cada parte está encendida salvo que se apague.
        store
            .set_launch_pref("forge", "jev_chores", None, "")
            .unwrap();
        assert!(store.jev_chores());
        store
            .set_launch_pref("forge", "jev_chores", None, "off")
            .unwrap();
        assert!(!store.jev_chores());
    }

    #[test]
    fn forge_jev_picks_the_claude_model_from_the_prompt() {
        // Tareas de git cortas: el modelo barato.
        for p in [
            "haz commit y push",
            "abre el PR de esta rama",
            "merge main into this branch",
        ] {
            assert_eq!(jev_claude_model(p), "haiku", "{p}");
        }
        // Diseño, bugs que fallan, investigar o planificar: el fuerte.
        for p in [
            "¿Por qué falla el login después de actualizar?",
            "Diseña la arquitectura del sistema de pagos",
            "Plan a phased migration of the store to SQLite",
            "investiga este crash en Windows",
        ] {
            assert_eq!(jev_claude_model(p), "opus", "{p}");
        }
        // Lo demás: el intermedio.
        for p in [
            "Añade un botón para exportar a CSV",
            "rename the field and update the tests",
        ] {
            assert_eq!(jev_claude_model(p), "sonnet", "{p}");
        }
        // Una sola palabra de git en un pedido normal no lo manda a Haiku (revisión).
        for p in [
            "Add a tag field to the product form",
            "fix the checkout button color",
            "implement merge sort",
            "add pull-to-refresh",
            "implement push notifications",
            "Añade un campo rama al formulario",
        ] {
            assert_eq!(jev_claude_model(p), "sonnet", "{p}");
        }
        // Un defecto pesa más que la tarea de git.
        for p in [
            "merge and fix the broken test",
            "push el arreglo de la regresión",
            "migra la base a Postgres",
        ] {
            assert_eq!(jev_claude_model(p), "opus", "{p}");
        }
        // Un commit dentro de un pedido largo no lo vuelve una tarea de git.
        let long = format!(
            "{} y luego haz commit",
            "Implementa la vista de facturas con filtros ".repeat(5)
        );
        assert_eq!(jev_claude_model(&long), "sonnet");

        let store = Store::in_memory().unwrap();
        assert!(!store.jev_launch());
        store.set_launch_pref("forge", "jev", None, "on").unwrap();
        assert!(store.jev_launch());
        store
            .set_launch_pref("forge", "jev_launch", None, "off")
            .unwrap();
        assert!(!store.jev_launch());

        // El mod sugiere el modelo con la misma regla.
        let module = include_str!("../../../claude-plugin/hooks/register.tsx");
        for list in [JEV_HARD_STEMS, JEV_HARD, JEV_GIT] {
            assert!(module.contains(&format!("{list:?}")), "{list:?}");
        }
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
        assert!(efforts("agy").contains(&"max") && efforts("agy").contains(&"low"));

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
