// Varias cuentas de un mismo agente (Claude Code, Codex) a la vez y sin cerrar sesión:
// cada cuenta es una carpeta de configuración propia (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`)
// con su login e historial; los ajustes, instrucciones y skills se comparten con la
// cuenta principal mediante enlaces.
use crate::tr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::secrets::SecretStore;
use crate::store::Store;

/// Agentes con cuentas: (programa, variable con su carpeta, carpeta principal en $HOME,
/// lo que se comparte con la principal).
const AGENTS: [(&str, &str, &str, &[&str]); 2] = [
    (
        "claude",
        "CLAUDE_CONFIG_DIR",
        ".claude",
        &[
            "settings.json",
            "CLAUDE.md",
            "skills",
            "agents",
            "commands",
            "plugins",
        ],
    ),
    (
        "codex",
        "CODEX_HOME",
        ".codex",
        &["config.toml", "AGENTS.md", "skills"],
    ),
];

/// Cuenta de un agente. La `id` 0 es la principal (la carpeta de siempre).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub program: String,
    pub name: String,
    /// Su carpeta (`None` = la principal, sin tocar el entorno). La pone el `Store`.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
    /// Cuenta con clave de API en vez de login: `anthropic`, `deepseek` o `custom`. La clave
    /// no se guarda aquí sino en el llavero del sistema (`secret_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Dirección de la API (solo `custom`; DeepSeek ya la trae).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Modelo a usar (`custom` lo exige; DeepSeek trae `deepseek-flash` por defecto).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Dirección de la API de DeepSeek compatible con Claude Code, y sus modelos.
const DEEPSEEK_URL: &str = "https://api.deepseek.com/anthropic";
const DEEPSEEK_MODEL: &str = "deepseek-flash[1m]";
const DEEPSEEK_SMALL: &str = "deepseek-flash";

/// Una ventana de uso del plan (5 h, semanal…), como la da Claude Code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub kind: String,
    pub percent_used: f64,
    #[serde(default)]
    pub resets_at: Option<String>,
}

/// Últimas ventanas de uso conocidas de una cuenta y cuándo se supieron.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountLimits {
    pub at: i64,
    pub windows: Vec<UsageWindow>,
}

pub fn supported(program: &str) -> bool {
    AGENTS.iter().any(|a| a.0 == program)
}

impl Account {
    pub fn is_main(&self) -> bool {
        self.id == 0
    }

    /// Variable de entorno que apunta el agente a esta cuenta.
    pub fn env(&self) -> Option<(String, String)> {
        let var = AGENTS.iter().find(|a| a.0 == self.program)?.1;
        Some((
            var.into(),
            self.dir.as_ref()?.to_string_lossy().into_owned(),
        ))
    }

    /// Es una cuenta con clave de API (en vez de login).
    pub fn is_api(&self) -> bool {
        self.provider.is_some()
    }

    /// Nombre bajo el que su clave está en el llavero.
    pub fn secret_id(&self) -> String {
        format!("{}-{}", self.program, self.id)
    }

    /// El asesor de Claude Code es de Anthropic: con otro proveedor no se activa.
    pub fn allows_advisor(&self) -> bool {
        matches!(self.provider.as_deref(), None | Some("anthropic"))
    }

    /// Variables que llevan la clave, la dirección y los modelos al agente (vacío si la
    /// cuenta es de login). La clave llega aquí ya leída del llavero.
    pub fn api_env(&self, secret: &str) -> Vec<(String, String)> {
        let var = |k: &str, v: &str| (k.to_string(), v.to_string());
        let models = |big: &str, small: &str| {
            vec![
                var("ANTHROPIC_MODEL", big),
                var("ANTHROPIC_DEFAULT_OPUS_MODEL", big),
                var("ANTHROPIC_DEFAULT_SONNET_MODEL", big),
                var("ANTHROPIC_DEFAULT_HAIKU_MODEL", small),
                var("CLAUDE_CODE_SUBAGENT_MODEL", small),
            ]
        };
        match self.provider.as_deref() {
            Some("anthropic") => {
                let mut env = vec![var("ANTHROPIC_API_KEY", secret)];
                if let Some(model) = &self.model {
                    env.push(var("ANTHROPIC_MODEL", model));
                }
                env
            }
            Some("deepseek") => {
                let model = self.model.as_deref().unwrap_or(DEEPSEEK_MODEL);
                let mut env = vec![
                    var("ANTHROPIC_BASE_URL", DEEPSEEK_URL),
                    var("ANTHROPIC_AUTH_TOKEN", secret),
                    // 1M de contexto: que Claude compacte antes de llenarlo.
                    var("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "786432"),
                ];
                env.extend(models(model, DEEPSEEK_SMALL));
                env
            }
            Some(_) => {
                let url = self.base_url.as_deref().unwrap_or_default();
                let model = self.model.as_deref().unwrap_or_default();
                let mut env = vec![
                    var("ANTHROPIC_BASE_URL", url),
                    var("ANTHROPIC_AUTH_TOKEN", secret),
                ];
                env.extend(models(model, model));
                env
            }
            None => Vec::new(),
        }
    }

    /// Comando para iniciar sesión en la cuenta (Claude lo pide solo al abrirse).
    pub fn login_command(&self) -> &'static str {
        match self.program.as_str() {
            "codex" => "codex login",
            _ => "claude",
        }
    }
}

