// Claves de API de las cuentas: van al llavero del sistema, nunca a un archivo ni a la
// base de datos de Forge.
use std::collections::HashMap;
use std::sync::Mutex;

use crate::tr;

/// Dónde se guardan las claves, una por cuenta. El llavero del sistema en la app; en
/// memoria en las pruebas.
pub trait SecretStore: Send + Sync {
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    /// `None` si la cuenta no tiene clave guardada.
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    /// Borrar una clave que no está no es un error.
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// Nombre bajo el que Forge guarda sus claves en el llavero.
const SERVICE: &str = "forge-api-key";

/// El llavero del sistema: Keychain en macOS, Administrador de credenciales en Windows y
/// Secret Service en Linux. Si no hay ninguno, falla con un aviso: nunca guarda en claro.
pub struct Keychain;

fn entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| keychain_error(&e))
}

/// El error del llavero, en palabras de la persona.
fn keychain_error(e: &keyring::Error) -> String {
    match e {
        keyring::Error::NoDefaultStore | keyring::Error::NoStorageAccess(_) => {
            tr!("No hay un llavero del sistema disponible, así que la clave no se guarda.")
                .to_string()
        }
        other => tr!("No se pudo usar el llavero del sistema: {e}", e = other),
    }
}

impl SecretStore for Keychain {
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        entry(account)?
            .set_password(secret)
            .map_err(|e| keychain_error(&e))
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        match entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_error(&e)),
        }
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_error(&e)),
        }
    }
}

/// Un llavero en memoria, para las pruebas.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(account.into(), secret.into());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_can_be_saved_read_and_deleted() {
        let store = MemoryStore::default();
        assert_eq!(store.get("cuenta-1").unwrap(), None);
        store.set("cuenta-1", "sk-ant-uno").unwrap();
        store.set("cuenta-2", "sk-dos").unwrap();
        assert_eq!(
            store.get("cuenta-1").unwrap().as_deref(),
            Some("sk-ant-uno")
        );
        // Guardar de nuevo reemplaza la clave.
        store.set("cuenta-1", "sk-ant-nueva").unwrap();
        assert_eq!(
            store.get("cuenta-1").unwrap().as_deref(),
            Some("sk-ant-nueva")
        );
        store.delete("cuenta-1").unwrap();
        assert_eq!(store.get("cuenta-1").unwrap(), None);
        assert_eq!(store.get("cuenta-2").unwrap().as_deref(), Some("sk-dos"));
        // Borrar lo que no está no es un error.
        store.delete("cuenta-1").unwrap();
    }

    #[test]
    fn without_a_system_keychain_the_error_says_the_key_is_not_saved() {
        let msg = keychain_error(&keyring::Error::NoDefaultStore);
        assert!(msg.contains("llavero"), "{msg}");
        assert!(msg.contains("no se guarda"), "{msg}");
    }

    /// El llavero de verdad (en el CI de Linux no hay Secret Service): a mano con
    /// `cargo test -p forge-core -- --ignored real_keychain`.
    #[test]
    #[ignore]
    fn real_keychain_round_trip() {
        let store = Keychain;
        let account = format!("test-{}", std::process::id());
        store.set(&account, "sk-prueba").unwrap();
        assert_eq!(store.get(&account).unwrap().as_deref(), Some("sk-prueba"));
        store.delete(&account).unwrap();
        assert_eq!(store.get(&account).unwrap(), None);
    }
}
