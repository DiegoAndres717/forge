// Abrir el proyecto en un editor instalado (VS Code, Cursor, Antigravity…): clic derecho
// en el proyecto, ⌘K y Archivo → Abrir en el editor (⌘⇧E). Sin navegador: `open -b`.
use std::path::Path;
use std::sync::OnceLock;

use forge_core::store::Store;

/// Editores conocidos: (nombre, bundle id). Launch Services los encuentra estén donde
/// estén (/Applications, ~/Applications, otro disco).
const EDITORS: &[(&str, &str)] = &[
    ("Visual Studio Code", "com.microsoft.VSCode"),
    ("VS Code Insiders", "com.microsoft.VSCodeInsiders"),
    ("Cursor", "com.todesktop.230313mzl4w4u92"),
    ("Antigravity", "com.google.antigravity"),
    ("Antigravity IDE", "com.google.antigravity-ide"),
    ("Windsurf", "com.exafunction.windsurf"),
    ("Zed", "dev.zed.Zed"),
    ("Kiro", "dev.kiro.desktop"),
    ("Sublime Text", "com.sublimetext.4"),
    ("Nova", "com.panic.Nova"),
    ("BBEdit", "com.barebones.bbedit"),
    ("Xcode", "com.apple.dt.Xcode"),
    ("IntelliJ IDEA", "com.jetbrains.intellij"),
    ("IntelliJ IDEA CE", "com.jetbrains.intellij.ce"),
    ("WebStorm", "com.jetbrains.WebStorm"),
    ("PyCharm", "com.jetbrains.pycharm"),
    ("RustRover", "com.jetbrains.rustrover"),
    ("Android Studio", "com.google.android.studio"),
];

pub(super) type Editor = (&'static str, &'static str);

/// Editores instalados, en el orden de la lista.
// ponytail: se miran una vez por sesión; uno instalado con Forge abierto sale al reiniciar.
pub(super) fn installed() -> &'static [Editor] {
    static FOUND: OnceLock<Vec<Editor>> = OnceLock::new();
    FOUND.get_or_init(|| {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::NSString;
        let workspace = NSWorkspace::sharedWorkspace();
        EDITORS
            .iter()
            .copied()
            .filter(|(_, id)| {
                workspace
                    .URLForApplicationWithBundleIdentifier(&NSString::from_str(id))
                    .is_some()
            })
            .collect()
    })
}

/// El preferido (el último usado) si sigue instalado; si no, el primero.
pub(super) fn preferred(store: Option<&Store>, editors: &[Editor]) -> Option<Editor> {
    let saved = store.and_then(|s| s.setting("editor").ok().flatten());
    pick(saved.as_deref(), editors)
}

fn pick(saved: Option<&str>, editors: &[Editor]) -> Option<Editor> {
    editors
        .iter()
        .find(|e| Some(e.1) == saved)
        .or(editors.first())
        .copied()
}

/// Abre la carpeta con el editor y lo recuerda como preferido.
pub(super) fn open(store: Option<&Store>, bundle: &str, folder: &Path) -> Result<(), String> {
    std::process::Command::new("open")
        .args(["-b", bundle])
        .arg(folder)
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(store) = store {
        store.set_setting("editor", bundle)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_editor_falls_back_to_the_first_installed() {
        let editors = [("Cursor", "c"), ("Zed", "z")];
        assert_eq!(pick(Some("z"), &editors), Some(("Zed", "z")));
        assert_eq!(pick(Some("desinstalado"), &editors), Some(("Cursor", "c")));
        assert_eq!(pick(None, &editors), Some(("Cursor", "c")));
        assert_eq!(pick(Some("z"), &[]), None);
    }
}
