// Pegar imágenes y soltar archivos en una terminal: los agentes leen las imágenes por su
// ruta (Claude, Codex, Gemini, OpenCode, Qwen), así que basta con escribir la ruta.
use eframe::egui;
use std::path::{Path, PathBuf};

/// Las rutas como las escribe Terminal.app al soltar archivos: separadas por espacios y con
/// los caracteres especiales escapados (en Windows, entre comillas simples de PowerShell).
pub fn drop_text(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for path in paths {
        let path = path.to_string_lossy();
        if cfg!(windows) {
            // PowerShell: entre comillas simples no se expande nada; la ' se dobla.
            out.push_str(&format!("'{}'", path.replace('\'', "''")));
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

/// ¿Se soltó una V cuya pulsación no llegó? egui se traga la pulsación de ⌘V (Ctrl+V fuera
/// de macOS) y solo deja pasar la suelta, con o sin el modificador según cuál se suelte
/// antes. `v_down` recuerda entre fotogramas si se vio la pulsación (una «v» escrita).
pub fn swallowed_paste(events: &[egui::Event], v_down: &mut bool) -> bool {
    let mut swallowed = false;
    for event in events {
        if let egui::Event::Key {
            key: egui::Key::V,
            pressed,
            ..
        } = event
        {
            if !*pressed && !*v_down {
                swallowed = true;
            }
            *v_down = *pressed;
        }
    }
    swallowed
}

/// Guarda una imagen RGBA del portapapeles como PNG en `dir` y devuelve su ruta.
pub fn save_png(width: usize, height: usize, rgba: &[u8], dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec())
        .ok_or("imagen del portapapeles con un tamaño que no cuadra")?;
    // Nombre corto (captura-1.png, captura-2.png…): el primer número libre. `create_new`
    // evita pisar otra pegada simultánea (otra ventana de Forge).
    let name = forge_core::tr!("captura");
    for n in 1.. {
        let path = dir.join(format!("{name}-{n}.png"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                image
                    .write_to(&mut file, image::ImageFormat::Png)
                    .map_err(|e| e.to_string())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    unreachable!()
}

/// Las rutas dentro de `base` (la carpeta del agente), relativas: más cortas y legibles en
/// el mensaje, y todos los agentes las entienden. Las de fuera, enteras.
pub fn relative_to(paths: &[PathBuf], base: Option<&Path>) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|p| {
            base.and_then(|b| p.strip_prefix(b).ok())
                .filter(|r| !r.as_os_str().is_empty())
                .map_or_else(|| p.clone(), Path::to_path_buf)
        })
        .collect()
}

/// Carpeta (dentro de la temporal) de las imágenes pegadas.
pub const PASTE_DIR: &str = "forge-paste";

/// Si el portapapeles tiene una imagen y ningún texto (una captura, una imagen copiada),
/// la guarda como PNG temporal y devuelve su ruta.
// ponytail: la carpeta temporal no se limpia; el sistema la vacía.
pub fn clipboard_image() -> Option<PathBuf> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if clipboard.get_text().is_ok_and(|t| !t.is_empty()) {
        return None;
    }
    let image = clipboard.get_image().ok()?;
    let dir = std::env::temp_dir().join(PASTE_DIR);
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
            // PowerShell: comillas simples (nada se expande dentro), la ' se dobla.
            assert_eq!(
                text,
                "'/Users/ana/Captura de pantalla.png' '/tmp/it''s(1).txt' '/tmp/plano.png' "
            );
        } else {
            assert_eq!(
                text,
                r"/Users/ana/Captura\ de\ pantalla.png /tmp/it\'s\(1\).txt /tmp/plano.png "
            );
        }
    }

    #[test]
    fn a_v_released_without_its_press_was_a_swallowed_paste() {
        let v = |pressed, command| egui::Event::Key {
            key: egui::Key::V,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers {
                command,
                ..Default::default()
            },
        };
        let mut down = false;
        // ⌘V con solo una imagen: egui se traga la pulsación y solo llega la suelta.
        assert!(swallowed_paste(&[v(false, true)], &mut down));
        // Se soltó ⌘ antes que la V: la suelta llega sin modificador, y cuenta igual.
        assert!(swallowed_paste(&[v(false, false)], &mut down));
        // Escribir una «v»: pulsación y suelta, aunque sea en fotogramas distintos.
        assert!(!swallowed_paste(
            &[v(true, false), v(false, false)],
            &mut down
        ));
        assert!(!swallowed_paste(&[v(true, false)], &mut down));
        assert!(!swallowed_paste(&[v(false, false)], &mut down));
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
        // Dos pegados seguidos no se pisan, y el nombre es corto y legible.
        let second = save_png(2, 1, &rgba, &dir).unwrap();
        assert_eq!(path.file_name().unwrap(), "captura-1.png");
        assert_eq!(second.file_name().unwrap(), "captura-2.png");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn project_files_are_sent_relative_to_the_agent_folder() {
        let base = Path::new("/proj");
        let paths = [
            PathBuf::from("/proj/src/main.rs"),
            PathBuf::from("/tmp/forge-paste/captura-1.png"),
            PathBuf::from("/project/x.rs"),
        ];
        assert_eq!(
            relative_to(&paths, Some(base)),
            [
                PathBuf::from("src/main.rs"),
                PathBuf::from("/tmp/forge-paste/captura-1.png"),
                PathBuf::from("/project/x.rs"),
            ]
        );
        // Sin carpeta conocida, las rutas tal cual.
        assert_eq!(relative_to(&paths, None), paths);
    }
}
