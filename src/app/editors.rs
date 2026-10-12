// Abrir el proyecto en un editor instalado (VS Code, Cursor, Antigravity…): clic derecho
// en el proyecto, ⌘K y Archivo → Abrir en el editor (⌘⇧E). Sin navegador: `open -b` en
// macOS; en Linux, el comando del editor.
use std::path::Path;

use eframe::egui;
use forge_core::store::Store;

/// Editores conocidos: (nombre, bundle id). Launch Services los encuentra estén donde
/// estén (/Applications, ~/Applications, otro disco).
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
pub(super) fn installed() -> &'static [Editor] {
    use std::sync::OnceLock;
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

/// Fuera de macOS: (nombre, comando que abre una carpeta), buscados en el PATH.
#[cfg(not(target_os = "macos"))]
const EDITORS: &[(&str, &str)] = &[
    ("Visual Studio Code", "code"),
    ("VS Code Insiders", "code-insiders"),
    ("Cursor", "cursor"),
    ("Antigravity", "antigravity"),
    ("Windsurf", "windsurf"),
    ("Zed", "zed"),
    ("Zed", "zeditor"),
    ("Kiro", "kiro"),
    ("Sublime Text", "subl"),
    ("IntelliJ IDEA", "idea"),
    ("WebStorm", "webstorm"),
    ("PyCharm", "pycharm"),
    ("RustRover", "rustrover"),
];

// ponytail: el PATH con el que arrancó Forge; uno instalado después sale al reiniciar.
#[cfg(not(target_os = "macos"))]
pub(super) fn installed() -> &'static [Editor] {
    use std::sync::OnceLock;
    static FOUND: OnceLock<Vec<Editor>> = OnceLock::new();
    FOUND.get_or_init(|| {
        #[cfg(not(windows))]
        let path = std::env::var_os("PATH").unwrap_or_default();
        EDITORS
            .iter()
            .copied()
            .filter(|(_, cmd)| {
                // Windows: con las extensiones de PATHEXT (code.cmd, cursor.cmd…).
                #[cfg(windows)]
                let found = forge_core::which(cmd).is_some();
                #[cfg(not(windows))]
                let found = on_path(cmd, &path);
                found
            })
            .collect()
    })
}

/// ¿Hay un ejecutable `cmd` en alguna carpeta de `path` (formato de $PATH)?
#[cfg(any(test, not(any(target_os = "macos", windows))))]
fn on_path(cmd: &str, path: &std::ffi::OsStr) -> bool {
    std::env::split_paths(path).any(|dir| dir.join(cmd).is_file())
}

/// Icono de la app tal como lo muestra macOS (64 px), como textura; se hace una vez.
pub(super) fn icon(ctx: &egui::Context, bundle: &str) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("editor-icon", bundle));
    if let Some(texture) = ctx.memory_mut(|m| m.data.get_temp::<Option<egui::TextureHandle>>(id)) {
        return texture;
    }
    let texture =
        render_icon(bundle).map(|image| ctx.load_texture(bundle, image, Default::default()));
    ctx.memory_mut(|m| m.data.insert_temp(id, texture.clone()));
    texture
}

#[cfg(not(target_os = "macos"))]
fn render_icon(_bundle: &str) -> Option<egui::ColorImage> {
    None
}

#[cfg(target_os = "macos")]
fn render_icon(bundle: &str) -> Option<egui::ColorImage> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSWorkspace};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    const SIZE: usize = 64;
    let workspace = NSWorkspace::sharedWorkspace();
    let url = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle))?;
    let path = url.path()?;
    let image = workspace.iconForFile(&path);
    // SAFETY: lienzo RGBA propio de SIZE×SIZE; se lee después de dibujar y soltar el contexto.
    unsafe {
        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            SIZE as isize,
            SIZE as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )?;
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        image.drawInRect(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(SIZE as f64, SIZE as f64),
        ));
        context.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        let (data, stride) = (rep.bitmapData(), rep.bytesPerRow() as usize);
        if data.is_null() {
            return None;
        }
        let rows = std::slice::from_raw_parts(data, stride * SIZE);
        let rgba: Vec<u8> = rows
            .chunks(stride)
            .flat_map(|row| &row[..SIZE * 4])
            .copied()
            .collect();
        Some(egui::ColorImage::from_rgba_premultiplied(
            [SIZE, SIZE],
            &rgba,
        ))
    }
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
    // En macOS `bundle` es el bundle id; en Linux, el comando del editor.
    let mut cmd = if cfg!(target_os = "macos") {
        let mut open = std::process::Command::new("open");
        open.args(["-b", bundle]);
        open
    } else {
        // Windows: la ruta completa (`…\\code.cmd`); Rust lanza los .cmd por cmd.exe con las
        // comillas bien puestas.
        #[cfg(windows)]
        let program = forge_core::which(bundle).unwrap_or_else(|| bundle.into());
        #[cfg(not(windows))]
        let program = bundle;
        std::process::Command::new(program)
    };
    cmd.arg(folder).spawn().map_err(|e| e.to_string())?;
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

    #[test]
    fn linux_editors_are_found_by_command_on_path() {
        let dir = std::env::temp_dir().join(format!("forge-editors-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("zed"), "").unwrap();
        let path = std::env::join_paths(["/no/such/dir".into(), dir.clone()]).unwrap();
        assert!(on_path("zed", &path));
        assert!(!on_path("cursor", &path));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
