// Hooks de Git (pre-commit y pre-push) que llaman a `forge guard`, para que las reglas
// se apliquen aunque el commit se haga desde VS Code, Warp, otro agente o un script.
use std::path::{Path, PathBuf};
use std::process::Command;

/// Primera línea de comentario que identifica un hook de Forge.
const MARKER: &str = "# forge-managed-hook";
const HOOKS: [(&str, &str); 2] = [("pre-commit", "commit"), ("pre-push", "push")];

#[derive(Debug, PartialEq)]
pub enum HookState {
    Installed,
    Missing,
    /// Hay otro hook (Forge lo encadenará al instalar).
    Foreign,
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Carpeta de hooks del repositorio. Si `core.hooksPath` está configurado (husky, lefthook…),
/// no se toca: esos gestores regeneran sus hooks y borrarían los de Forge.
fn hooks_dir(project: &Path) -> Result<PathBuf, String> {
    let repo = PathBuf::from(
        git(project, &["rev-parse", "--show-toplevel"])
            .map_err(|_| "no es un repositorio Git".to_string())?,
    );
    if let Ok(path) = git(&repo, &["config", "core.hooksPath"])
        && !path.is_empty()
    {
        return Err(format!(
            "core.hooksPath = {path} (¿husky?). Añade `forge guard commit` a tu pre-commit y `forge guard push` a tu pre-push."
        ));
    }
    let dir = PathBuf::from(git(&repo, &["rev-parse", "--git-path", "hooks"])?);
    Ok(if dir.is_absolute() {
        dir
    } else {
        repo.join(dir)
    })
}

fn is_managed(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .is_ok_and(|s| s.lines().nth(1).is_some_and(|l| l.starts_with(MARKER)))
}

fn backup(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.forge-backup",
        path.file_name().unwrap_or_default().to_string_lossy()
    ))
}

/// Comillas simples para sh.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn script(name: &str, stage: &str, forge: &Path, project: &Path) -> String {
    let backup = format!("\"$HOOK_DIR/{name}.forge-backup\"");
    // pre-push recibe por stdin las referencias a subir: se guardan para el hook anterior y para Forge.
    let (save, feed) = if name == "pre-push" {
        ("INPUT=\"$(cat)\"\n", "printf '%s\\n' \"$INPUT\" | ")
    } else {
        ("", "")
    };
    format!(
        "#!/bin/sh\n\
         {MARKER}: generado por Forge. `forge hooks uninstall` lo quita y restaura el hook anterior.\n\
         HOOK_DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
         {save}\
         if [ -x {backup} ]; then {feed}{backup} \"$@\" || exit $?; fi\n\
         FORGE={forge}\n\
         [ -x \"$FORGE\" ] || FORGE=\"$(command -v forge)\"\n\
         if [ -z \"$FORGE\" ]; then\n  echo \"Forge Guard: no se encontró el binario de forge. Reinstala los hooks desde Forge.\" >&2\n  exit 1\nfi\n\
         {feed}exec \"$FORGE\" guard {stage} --hook --project {project} \"$@\"\n",
        forge = quote(&forge.to_string_lossy()),
        project = quote(&project.to_string_lossy()),
    )
}

/// Estado de cada hook (o por qué no se pueden gestionar).
pub type Status = Result<Vec<(&'static str, HookState)>, String>;

pub fn status(project: &Path) -> Status {
    let dir = hooks_dir(project)?;
    Ok(HOOKS
        .iter()
        .map(|(name, _)| {
            let path = dir.join(name);
            let state = if is_managed(&path) {
                HookState::Installed
            } else if path.exists() {
                HookState::Foreign
            } else {
                HookState::Missing
            };
            (*name, state)
        })
        .collect())
}

/// Instala los hooks. Un hook previo que no sea de Forge se conserva como
/// `<nombre>.forge-backup` y se ejecuta antes que Forge.
pub fn install(project: &Path, forge: &Path) -> Result<Vec<String>, String> {
    let dir = hooks_dir(project)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut report = Vec::new();
    for (name, stage) in HOOKS {
        let path = dir.join(name);
        if path.exists() && !is_managed(&path) {
            let bak = backup(&path);
            if bak.exists() {
                return Err(format!(
                    "ya existe {}: revísalo antes de instalar",
                    bak.display()
                ));
            }
            std::fs::rename(&path, &bak).map_err(|e| format!("{}: {e}", path.display()))?;
            report.push(format!(
                "{name}: el hook anterior se conserva y se ejecuta antes ({})",
                bak.display()
            ));
        }
        std::fs::write(&path, script(name, stage, forge, project))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        set_executable(&path)?;
        report.push(format!("{name}: instalado ({})", path.display()));
    }
    Ok(report)
}

/// Quita solo los hooks de Forge y restaura los anteriores.
pub fn uninstall(project: &Path) -> Result<Vec<String>, String> {
    let dir = hooks_dir(project)?;
    let mut report = Vec::new();
    for (name, _) in HOOKS {
        let path = dir.join(name);
        if !is_managed(&path) {
            if path.exists() {
                report.push(format!("{name}: no es de Forge, no se toca"));
            }
            continue;
        }
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let bak = backup(&path);
        if bak.exists() {
            std::fs::rename(&bak, &path).map_err(|e| format!("{}: {e}", bak.display()))?;
            report.push(format!("{name}: quitado; restaurado el hook anterior"));
        } else {
            report.push(format!("{name}: quitado"));
        }
    }
    Ok(report)
}

fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Rango a evaluar a partir del stdin de pre-push:
/// "<ref local> <sha local> <ref remota> <sha remota>" por línea.
pub fn push_range(stdin: &str) -> Option<String> {
    const ZERO: &str = "0000000000000000000000000000000000000000";
    stdin.lines().find_map(|line| {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [_, local, _, remote] = parts[..] else {
            return None;
        };
        if local.chars().all(|c| c == '0') {
            return None; // borrado de rama remota: no hay nada que evaluar
        }
        // Rama nueva en el remoto: se evalúa contra la rama principal (None = la detecta el guard).
        if remote == ZERO || remote.chars().all(|c| c == '0') {
            return None;
        }
        Some(format!("{remote}..{local}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pre_push_stdin() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let z = "0".repeat(40);
        assert_eq!(
            push_range(&format!("refs/heads/x {a} refs/heads/x {b}\n")),
            Some(format!("{b}..{a}"))
        );
        assert_eq!(
            push_range(&format!("refs/heads/x {a} refs/heads/x {z}\n")),
            None
        );
        assert_eq!(
            push_range(&format!("(delete) {z} refs/heads/x {b}\n")),
            None
        );
    }

    #[test]
    fn quotes_paths_for_sh() {
        assert_eq!(quote("/a b/it's"), r"'/a b/it'\''s'");
        let s = script("pre-push", "push", Path::new("/bin/forge"), Path::new("/p"));
        assert!(s.lines().nth(1).unwrap().starts_with(MARKER));
        assert!(s.contains("INPUT=\"$(cat)\"") && s.contains("guard push --hook --project '/p'"));
    }
}
