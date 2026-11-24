// Conectar el MCP de Forge a apps sin terminal (la app de escritorio de Gemini y similares):
// esas apps arrancan el servidor sin el PATH del usuario y desde otra carpeta, así que
// necesitan la ruta completa del binario y `--project` con la ruta del proyecto.
use crate::agents::McpServer;
use std::path::Path;

/// Cómo se separan los argumentos de una línea de comandos.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shell {
    /// `/bin/sh` (macOS y Linux): lo que usa la app de Gemini para el comando local.
    Posix,
    /// Reglas de `CommandLineToArgvW` (cmd y la mayoría de programas de Windows).
    Windows,
}

impl Shell {
    pub fn native() -> Self {
        if cfg!(windows) {
            Shell::Windows
        } else {
            Shell::Posix
        }
    }
}

/// El servidor `forge mcp` de un proyecto, con la ruta real del binario.
pub fn server(exe: &Path, project: &Path) -> McpServer {
    McpServer {
        name: server_id(project),
        command: exe.to_string_lossy().into_owned(),
        args: vec![
            "mcp".into(),
            "--project".into(),
            project.to_string_lossy().into_owned(),
        ],
    }
}

/// La ruta del binario que conviene guardar: la real, salvo en Homebrew, donde la real
/// (`…/Cellar/forge/<versión>/bin/forge`) deja de existir con `brew upgrade`; entonces el
/// enlace estable `<prefijo>/bin/forge`, si apunta al mismo binario.
pub fn stable_exe(real: &Path) -> std::path::PathBuf {
    let prefix = real
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "Cellar"))
        .and_then(Path::parent);
    let link = prefix
        .zip(real.file_name())
        .map(|(p, n)| p.join("bin").join(n));
    match link {
        Some(link) if crate::canonical(&link) == real => link,
        _ => real.to_path_buf(),
    }
}

/// Id del servidor en la app (`forge-<proyecto>`): uno por proyecto, sin chocar entre ellos.
pub fn server_id(project: &Path) -> String {
    let name = project
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // Solo a-z, 0-9 y guiones sueltos: lo que aceptan los ids de servidor sin sorpresas.
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "forge".into()
    } else {
        format!("forge-{slug}")
    }
}

/// Un argumento con las comillas que necesite en ese shell.
pub fn quote(arg: &str, shell: Shell) -> String {
    match shell {
        Shell::Posix => {
            let safe = |c: char| c.is_ascii_alphanumeric() || "_-+=:,./@%".contains(c);
            if !arg.is_empty() && arg.chars().all(safe) {
                arg.to_string()
            } else {
                format!("'{}'", arg.replace('\'', r"'\''"))
            }
        }
        Shell::Windows => {
            if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
                return arg.to_string();
            }
            // Las barras antes de una comilla (o del cierre) se doblan; las demás, tal cual.
            let mut out = String::from('"');
            let mut slashes = 0;
            for c in arg.chars() {
                match c {
                    '\\' => {
                        slashes += 1;
                        continue;
                    }
                    '"' => out.push_str(&"\\".repeat(slashes * 2 + 1)),
                    _ => out.push_str(&"\\".repeat(slashes)),
                }
                slashes = 0;
                out.push(c);
            }
            out.push_str(&"\\".repeat(slashes * 2));
            out.push('"');
            out
        }
    }
}

/// La línea completa para pegar en el campo «Command» de la app.
pub fn command_line(server: &McpServer, shell: Shell) -> String {
    std::iter::once(&server.command)
        .chain(&server.args)
        .map(|a| quote(a, shell))
        .collect::<Vec<_>>()
        .join(" ")
}

