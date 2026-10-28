// Color de sintaxis del editor de archivos: gramáticas de syntect (con las extra de two-face:
// TypeScript, TOML…) y el tema oscuro «Two Dark».
use eframe::egui::{
    Color32, FontId,
    text::{LayoutJob, TextFormat},
};
use std::sync::OnceLock;
use syntect::{
    easy::HighlightLines, highlighting::Theme, parsing::SyntaxSet, util::LinesWithEndings,
};
use two_face::theme::EmbeddedThemeName;

/// Más allá de este tamaño no se colorea: el editor sigue fluido con el texto plano.
const MAX_HIGHLIGHT_BYTES: usize = 150_000;

struct Assets {
    syntaxes: SyntaxSet,
    theme: Theme,
}

fn assets() -> &'static Assets {
    static ASSETS: OnceLock<Assets> = OnceLock::new();
    ASSETS.get_or_init(|| Assets {
        syntaxes: two_face::syntax::extra_newlines(),
        theme: two_face::theme::extra()
            .get(EmbeddedThemeName::TwoDark)
            .clone(),
    })
}

/// El texto como secciones de color según el lenguaje que dice el nombre del archivo.
/// Sin gramática para él (o si es enorme) queda en un solo color.
pub fn layout(text: &str, file_name: &str, font: FontId, plain: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let add = |job: &mut LayoutJob, piece: &str, color: Color32| {
        job.append(piece, 0.0, TextFormat::simple(font.clone(), color));
    };
    let assets = assets();
    let ext = file_name.rsplit_once('.').map_or(file_name, |(_, e)| e);
    let syntax = assets
        .syntaxes
        .find_syntax_by_extension(ext)
        .filter(|_| text.len() <= MAX_HIGHLIGHT_BYTES);
    let Some(syntax) = syntax else {
        add(&mut job, text, plain);
        return job;
    };
    let mut lines = HighlightLines::new(syntax, &assets.theme);
    for line in LinesWithEndings::from(text) {
        match lines.highlight_line(line, &assets.syntaxes) {
            Ok(pieces) => {
                for (style, piece) in pieces {
                    let c = style.foreground;
                    add(&mut job, piece, Color32::from_rgb(c.r, c.g, c.b));
                }
            }
            // Una línea que la gramática no entiende se deja sin color.
            Err(_) => add(&mut job, line, plain),
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(job: &LayoutJob) -> std::collections::HashSet<[u8; 4]> {
        job.sections
            .iter()
            .map(|s| s.format.color.to_array())
            .collect()
    }

    #[test]
    fn code_gets_several_colors_and_keeps_its_text() {
        let code = "fn main() {\n    let x = \"hi\"; // nota\n}\n";
        let job = layout(code, "main.rs", FontId::monospace(12.0), Color32::WHITE);
        assert_eq!(job.text, code);
        assert!(
            colors(&job).len() >= 3,
            "palabra clave, cadena y comentario"
        );
    }

    #[test]
    fn typescript_and_toml_are_known_and_unknown_files_stay_plain() {
        let ts = layout(
            "const a: number = 1;",
            "a.ts",
            FontId::monospace(12.0),
            Color32::WHITE,
        );
        assert!(colors(&ts).len() >= 2);
        let toml = layout(
            "[package]\nname = \"x\"\n",
            "Cargo.toml",
            FontId::monospace(12.0),
            Color32::WHITE,
        );
        assert!(colors(&toml).len() >= 2);
        let none = layout(
            "lo que sea",
            "notas.zzz",
            FontId::monospace(12.0),
            Color32::WHITE,
        );
        assert_eq!(colors(&none).len(), 1);
    }
}
