// Lógica de Forge sin interfaz: Guard, hooks, evidencias, agentes, router de modelos,
// memoria, servidor MCP y persistencia. La app (ventana y CLI) se construye encima.
pub mod accounts;
pub mod agents;
pub mod candidate;
pub mod danger;
pub mod events;
pub mod evidence;
pub mod git;
pub mod guard;
pub mod hooks;
pub mod i18n;
mod i18n_en;
pub mod ideas;
pub mod launch;
pub mod mcp;
pub mod memory;
pub mod pr;
pub mod project;
pub mod reviewers;
pub mod router;
pub mod sessions;
pub mod store;
pub mod subagents;
pub mod updates;

/// Un comando del usuario en su shell: `$SHELL -l -c` en macOS y Linux (carga su PATH), y
/// PowerShell en Windows.
pub fn shell_command(command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new(powershell());
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let fallback = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/sh"
        };
        let shell = std::env::var("SHELL").unwrap_or_else(|_| fallback.into());
        let mut cmd = std::process::Command::new(shell);
        cmd.args(["-l", "-c", command]);
        cmd
    }
}

/// PowerShell para los comandos en Windows: PowerShell 7 (`pwsh`, entiende `&&`) si está en
/// el PATH o en su carpeta de instalación; si no, el Windows PowerShell 5.1 de Windows.
fn pick_powershell(path: &std::ffi::OsStr, installed: &[std::path::PathBuf]) -> std::path::PathBuf {
    std::env::split_paths(path)
        .map(|dir| dir.join("pwsh.exe"))
        .chain(installed.iter().cloned())
        .find(|p| p.is_file())
        .unwrap_or_else(|| "powershell.exe".into())
}

/// El PowerShell de esta máquina (ver `pick_powershell`), buscado una vez.
pub fn powershell() -> &'static std::path::Path {
    static FOUND: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    FOUND.get_or_init(|| {
        let program_files = std::env::var_os("ProgramFiles").map(std::path::PathBuf::from);
        let installed: Vec<_> = program_files
            .map(|p| p.join("PowerShell").join("7").join("pwsh.exe"))
            .into_iter()
            .collect();
        pick_powershell(&std::env::var_os("PATH").unwrap_or_default(), &installed)
    })
}

/// Enlace simbólico a un archivo o carpeta. En Windows los symlinks piden Modo de
/// desarrollador o administrador (error 1314); sin ellos, una carpeta se enlaza con un
/// junction (no pide permisos) y un archivo se copia.
pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if from.is_dir() {
            std::os::windows::fs::symlink_dir(from, to).or_else(|_| junction(from, to))
        } else {
            std::os::windows::fs::symlink_file(from, to)
                .or_else(|_| std::fs::copy(from, to).map(|_| ()))
        }
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(from, to)
    }
}

/// Junction de Windows con `mklink /J` (orden interna de cmd).
#[cfg(windows)]
fn junction(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(to)
        .arg(from)
        .stdout(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "mklink /J {} {}",
            to.display(),
            from.display()
        )))
    }
}

/// Quita secuencias de escape ANSI (CSI y OSC) de la salida de un comando.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ ... byte final en @..~
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL o ESC \
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod platform_tests {
    use std::path::PathBuf;

    #[test]
    fn powershell_7_wins_when_it_is_installed() {
        let dir = std::env::temp_dir().join(format!("forge-pwsh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let none = std::ffi::OsStr::new("");
        assert_eq!(
            super::pick_powershell(none, &[]),
            PathBuf::from("powershell.exe")
        );
        // En la carpeta de instalación, aunque no esté en el PATH.
        let installed = dir.join("pwsh.exe");
        std::fs::write(&installed, "").unwrap();
        assert_eq!(
            super::pick_powershell(none, std::slice::from_ref(&installed)),
            installed
        );
        // En el PATH.
        let path = std::env::join_paths([dir.clone()]).unwrap();
        assert_eq!(super::pick_powershell(&path, &[]), dir.join("pwsh.exe"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Sin Modo de desarrollador, las carpetas se enlazan con un junction.
    #[cfg(windows)]
    #[test]
    fn junctions_link_folders_without_admin() {
        let base = std::env::temp_dir().join(format!("forge-junction-{}", std::process::id()));
        let from = base.join("real");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("a.txt"), "hola").unwrap();
        super::junction(&from, &base.join("link")).unwrap();
        assert_eq!(
            std::fs::read_to_string(base.join("link").join("a.txt")).unwrap(),
            "hola"
        );
        std::fs::remove_dir_all(base).unwrap();
    }
}
