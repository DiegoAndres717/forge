// Abrir el proyecto en un editor instalado (VS Code, Cursor, Antigravity…): clic derecho
// en el proyecto, ⌘K y Archivo → Abrir en el editor (⌘⇧E). Sin navegador: `open -b`.
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

// ponytail: fuera de macOS aún no se buscan editores (vendrá por el PATH).
#[cfg(not(target_os = "macos"))]
pub(super) fn installed() -> &'static [Editor] {
    &[]
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
