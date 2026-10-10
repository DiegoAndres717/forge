mod app;
mod claude_plugin;
mod cli;
mod layout;
mod processes;
mod suggest;
mod terminal;
mod theme;
#[cfg(target_os = "macos")]
mod traffic_lights;
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
#[derive(Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    font_size: f32,
    /// Fuente monoespaciada (.ttf/.otf/.ttc). Por defecto Menlo.
    #[serde(skip_serializing_if = "Option::is_none")]
    font: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    font_bold: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    font_italic: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
/// Guarda los ajustes en `~/.config/forge/config.toml` (los cambia la ventana de Ajustes).
pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = config_path().ok_or_else(|| "no se encontró $HOME".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

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
    // Ejecutado como `rm`, `git`… desde `shims/`: control de comandos peligrosos.
    if let Some(name) = std::env::args()
        .next()
        .as_deref()
        .and_then(|a| {
            std::path::Path::new(a)
                .file_name()?
                .to_str()
                .map(str::to_string)
        })
        .filter(|n| forge_core::danger::SHIMMED.contains(&n.as_str()))
    {
        forge_core::danger::shim_main(&name, std::env::args().skip(1).collect());
    }
    // Idioma: el guardado por el usuario; si no, inglés.
    forge_core::i18n::init_from_store();
    // `forge [carpeta]` abre (o activa) ese proyecto.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // CLI sin ventana (la usan también los hooks de Git).
        Some("agent-usage") => {
            forge_core::events::record_usage();
            return Ok(());
        }
        Some("agent-event") => {
            // Codex pasa su JSON como argumento; `--project …` (del mod) no lo es.
            forge_core::events::record(
                args.get(1).map_or("stop", String::as_str),
                args.get(2)
                    .map(String::as_str)
                    .filter(|a| !a.starts_with("--")),
            );
            return Ok(());
        }
        Some(
            "guard" | "hooks" | "agent" | "doctor" | "ai" | "mcp" | "memory" | "ideas" | "plans"
            | "status" | "context",
        ) => std::process::exit(cli::run(&args)),
        Some("help" | "--help" | "-h") => {
            print!("{}", cli::usage());
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
            .with_title_shown(false)
            // Sin esto eframe pone su icono por defecto en el Dock al abrir la ventana.
            .with_icon(std::sync::Arc::new(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .unwrap_or_default(),
            )),
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
            // Terminales con control de comandos peligrosos (§15.2).
            let installed = forge_core::store::Store::default_path()
                .and_then(|db| db.parent().map(|d| d.to_path_buf()))
                .zip(std::env::current_exe().ok())
                .ok_or_else(|| "no se encontró la carpeta de datos".to_string())
                .and_then(|(base, exe)| {
                    forge_core::danger::install(&base, &exe).map_err(|e| e.to_string())
                });
            match installed {
                Ok(mut env) => {
                    if let Some(base) = forge_core::store::Store::default_path()
                        .and_then(|db| db.parent().map(|d| d.to_path_buf()))
                    {
                        let _ = workspace::HISTORY_DIR.set(base.join("scrollback"));
                        // Mod de Forge para Claude Code: cualquier `claude` de una terminal de
                        // Forge lo carga (escrito a mano o abierto desde la barra lateral).
                        if let Ok(dir) = claude_plugin::install(&base) {
                            env.insert(
                                "CLAUDE_CODE_PLUGIN_DIRS".into(),
                                dir.to_string_lossy().into_owned(),
                            );
                            let _ = claude_plugin::PLUGIN_DIR.set(dir);
                        }
                    }
                    let _ = terminal::SHELL_ENV.set(env);
                }
                Err(e) => {
                    error.get_or_insert(format!("sin control de comandos peligrosos: {e}"));
                }
            }
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
