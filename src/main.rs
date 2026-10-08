mod agents;
mod app;
mod candidate;
mod cli;
mod evidence;
mod guard;
mod hooks;
mod layout;
mod mcp;
mod memory;
mod processes;
mod project;
mod reviewers;
mod router;
mod store;
mod terminal;
mod theme;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, Vec2};
use serde::Deserialize;

use terminal::Metrics;

const MENLO: &str = "/System/Library/Fonts/Menlo.ttc";
const SF_MONO: &str = "/System/Library/Fonts/SFNSMono.ttf";
const SF_MONO_ITALIC: &str = "/System/Library/Fonts/SFNSMonoItalic.ttf";
/// SF Pro (fuente del sistema) para la interfaz.
const SF_PRO: &str = "/System/Library/Fonts/SFNS.ttf";
const SYMBOLS: &str = "/System/Library/Fonts/Apple Symbols.ttf";

/// `~/.config/forge/config.toml`
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    font_size: f32,
    /// Fuente monoespaciada (.ttf/.otf/.ttc). Por defecto Menlo.
    font: Option<PathBuf>,
    font_bold: Option<PathBuf>,
    font_italic: Option<PathBuf>,
    font_bold_italic: Option<PathBuf>,
    /// Option (⌥) envía ESC+tecla en vez de caracteres especiales (∂, ƒ...).
    option_as_meta: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font: None,
            font_bold: None,
            font_italic: None,
            font_bold_italic: None,
            option_as_meta: true,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/forge/config.toml"))
}

/// Lee la configuración; si es inválida devuelve los valores por defecto y el error.
fn load_settings() -> (Settings, Option<String>) {
    let Some(path) = config_path() else {
        return (Settings::default(), None);
    };
    match std::fs::read_to_string(&path) {
        Err(_) => (Settings::default(), None),
        Ok(text) => match toml::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                Settings::default(),
                Some(format!("{}: {e}", path.display())),
            ),
        },
    }
}

pub fn install_fonts(ctx: &egui::Context, s: &Settings) -> Result<(), String> {
    let load = |path: &PathBuf, index: u32| {
        std::fs::read(path)
            .map(|bytes| {
                Arc::new(FontData {
                    index,
                    ..FontData::from_owned(bytes)
                })
            })
            .map_err(|e| format!("no se pudo leer la fuente {}: {e}", path.display()))
    };
    let menlo = PathBuf::from(MENLO);
    let (sf_mono, sf_mono_italic) = (PathBuf::from(SF_MONO), PathBuf::from(SF_MONO_ITALIC));
    // Por defecto SF Mono (la de Terminal.app). Solo existe con peso variable y egui no
    // puede elegir peso, así que la negrita usa Menlo Bold (Menlo.ttc: 1 negrita, 3 negrita
    // cursiva). Sin SF Mono, todo Menlo. Con fuente propia, las variantes que falten usan la regular.
    let variants = match &s.font {
        None if sf_mono.exists() && sf_mono_italic.exists() => [
            (&sf_mono, 0),
            (&menlo, 1),
            (&sf_mono_italic, 0),
            (&menlo, 3),
        ],
        None => [(&menlo, 0), (&menlo, 1), (&menlo, 2), (&menlo, 3)],
        Some(regular) => [
            (regular, 0),
            (s.font_bold.as_ref().unwrap_or(regular), 0),
            (s.font_italic.as_ref().unwrap_or(regular), 0),
            (s.font_bold_italic.as_ref().unwrap_or(regular), 0),
        ],
    };

    let mut defs = FontDefinitions::default();
    let fallbacks = defs.families[&FontFamily::Monospace].clone();
    let symbols = PathBuf::from(SYMBOLS);
    if let Ok(data) = load(&symbols, 0) {
        defs.font_data.insert("symbols".into(), data);
    }

    let families = [
        FontFamily::Monospace,
        FontFamily::Name("bold".into()),
        FontFamily::Name("italic".into()),
        FontFamily::Name("bold_italic".into()),
    ];
    for (i, (family, (path, index))) in families.into_iter().zip(variants).enumerate() {
        let name = format!("term-{i}");
        defs.font_data.insert(name.clone(), load(path, index)?);
        let mut chain = vec![name];
        if defs.font_data.contains_key("symbols") {
            chain.push("symbols".into());
        }
        chain.extend(fallbacks.iter().cloned());
        defs.families.insert(family, chain);
    }
    // Interfaz: SF Pro, con los iconos Phosphor como respaldo para poder mezclarlos en el texto.
    if let Ok(data) = load(&PathBuf::from(SF_PRO), 0) {
        defs.font_data.insert("sf-pro".into(), data);
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "sf-pro".into());
    }
    egui_phosphor::add_to_fonts(&mut defs, egui_phosphor::Variant::Regular);
    ctx.set_fonts(defs);
    theme::apply(ctx);
    Ok(())
}

/// Tamaño de celda alineado a píxeles físicos para que el texto no se vea borroso.
pub fn metrics(ctx: &egui::Context, s: &Settings) -> Metrics {
    let font = FontId::monospace(s.font_size);
    let ppp = ctx.pixels_per_point();
    let (w, h) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let snap = |v: f32| (v * ppp).round() / ppp;
    Metrics {
        font_size: s.font_size,
        cell: Vec2::new(snap(w), snap(h)),
        option_as_meta: s.option_as_meta,
    }
}

fn main() -> eframe::Result {
    // `forge [carpeta]` abre (o activa) ese proyecto.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // CLI sin ventana (la usan también los hooks de Git).
        Some("guard" | "hooks" | "agent" | "doctor" | "ai" | "mcp" | "memory") => {
            std::process::exit(cli::run(&args))
        }
        Some("help" | "--help" | "-h") => {
            print!("{}", cli::USAGE);
            return Ok(());
        }
        _ => {}
    }
    let open = match args.first().map(String::as_str) {
        Some("open") => args.get(1),
        _ => args.first(),
    }
    .map(PathBuf::from);
    let options = eframe::NativeOptions {
        // Ventana nativa de macOS: el contenido sube bajo la barra de título (los semáforos
        // quedan sobre la barra lateral). Sin transparencia: Forge pinta con Metal en la propia
        // vista y un NSVisualEffectView (desenfoque) quedaría encima tapando todo.
        viewport: egui::ViewportBuilder::default()
            .with_title("Forge")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([760.0, 480.0])
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        ..Default::default()
    };
    eframe::run_native(
        "Forge",
        options,
        Box::new(|cc| {
            let (settings, mut error) = load_settings();
            if let Err(e) = install_fonts(&cc.egui_ctx, &settings) {
                error = Some(e);
                install_fonts(&cc.egui_ctx, &Settings::default())?;
            }
            // ⌘+/⌘- cambian el tamaño de fuente del terminal, no el zoom de la UI.
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            Ok(Box::new(app::App::new(&cc.egui_ctx, settings, error, open)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse_and_reject_typos() {
        let s: Settings = toml::from_str("font_size = 16\noption_as_meta = false").unwrap();
        assert_eq!(s.font_size, 16.0);
        assert!(!s.option_as_meta);
        assert!(toml::from_str::<Settings>("font_sise = 16").is_err());
    }
}
