// Idiomas de la interfaz (inglés por defecto). El español es el texto base del código y
// `i18n_en.rs` lo traduce; sin traducción se muestra el español: nunca falla.
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lang {
    Es,
    En,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::En,
        _ => Lang::Es,
    }
}

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Es => "es",
            Lang::En => "en",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        match code.get(..2)?.to_lowercase().as_str() {
            "es" => Some(Lang::Es),
            "en" => Some(Lang::En),
            _ => None,
        }
    }
}

/// Idioma elegido (`FORGE_LANG`, luego el ajuste guardado); por defecto, inglés.
pub fn init(saved: Option<&str>) {
    let chosen = std::env::var("FORGE_LANG")
        .ok()
        .as_deref()
        .and_then(Lang::parse)
        .or_else(|| saved.and_then(Lang::parse))
        .unwrap_or(Lang::En);
    set_lang(chosen);
}

/// Como `init`, con el idioma guardado en la base de Forge (lo elige el usuario en la app).
pub fn init_from_store() {
    let saved = crate::store::Store::default_path()
        .and_then(|p| crate::store::Store::open(&p).ok())
        .and_then(|s| s.setting("language").ok().flatten());
    init(saved.as_deref());
}

static EN: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| crate::i18n_en::EN.iter().copied().collect());

/// Texto en el idioma actual (`s` es el español del código).
pub fn t(s: &'static str) -> &'static str {
    let s = translate(lang(), s);
    if cfg!(target_os = "macos") || !s.contains(MAC_KEYS) {
        return s;
    }
    // ponytail: se filtra una copia por texto con atajos; son literales, un puñado fijo.
    static PC: LazyLock<std::sync::Mutex<HashMap<&'static str, &'static str>>> =
        LazyLock::new(Default::default);
    let mut pc = PC.lock().unwrap_or_else(|e| e.into_inner());
    pc.entry(s)
        .or_insert_with(|| Box::leak(pc_keys(s).into_boxed_str()))
}

fn translate(lang: Lang, s: &'static str) -> &'static str {
    match lang {
        Lang::Es => s,
        Lang::En => EN.get(s).copied().unwrap_or(s),
    }
}

const MAC_KEYS: [char; 5] = ['⌘', '⌃', '⌥', '⇧', '↩'];

/// Atajos de macOS escritos para Linux/Windows: «⌘⇧G» → «Ctrl+Alt+G» (ver `command_mods`
/// en la app). Siempre compilada, para probarla en macOS.
pub fn pc_keys(s: &str) -> String {
    [
        ("⌘⌥", "Ctrl+Alt+Shift+"),
        ("⌘⇧", "Ctrl+Alt+"),
        ("⌘", "Ctrl+Shift+"),
        ("⌃", "Ctrl+"),
        ("⌥-", "Alt-"),
        ("⌥", "Alt+"),
        ("⇧", "Shift+"),
        ("↩", "Enter"),
    ]
    .iter()
    .fold(s.to_string(), |s, (mac, pc)| s.replace(mac, pc))
}

/// Texto con atajos en las teclas de esta plataforma (en macOS, tal cual).
pub fn keys(s: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(target_os = "macos") || !s.contains(MAC_KEYS) {
        s.into()
    } else {
        pc_keys(s).into()
    }
}

/// `tr!("Abrir")` → texto traducido; `tr!("{n} en marcha", n = 3)` → con valores.
#[macro_export]
macro_rules! tr {
    ($s:literal) => {
        $crate::i18n::t($s)
    };
    ($s:literal, $($k:ident = $v:expr),+ $(,)?) => {{
        let mut s = $crate::i18n::t($s).to_string();
        $( s = s.replace(concat!("{", stringify!($k), "}"), &$v.to_string()); )+
        s
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pc_keys_spell_out_modifiers() {
        for (mac, pc) in [
            ("⌘K", "Ctrl+Shift+K"),
            ("Git (⌘⇧G)", "Git (Ctrl+Alt+G)"),
            ("⌘⌥← → Foco", "Ctrl+Alt+Shift+← → Foco"),
            ("⌃Tab Reciente", "Ctrl+Tab Reciente"),
            ("⌥-clic o +", "Alt-clic o +"),
            ("⌥B y ⌥F", "Alt+B y Alt+F"),
            ("Anterior (⇧↩)", "Anterior (Shift+Enter)"),
            ("Sin atajos", "Sin atajos"),
        ] {
            assert_eq!(pc_keys(mac), pc);
        }
    }

    /// Todo `tr!("…")` del código tiene su inglés: sin él, la app en inglés (la de serie)
    /// enseña español, y el texto traducido no sirve para comparar ni para huellas.
    #[test]
    fn every_tr_string_has_an_english_translation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        let mut dirs = vec![root.join("src"), root.join("crates")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() && !path.ends_with("target") {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !path.ends_with("i18n_en.rs")
                    && !path.ends_with("i18n.rs")
                {
                    files.push(path);
                }
            }
        }
        let mut missing = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for literal in tr_literals(&text) {
                if !EN.contains_key(literal.as_str()) {
                    missing.push(format!("{}: {literal:?}", file.display()));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "sin traducción:\n{}",
            missing.join("\n")
        );
    }

    /// Los literales de `tr!("…")` (no `include_str!`), con los escapes de Rust aplicados.
    fn tr_literals(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (at, _) in text.match_indices("tr!(") {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let Some(body) = text[at + 4..].trim_start().strip_prefix('"') else {
                continue;
            };
            let mut s = String::new();
            let mut chars = body.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('0') => s.push('\0'),
                        // Continuación de línea: se saltan el salto y la sangría.
                        Some('\n') => while chars.next_if(|c| c.is_whitespace()).is_some() {},
                        Some(other) => s.push(other),
                        None => break,
                    },
                    _ => s.push(c),
                }
            }
            out.push(s);
        }
        out
    }

    #[test]
    fn translates_with_fallback_and_values() {
        // Sin tocar el idioma global (otros tests corren en paralelo en español).
        assert_eq!(translate(Lang::En, "Cancelar"), "Cancel");
        assert_eq!(
            translate(Lang::En, "texto sin traducir"),
            "texto sin traducir"
        );
        assert_eq!(translate(Lang::Es, "Cancelar"), "Cancelar");
        assert_eq!(tr!("{n} en marcha", n = 3), "3 en marcha");
        // Todas las plantillas traducidas conservan sus {valores}.
        for (es, en) in crate::i18n_en::EN {
            let holes = |s: &str| {
                let mut v: Vec<String> = s
                    .split('{')
                    .skip(1)
                    .filter_map(|p| p.split_once('}').map(|(k, _)| k.to_string()))
                    .collect();
                v.sort();
                v
            };
            assert_eq!(holes(es), holes(en), "{es} → {en}");
        }
    }
}
