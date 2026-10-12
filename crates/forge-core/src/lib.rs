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
        let mut cmd = std::process::Command::new("powershell.exe");
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

/// Enlace simbólico a un archivo o carpeta (en Windows, el tipo según lo que sea `from`).
pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if from.is_dir() {
            std::os::windows::fs::symlink_dir(from, to)
        } else {
            std::os::windows::fs::symlink_file(from, to)
        }
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(from, to)
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