fn main_account(program: &str) -> Account {
    Account {
        id: 0,
        program: program.into(),
        name: tr!("Principal").into(),
        dir: None,
        provider: None,
        base_url: None,
        model: None,
    }
}

/// Enlaza un archivo o carpeta de la cuenta principal. En Windows, un archivo solo se
/// comparte de verdad con un symlink (Modo de desarrollador): un enlace duro se rompe cuando
/// el agente reescribe el archivo y una copia no recibe los cambios, así que se avisa.
fn share(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(windows)]
    if from.is_file() {
        return std::os::windows::fs::symlink_file(from, to).map_err(|_| {
            tr!(
                "Para compartir {p0} entre cuentas, activa el Modo de desarrollador de Windows (Configuración → Sistema → Para desarrolladores).",
                p0 = from.display()
            )
        });
    }
    crate::symlink(from, to).map_err(|e| e.to_string())
}

/// Crea la carpeta de la cuenta y enlaza lo compartido con la principal (lo que ya
/// exista en la carpeta no se toca).
fn prepare(account: &Account, home: Option<&Path>) -> Result<(), String> {
    let (Some(dir), Some(agent)) = (&account.dir, AGENTS.iter().find(|a| a.0 == account.program))
    else {
        return Ok(());
    };
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let Some(home) = home else {
        return Ok(());
    };
    let main = home.join(agent.2);
    for name in agent.3 {
        let (from, to) = (main.join(name), dir.join(name));
        if from.exists() && to.symlink_metadata().is_err() {
            share(&from, &to)?;
        }
    }
    Ok(())
}

impl Store {
    fn saved_accounts(&self) -> Vec<Account> {
        let mut saved: Vec<Account> = self
            .setting("accounts")
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        for a in &mut saved {
            a.dir = self.account_dir(a);
        }
        saved
    }

    fn account_dir(&self, a: &Account) -> Option<PathBuf> {
        if a.is_main() {
            return None;
        }
        Some(
            self.accounts_root
                .as_ref()?
                .join(format!("{}-{}", a.program, a.id)),
        )
    }

    fn save_accounts(&self, accounts: &[Account]) -> Result<(), String> {
        let json = serde_json::to_string(accounts).map_err(|e| e.to_string())?;
        self.set_setting("accounts", &json)
    }

    /// Cuentas de un agente: la principal primero (aunque no se haya guardado nada).
    pub fn accounts(&self, program: &str) -> Vec<Account> {
        let saved: Vec<Account> = self
            .saved_accounts()
            .into_iter()
            .filter(|a| a.program == program)
            .collect();
        let mut out = Vec::new();
        if !saved.iter().any(Account::is_main) {
            out.push(main_account(program));
        }
        out.extend(saved);
        out.sort_by_key(|a| a.id);
        out
    }

    /// Todas las cuentas de los agentes que las admiten.
    pub fn all_accounts(&self) -> Vec<Account> {
        AGENTS.iter().flat_map(|a| self.accounts(a.0)).collect()
    }

    pub fn add_account(&self, program: &str, name: &str) -> Result<Account, String> {
        if !supported(program) {
            return Err(tr!("{program} no admite varias cuentas", program = program));
        }
        let mut saved = self.saved_accounts();
        let id = saved.iter().map(|a| a.id).max().unwrap_or(0) + 1;
        let mut account = Account {
            id,
            program: program.into(),
            name: name.trim().into(),
            dir: None,
            provider: None,
            base_url: None,
            model: None,
        };
        account.dir = self.account_dir(&account);
        prepare(&account, self.home.as_deref())?;
        saved.push(account.clone());
        self.save_accounts(&saved)?;
        Ok(account)
    }

