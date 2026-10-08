// Lógica de Forge sin interfaz: Guard, hooks, evidencias, agentes, router de modelos,
// memoria, servidor MCP y persistencia. La app (ventana y CLI) se construye encima.
pub mod agents;
pub mod candidate;
pub mod danger;
pub mod events;
pub mod evidence;
pub mod git;
pub mod guard;
pub mod hooks;
pub mod i18n;
mod i18n_en;
pub mod ideas;
pub mod mcp;
pub mod memory;
pub mod pr;
pub mod project;
pub mod reviewers;
pub mod router;
pub mod store;

/// Quita secuencias de escape ANSI (CSI y OSC) de la salida de un comando.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ ... byte final en @..~
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL o ESC \
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
