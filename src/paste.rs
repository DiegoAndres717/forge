// Pegar imágenes y soltar archivos en una terminal: los agentes leen las imágenes por su
// ruta (Claude, Codex, Gemini, OpenCode, Qwen), así que basta con escribir la ruta.
use std::path::{Path, PathBuf};

/// Las rutas como las escribe Terminal.app al soltar archivos: separadas por espacios y con
/// los caracteres especiales escapados (en Windows, entre comillas dobles).
pub fn drop_text(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for path in paths {
        let path = path.to_string_lossy();
        if cfg!(windows) {
            out.push_str(&format!("\"{path}\""));
        } else {
            for c in path.chars() {
                if !(c.is_alphanumeric() || "/._-+,:@%".contains(c)) {
                    out.push('\\');
                }
                out.push(c);
            }
        }
        out.push(' ');
    }
    out
}

/// Guarda una imagen RGBA del portapapeles como PNG en `dir` y devuelve su ruta.
pub fn save_png(width: usize, height: usize, rgba: &[u8], dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("imagen-{stamp}.png"));
    let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec())
        .ok_or("imagen del portapapeles con un tamaño que no cuadra")?;
    image.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Si el portapapeles tiene una imagen y ningún texto (una captura, una imagen copiada),
/// la guarda como PNG temporal y devuelve su ruta.
// ponytail: la carpeta temporal no se limpia; el sistema la vacía.
pub fn clipboard_image() -> Option<PathBuf> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if clipboard.get_text().is_ok_and(|t| !t.is_empty()) {
        return None;
    }
    let image = clipboard.get_image().ok()?;
    let dir = std::env::temp_dir().join("forge-paste");
    save_png(image.width, image.height, &image.bytes, &dir).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_are_escaped_like_terminal_app() {
        let paths = [
            PathBuf::from("/Users/ana/Captura de pantalla.png"),
            PathBuf::from("/tmp/it's(1).txt"),
            PathBuf::from("/tmp/plano.png"),
        ];
        let text = drop_text(&paths);
        if cfg!(windows) {
            assert!(text.starts_with('"'), "{text}");
        } else {
            assert_eq!(
                text,
                r"/Users/ana/Captura\ de\ pantalla.png /tmp/it\'s\(1\).txt /tmp/plano.png "
            );
        }
    }

    #[test]
    fn clipboard_images_become_png_files() {
        let dir = std::env::temp_dir().join(format!("forge-paste-{}", std::process::id()));
        // 2×1 píxeles: rojo y azul.
        let rgba = [255, 0, 0, 255, 0, 0, 255, 255];
        let path = save_png(2, 1, &rgba, &dir).unwrap();
        assert_eq!(path.extension().unwrap(), "png");
        let image = image::open(&path).unwrap().to_rgba8();
        assert_eq!((image.width(), image.height()), (2, 1));
        assert_eq!(image.into_raw(), rgba);
        // Dos pegados seguidos no se pisan.
        assert_ne!(save_png(2, 1, &rgba, &dir).unwrap(), path);
        let _ = std::fs::remove_dir_all(dir);
    }
}
