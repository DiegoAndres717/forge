// Linux: lanzador (.desktop) e icono de Forge en ~/.local/share, para que el menú de
// aplicaciones, el buscador y la barra lo muestren (en Wayland el icono sale del .desktop
// que coincide con el app_id de la ventana, no de la ventana). Lo escribe la propia app:
// el .tar.gz solo trae el binario.
use std::path::Path;

/// Escribe (o actualiza) el lanzador y el icono bajo `data_home` (XDG_DATA_HOME).
pub fn install(data_home: &Path, exe: &Path) -> std::io::Result<()> {
    // ponytail: el PNG es de 1024 px en la carpeta de 512; los temas lo escalan.
    let icon = data_home.join("icons/hicolor/512x512/apps/forge.png");
    write_if_changed(&icon, include_bytes!("../assets/icon.png"))?;
    let exe = exe.to_string_lossy().replace('"', "\\\"");
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Forge\n\
         Comment=Workspace for coding with AI agents\nExec=\"{exe}\" %F\nIcon=forge\n\
         Terminal=false\nCategories=Development;\nStartupWMClass=forge\n"
    );
    write_if_changed(
        &data_home.join("applications/forge.desktop"),
        entry.as_bytes(),
    )
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_launcher_and_icon_once() {
        let home = std::env::temp_dir().join(format!("forge-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let exe = Path::new("/opt/mis apps/forge");
        install(&home, exe).unwrap();
        let entry = std::fs::read_to_string(home.join("applications/forge.desktop")).unwrap();
        assert!(entry.contains("Name=Forge"), "{entry}");
        assert!(
            entry.contains(r#"Exec="/opt/mis apps/forge" %F"#),
            "{entry}"
        );
        assert!(entry.contains("StartupWMClass=forge"), "{entry}");
        let icon = home.join("icons/hicolor/512x512/apps/forge.png");
        assert!(entry.contains("Icon=forge"), "{entry}");
        assert!(std::fs::metadata(&icon).unwrap().len() > 1000);
        // Sin cambios no se reescribe.
        let before = std::fs::metadata(&icon).unwrap().modified().unwrap();
        install(&home, exe).unwrap();
        assert_eq!(
            std::fs::metadata(&icon).unwrap().modified().unwrap(),
            before
        );
        let _ = std::fs::remove_dir_all(home);
    }
}
