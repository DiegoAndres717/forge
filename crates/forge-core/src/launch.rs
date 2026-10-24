// Con qué modelo y esfuerzo se abren los agentes desde Forge: un valor general por
// agente y, si se quiere, otro para un proyecto. Vacío = lo que tenga el agente en su
// propia configuración (Forge no lo toca).
use std::path::Path;

use crate::accounts::Account;
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

/// La cuenta forma parte de la clave para no confundir el modelo de inicio con el que una
/// cuenta API entrega al agente en su entorno.
fn account_key(account: &Account, what: &str, project: Option<&str>) -> String {
    match project {
        Some(p) => format!("launch:{}:{}:{what}:{p}", account.program, account.id),
        None => format!("launch:{}:{}:{what}", account.program, account.id),
    }
}

impl Store {
    /// El valor guardado (general si `project` es `None`); vacío si no hay.
    pub fn launch_pref(&self, program: &str, what: &str, project: Option<&Path>) -> String {
        let project = project.map(|p| self.project_key(p));
        self.launch_pref_keyed(program, what, project.as_deref())
    }

    /// Lo mismo con la clave del proyecto ya calculada (`project_key` no se aplica dos veces).
    fn launch_pref_keyed(&self, program: &str, what: &str, project: Option<&str>) -> String {
        let saved = self.setting(&key(program, what, project)).ok().flatten();
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

    /// Preferencia de inicio de una cuenta. Sin valor propio sigue lo que Forge guardaba antes
    /// sin cuenta (lo que la pantalla de Ajustes escribe): actualizar no cambia las terminales
    /// de ninguna cuenta.
    pub fn account_launch_pref(
        &self,
        account: &Account,
        what: &str,
        project: Option<&Path>,
    ) -> String {
        let project = project.map(|p| self.project_key(p));
        match self
            .setting(&account_key(account, what, project.as_deref()))
            .ok()
            .flatten()
        {
            Some(value) => value,
            None => self.launch_pref_keyed(&account.program, what, project.as_deref()),
        }
    }

    pub fn set_account_launch_pref(
        &self,
        account: &Account,
        what: &str,
        project: Option<&Path>,
        value: &str,
    ) -> Result<(), String> {
        let project = project.map(|p| self.project_key(p));
        self.set_setting(
            &account_key(account, what, project.as_deref()),
            value.trim(),
        )
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

    /// Claude en Sonnet con Opus de asesor (plan #81). Apagado de serie.
    pub fn advisor(&self) -> bool {
        self.launch_pref("forge", "advisor", None) == "on"
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

    /// Modelo y esfuerzo que recibe esta cuenta al abrir una terminal. El proyecto puede
    /// especializarlos y, si no, toma los de la cuenta.
    pub fn account_launch_for(&self, account: &Account, project: &Path) -> (String, String) {
        let pick = |what: &str| {
            let own = self.account_launch_pref(account, what, Some(project));
            if own.is_empty() {
                self.account_launch_pref(account, what, None)
            } else {
                own
            }
        };
        // Una cuenta que ya fija su modelo en el entorno no recibe `--model`: lo pisaría.
        let model = if account.fixes_model() {
            String::new()
        } else {
            pick("model")
        };
        (model, pick("effort"))
    }
}

/// Con el asesor encendido, Claude Code abre en Sonnet: `ANTHROPIC_MODEL` va por encima del
/// `model` de los ajustes. Un modelo ya fijado en el entorno del proyecto manda, y un `/model`
/// o `--model` en la propia sesión también. `FORGE_ADVISOR=1` avisa al mod de Forge para que
/// le pida a Sonnet las consultas a Opus (él decide poco por su cuenta).
pub fn advisor_env(on: bool, program: &str, env: &mut std::collections::HashMap<String, String>) {
    if on && program == "claude" {
        env.entry("ANTHROPIC_MODEL".into())
            .or_insert_with(|| "sonnet".into());
        env.insert("FORGE_ADVISOR".into(), "1".into());
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
    fn the_advisor_puts_claude_on_sonnet_unless_a_model_is_already_set() {
        let mut env = std::collections::HashMap::new();
        advisor_env(false, "claude", &mut env);
        advisor_env(true, "codex", &mut env);
        assert!(env.is_empty(), "apagado, o en otro agente: nada");
        advisor_env(true, "claude", &mut env);
        assert_eq!(env["ANTHROPIC_MODEL"], "sonnet");
        // El mod de Claude sabe así que debe pedir las consultas al asesor.
        assert_eq!(env["FORGE_ADVISOR"], "1");
        env.insert("ANTHROPIC_MODEL".into(), "haiku".into());
        advisor_env(true, "claude", &mut env);
        assert_eq!(env["ANTHROPIC_MODEL"], "haiku", "el modelo fijado manda");
    }

    #[test]
    fn the_advisor_is_off_by_default() {
        let store = Store::in_memory().unwrap();
        assert!(!store.advisor());
        store
            .set_launch_pref("forge", "advisor", None, "on")
            .unwrap();
        assert!(store.advisor());
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

    #[test]
    fn every_account_has_its_own_launch_preferences() {
        let store = Store::in_memory().unwrap();
        let (a, b) = (Path::new("/p/a"), Path::new("/p/b"));
        let main = Account {
            id: 0,
            program: "claude".into(),
            name: "Main".into(),
            dir: None,
            provider: None,
            base_url: None,
            model: None,
        };
        let work = Account {
            id: 1,
            name: "Work".into(),
            ..main.clone()
        };

        // Lo existente sigue siendo la configuración de la principal tras actualizar.
        store.set_setting("claude_model", "opus").unwrap();
        store
            .set_launch_pref("claude", "effort", None, "medium")
            .unwrap();
        store
            .set_launch_pref("claude", "effort", Some(a), "max")
            .unwrap();
        assert_eq!(
            store.account_launch_for(&main, a),
            ("opus".into(), "max".into())
        );

        // Una cuenta sin valor propio sigue lo general (lo que la pantalla de Ajustes escribe
        // hasta que cada cuenta se configure sola): actualizar no cambia sus terminales.
        assert_eq!(
            store.account_launch_for(&work, a),
            ("opus".into(), "max".into())
        );
        store
            .set_account_launch_pref(&work, "model", None, "sonnet")
            .unwrap();
        store
            .set_account_launch_pref(&work, "effort", Some(a), "high")
            .unwrap();
        assert_eq!(
            store.account_launch_for(&work, a),
            ("sonnet".into(), "high".into())
        );
        assert_eq!(
            store.account_launch_for(&work, b),
            ("sonnet".into(), "medium".into())
        );
    }

    #[test]
    fn an_account_that_fixes_its_own_model_gets_no_launch_model() {
        let store = Store::in_memory().unwrap();
        store.set_setting("claude_model", "opus").unwrap();
        store
            .set_launch_pref("claude", "effort", None, "high")
            .unwrap();
        let api = |provider: &str, model: Option<&str>| Account {
            id: 1,
            program: "claude".into(),
            name: "Api".into(),
            dir: None,
            provider: Some(provider.into()),
            base_url: None,
            model: model.map(Into::into),
        };
        let a = Path::new("/p/a");
        // Los modelos de DeepSeek y de una dirección propia van en el entorno de la cuenta:
        // `--model opus` los pisaría. El esfuerzo no choca con nada.
        for fixed in [
            api("deepseek", None),
            api("custom", Some("m")),
            api("anthropic", Some("claude-haiku-5-5")),
        ] {
            assert_eq!(
                store.account_launch_for(&fixed, a),
                (String::new(), "high".into()),
                "{fixed:?}"
            );
        }
        // Anthropic sin modelo propio sí recibe el que se eligió.
        assert_eq!(
            store.account_launch_for(&api("anthropic", None), a),
            ("opus".into(), "high".into())
        );
    }
}
