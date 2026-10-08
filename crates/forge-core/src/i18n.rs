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
    translate(lang(), s)
}

fn translate(lang: Lang, s: &'static str) -> &'static str {
    match lang {
        Lang::Es => s,
        Lang::En => EN.get(s).copied().unwrap_or(s),
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