    /// Añade una cuenta de Claude Code con clave de API. La clave va al llavero; en los
    /// ajustes solo queda el proveedor. Si algo no vale, no se guarda nada.
    pub fn add_api_account(
        &self,
        name: &str,
        provider: &str,
        base_url: Option<&str>,
        model: Option<&str>,
        secret: &str,
        keys: &dyn SecretStore,
    ) -> Result<Account, String> {
        let clean = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(String::from);
        let (base_url, model) = (clean(base_url), clean(model));
        if secret.trim().is_empty() {
            return Err(tr!("Falta la clave.").to_string());
        }
        match provider {
            "anthropic" | "deepseek" => {}
            "custom" => {
                let url = base_url.as_deref().unwrap_or_default();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(tr!("La dirección debe empezar por https://").to_string());
                }
                if model.is_none() {
                    return Err(tr!("Falta el modelo.").to_string());
                }
            }
            other => return Err(tr!("Proveedor desconocido: {p}", p = other)),
        }
        let mut account = self.add_account("claude", name)?;
        account.provider = Some(provider.into());
        account.base_url = base_url;
        account.model = model;
        // La clave primero: sin llavero la cuenta no queda a medias.
        if let Err(e) = keys.set(&account.secret_id(), secret.trim()) {
            let _ = self.delete_account("claude", account.id);
            return Err(e);
        }
        let mut saved = self.saved_accounts();
        if let Some(slot) = saved.iter_mut().find(|a| a.id == account.id) {
            *slot = account.clone();
        }
        self.save_accounts(&saved)?;
        Ok(account)
    }

    pub fn rename_account(&self, program: &str, id: i64, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(tr!("la cuenta necesita un nombre").into());
        }
        let mut saved = self.saved_accounts();
        match saved
            .iter_mut()
            .find(|a| a.program == program && a.id == id)
        {
            Some(a) => a.name = name.into(),
            // La principal se guarda la primera vez que se le cambia el nombre.
            None if id == 0 => saved.push(Account {
                name: name.into(),
                ..main_account(program)
            }),
            None => return Err(tr!("No existe esa cuenta.").into()),
        }
        self.save_accounts(&saved)
    }

    /// Borra la cuenta y su carpeta (su login e historial). La principal no se borra.
    pub fn delete_account(&self, program: &str, id: i64) -> Result<(), String> {
        if id == 0 {
            return Err(tr!("La cuenta principal no se puede borrar.").into());
        }
        let mut saved = self.saved_accounts();
        let Some(i) = saved
            .iter()
            .position(|a| a.program == program && a.id == id)
        else {
            return Ok(());
        };
        let account = saved.remove(i);
        self.save_accounts(&saved)?;
        if let Some(dir) = account.dir {
            // Los enlaces se borran como enlaces: lo compartido con la principal queda intacto.
            let _ = std::fs::remove_dir_all(dir);
        }
        Ok(())
    }

    pub fn set_account_limits(
        &self,
        program: &str,
        id: i64,
        windows: &[UsageWindow],
    ) -> Result<(), String> {
        let limits = AccountLimits {
            at: crate::store::now(),
            windows: windows.to_vec(),
        };
        let json = serde_json::to_string(&limits).map_err(|e| e.to_string())?;
        self.set_setting(&format!("limits:{program}:{id}"), &json)
    }

    pub fn account_limits(&self, program: &str, id: i64) -> Option<AccountLimits> {
        let json = self.setting(&format!("limits:{program}:{id}")).ok()??;
        serde_json::from_str(&json).ok()
    }

    /// Cuenta que usa el proyecto para ese agente (la última elegida; si no, la principal).
    pub fn project_account(&self, project: &Path, program: &str) -> Account {
        let key = format!("account:{program}:{}", self.project_key(project));
        let id = self
            .setting(&key)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let accounts = self.accounts(program);
        accounts
            .iter()
            .find(|a| a.id == id)
            .unwrap_or(&accounts[0])
            .clone()
    }

    pub fn set_project_account(
        &self,
        project: &Path,
        program: &str,
        id: i64,
    ) -> Result<(), String> {
        let key = format!("account:{program}:{}", self.project_key(project));
        self.set_setting(&key, &id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_have_their_own_folder_and_share_settings() {
        let tmp = std::env::temp_dir().join(format!("forge-accounts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("home/.claude/skills")).unwrap();
        std::fs::write(tmp.join("home/.claude/settings.json"), "{}").unwrap();
        let mut store = Store::in_memory().unwrap();
        store.home = Some(tmp.join("home"));
        store.accounts_root = Some(tmp.join("forge/accounts"));
        let p = Path::new("/p/x");

        assert_eq!(
            store.accounts("claude").len(),
            1,
            "siempre está la principal"
        );
        assert_eq!(store.project_account(p, "claude").id, 0);
        assert_eq!(store.accounts("claude")[0].env(), None);

        let work = store.add_account("claude", "Trabajo").unwrap();
        let dir = work.dir.clone().unwrap();
        assert_eq!(dir, tmp.join("forge/accounts/claude-1"));
        assert!(
            dir.join("settings.json")
                .symlink_metadata()
                .unwrap()
                .is_symlink()
        );
        assert!(dir.join("skills").is_dir());
        assert!(
            dir.join("CLAUDE.md").symlink_metadata().is_err(),
            "solo lo que existe"
        );
        assert_eq!(work.env().unwrap().0, "CLAUDE_CONFIG_DIR");

        store.set_project_account(p, "claude", work.id).unwrap();
        assert_eq!(store.project_account(p, "claude").name, "Trabajo");
        assert_eq!(
            store.project_account(p, "codex").id,
            0,
            "cada agente la suya"
        );

        store.rename_account("claude", 0, "Personal").unwrap();
        store.rename_account("claude", work.id, "Empresa").unwrap();
        let names: Vec<String> = store
            .accounts("claude")
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["Personal", "Empresa"]);
        assert!(store.rename_account("claude", work.id, " ").is_err());
        assert!(store.add_account("gemini", "x").is_err());

        // Uso del plan por cuenta (lo manda el mod) y tokens por cuenta.
        let windows = vec![UsageWindow {
            kind: "five_hour".into(),
            percent_used: 40.0,
            resets_at: None,
        }];
        store
            .set_account_limits("claude", work.id, &windows)
            .unwrap();
        assert_eq!(
            store.account_limits("claude", work.id).unwrap().windows,
            windows
        );
        assert!(store.account_limits("claude", 0).is_none());
        let parsed: Vec<UsageWindow> =
            serde_json::from_str(r#"[{"kind":"seven_day","percentUsed":3}]"#).unwrap();
        assert_eq!(parsed[0].percent_used, 3.0);
        let turn = |model: &str| crate::events::TurnUsage {
            model: model.into(),
            input_tokens: 100,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            subagent: false,
        };
        store.add_agent_usage(p, &turn("opus"), work.id).unwrap();
        store.add_agent_usage(p, &turn("opus"), work.id).unwrap();
        store.add_agent_usage(p, &turn("haiku"), 0).unwrap();
        assert_eq!(
            store.usage_by_account(0).unwrap(),
            [(work.id, 200), (0, 100)]
        );

        assert!(store.delete_account("claude", 0).is_err());
        store.delete_account("claude", work.id).unwrap();
        assert!(!dir.exists());
        assert!(
            tmp.join("home/.claude/settings.json").exists(),
            "lo compartido sigue"
        );
        assert_eq!(
            store.project_account(p, "claude").name,
            "Personal",
            "vuelve a la principal"
        );
        let _ = std::fs::remove_dir_all(tmp);
    }
    fn api(provider: &str, base: Option<&str>, model: Option<&str>) -> Account {
        Account {
            id: 3,
            program: "claude".into(),
            name: "API".into(),
            dir: None,
            provider: Some(provider.into()),
            base_url: base.map(Into::into),
            model: model.map(Into::into),
        }
    }

    fn env_of(account: &Account) -> std::collections::HashMap<String, String> {
        account.api_env("sk-clave").into_iter().collect()
    }

    /// Cada proveedor lleva su dirección, su clave y sus modelos al Claude Code que se abre.
    #[test]
    fn api_accounts_set_the_environment_of_their_provider() {
        let a = env_of(&api("anthropic", None, None));
        assert_eq!(a["ANTHROPIC_API_KEY"], "sk-clave");
        assert!(!a.contains_key("ANTHROPIC_BASE_URL") && !a.contains_key("ANTHROPIC_MODEL"));

        let d = env_of(&api("deepseek", None, None));
        assert_eq!(
            d["ANTHROPIC_BASE_URL"],
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(d["ANTHROPIC_AUTH_TOKEN"], "sk-clave");
        assert_eq!(d["ANTHROPIC_MODEL"], "deepseek-flash[1m]");
        assert_eq!(d["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "deepseek-flash");
        assert!(!d.contains_key("ANTHROPIC_API_KEY"));
        let pro = env_of(&api("deepseek", None, Some("deepseek-v4-pro")));
        assert_eq!(pro["ANTHROPIC_MODEL"], "deepseek-v4-pro");
        assert_eq!(pro["ANTHROPIC_DEFAULT_OPUS_MODEL"], "deepseek-v4-pro");

        let c = env_of(&api(
            "custom",
            Some("https://proxy.example/api"),
            Some("mi-modelo"),
        ));
        assert_eq!(c["ANTHROPIC_BASE_URL"], "https://proxy.example/api");
        assert_eq!(c["ANTHROPIC_AUTH_TOKEN"], "sk-clave");
        assert_eq!(c["ANTHROPIC_MODEL"], "mi-modelo");
        assert_eq!(c["ANTHROPIC_DEFAULT_SONNET_MODEL"], "mi-modelo");

        // Una cuenta normal (con login) no pone nada.
        let mut normal = api("anthropic", None, None);
        normal.provider = None;
        assert!(normal.api_env("x").is_empty());
    }

    /// El asesor de Claude es de Anthropic: con otro proveedor no se activa.
    #[test]
    fn the_advisor_only_applies_to_anthropic_accounts() {
        assert!(api("anthropic", None, None).allows_advisor());
        assert!(!api("deepseek", None, None).allows_advisor());
        assert!(!api("custom", Some("https://x.example"), Some("m")).allows_advisor());
        let mut normal = api("anthropic", None, None);
        normal.provider = None;
        assert!(normal.allows_advisor());
    }

    /// Las cuentas guardadas antes de esto siguen leyéndose igual.
    #[test]
    fn old_saved_accounts_have_no_provider() {
        let old: Account =
            serde_json::from_str(r#"{"id":1,"program":"claude","name":"Trabajo"}"#).unwrap();
        assert_eq!((old.provider, old.base_url, old.model), (None, None, None));
    }

    /// La clave va al llavero; lo que se guarda en los ajustes no la lleva.
    #[test]
    fn adding_an_api_account_keeps_the_key_out_of_the_saved_accounts() {
        use crate::secrets::{MemoryStore, SecretStore};
        let tmp = std::env::temp_dir().join(format!("forge-apiacc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("home/.claude")).unwrap();
        let mut store = Store::in_memory().unwrap();
        store.home = Some(tmp.join("home"));
        store.accounts_root = Some(tmp.join("forge/accounts"));
        let keys = MemoryStore::default();

        let added = store
            .add_api_account("Mi API", "anthropic", None, None, "sk-ant-secreto", &keys)
            .unwrap();
        assert_eq!(
            keys.get(&added.secret_id()).unwrap().as_deref(),
            Some("sk-ant-secreto")
        );
        let saved = store.setting("accounts").unwrap().unwrap();
        assert!(!saved.contains("sk-ant-secreto"), "{saved}");
        let listed = store.accounts("claude");
        assert_eq!(listed[1].provider.as_deref(), Some("anthropic"));
        // Tiene su carpeta propia: su historial no se mezcla con el de la principal.
        assert!(listed[1].dir.as_ref().unwrap().is_dir());

        // Lo que falta o no vale se dice antes de guardar nada.
        assert!(
            store
                .add_api_account("X", "anthropic", None, None, "  ", &keys)
                .is_err()
        );
        assert!(
            store
                .add_api_account("X", "custom", None, Some("m"), "k", &keys)
                .is_err()
        );
        assert!(
            store
                .add_api_account("X", "custom", Some("proxy.example"), Some("m"), "k", &keys)
                .is_err()
        );
        assert!(
            store
                .add_api_account("X", "custom", Some("https://p.example"), None, "k", &keys)
                .is_err()
        );
        assert!(
            store
                .add_api_account("X", "otro", None, None, "k", &keys)
                .is_err()
        );
        assert_eq!(store.accounts("claude").len(), 2, "nada se guardó a medias");
        let _ = std::fs::remove_dir_all(tmp);
    }
}
