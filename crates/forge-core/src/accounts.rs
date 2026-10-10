// Varias cuentas de un mismo agente (Claude Code, Codex) a la vez y sin cerrar sesión:
// cada cuenta es una carpeta de configuración propia (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`)
// con su login e historial; los ajustes, instrucciones y skills se comparten con la
// cuenta principal mediante enlaces.
use crate::tr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    }
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
            std::os::unix::fs::symlink(&from, &to).map_err(|e| e.to_string())?;
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
        };
        account.dir = self.account_dir(&account);
        prepare(&account, self.home.as_deref())?;
        saved.push(account.clone());
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
}
