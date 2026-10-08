// El mod de Forge para Claude Code (carpeta claude-plugin/ del repo), incluido en el
// binario y escrito al arrancar en la carpeta de datos de Forge: cada Claude que Forge
// abre lo carga con --plugin-dir, sin instalar nada en el Claude del usuario.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Carpeta del mod instalado (solo la app real; en tests no se carga).
pub static PLUGIN_DIR: OnceLock<PathBuf> = OnceLock::new();

const FILES: [(&str, &str); 4] = [
    (
        ".claude-plugin/plugin.json",
        include_str!("../claude-plugin/.claude-plugin/plugin.json"),
    ),
    (
        "hooks/hooks.json",
        include_str!("../claude-plugin/hooks/hooks.json"),
    ),
    (
        "hooks/register.tsx",
        include_str!("../claude-plugin/hooks/register.tsx"),
    ),
    (
        "types/index.d.ts",
        include_str!("../claude-plugin/types/index.d.ts"),
    ),
];

/// Escribe el mod en `base/claude-plugin` (solo lo que cambió) y devuelve la carpeta.
pub fn install(base: &Path) -> std::io::Result<PathBuf> {
    let dir = base.join("claude-plugin");
    for (path, content) in FILES {
        let file = dir.join(path);
        if std::fs::read_to_string(&file).ok().as_deref() != Some(content) {
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&file, content)?;
        }
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    #[test]
    fn writes_the_mod_files() {
        let base = std::env::temp_dir().join(format!("forge-mod-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = super::install(&base).unwrap();
        for (path, content) in super::FILES {
            assert_eq!(std::fs::read_to_string(dir.join(path)).unwrap(), content);
        }
        assert!(super::install(&base).is_ok(), "idempotente");
    }
}