/// JSON para «Import JSON Config»: `{"mcpServers": {"<id>": {"command", "args"}}}`, el formato
/// estándar de los clientes MCP (el que lee la app de Gemini de macOS).
pub fn config_json(server: &McpServer) -> String {
    let mut servers = serde_json::Map::new();
    servers.insert(
        server.name.clone(),
        serde_json::json!({ "command": server.command, "args": server.args }),
    );
    let json = serde_json::json!({ "mcpServers": servers });
    let mut text = serde_json::to_string_pretty(&json).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample() -> McpServer {
        server(
            Path::new("/Volumes/Mac Ext/Forge.app/Contents/MacOS/forge"),
            Path::new("/Users/ana/mis proyectos/his-erp"),
        )
    }

    #[test]
    fn server_runs_mcp_with_the_project_path() {
        let s = sample();
        assert_eq!(s.command, "/Volumes/Mac Ext/Forge.app/Contents/MacOS/forge");
        assert_eq!(
            s.args,
            ["mcp", "--project", "/Users/ana/mis proyectos/his-erp"]
        );
        assert_eq!(s.name, "forge-his-erp");
    }

    #[cfg(unix)]
    #[test]
    fn homebrew_keeps_the_stable_link_instead_of_the_versioned_cellar_path() {
        let base = std::env::temp_dir().join(format!("forge-brew-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let cellar = base.join("Cellar/forge/0.17.6/bin");
        std::fs::create_dir_all(&cellar).unwrap();
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::write(cellar.join("forge"), "").unwrap();
        std::os::unix::fs::symlink(cellar.join("forge"), base.join("bin/forge")).unwrap();
        let real = crate::canonical(&cellar.join("forge"));
        let base = crate::canonical(&base);
        assert_eq!(stable_exe(&real), base.join("bin/forge"));
        // Fuera de Homebrew (o sin el enlace), la real.
        std::fs::remove_file(base.join("bin/forge")).unwrap();
        assert_eq!(stable_exe(&real), real);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn server_id_is_a_safe_slug_of_the_folder_name() {
        assert_eq!(server_id(Path::new("/x/His ERP_v2")), "forge-his-erp-v2");
        assert_eq!(server_id(Path::new("/x/--ñandú--")), "forge-and");
        assert_eq!(server_id(&PathBuf::from("/")), "forge");
    }

    #[test]
    fn posix_quotes_only_what_needs_it() {
        assert_eq!(quote("/usr/bin/forge", Shell::Posix), "/usr/bin/forge");
        assert_eq!(quote("--project", Shell::Posix), "--project");
        assert_eq!(quote("/a b/c", Shell::Posix), "'/a b/c'");
        assert_eq!(quote("it's", Shell::Posix), r#"'it'\''s'"#);
        assert_eq!(quote("", Shell::Posix), "''");
        assert_eq!(quote("$HOME", Shell::Posix), "'$HOME'");
    }

    #[test]
    fn windows_quotes_spaces_and_escapes_quotes_and_trailing_backslashes() {
        assert_eq!(
            quote(r"C:\forge\forge.exe", Shell::Windows),
            r"C:\forge\forge.exe"
        );
        assert_eq!(
            quote(r"C:\Program Files\Forge\forge.exe", Shell::Windows),
            r#""C:\Program Files\Forge\forge.exe""#
        );
        assert_eq!(
            quote(r"C:\mis cosas\", Shell::Windows),
            r#""C:\mis cosas\\""#
        );
        assert_eq!(quote(r#"a "b""#, Shell::Windows), r#""a \"b\"""#);
        assert_eq!(quote("", Shell::Windows), r#""""#);
    }

    #[test]
    fn command_line_quotes_paths_with_spaces() {
        assert_eq!(
            command_line(&sample(), Shell::Posix),
            "'/Volumes/Mac Ext/Forge.app/Contents/MacOS/forge' mcp --project '/Users/ana/mis proyectos/his-erp'"
        );
        let win = server(
            Path::new(r"C:\Program Files\Forge\forge.exe"),
            Path::new(r"C:\Users\ana\mis proyectos\erp"),
        );
        assert_eq!(
            command_line(&win, Shell::Windows),
            r#""C:\Program Files\Forge\forge.exe" mcp --project "C:\Users\ana\mis proyectos\erp""#
        );
    }

    #[test]
    fn config_json_uses_the_standard_mcp_servers_shape() {
        let json: serde_json::Value = serde_json::from_str(&config_json(&sample())).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "mcpServers": {
                    "forge-his-erp": {
                        "command": "/Volumes/Mac Ext/Forge.app/Contents/MacOS/forge",
                        "args": ["mcp", "--project", "/Users/ana/mis proyectos/his-erp"]
                    }
                }
            })
        );
    }
}
