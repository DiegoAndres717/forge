// Sugerencias de carpetas y archivos mientras se escribe en la terminal (como Warp).
//
// El zsh de Forge (danger.rs) manda en cada redibujado de la línea una secuencia invisible
// `ESC ] 7777 ; <línea hasta el cursor> US <carpeta actual> BEL`, y una vacía al ejecutarla.
// Con eso la terminal calcula aquí qué sugerir y qué teclas enviar al aceptar.
use std::path::{Path, PathBuf};

const OSC_LINE: &[u8] = b"\x1b]7777;";
/// Máximo de sugerencias a la vista.
const MAX: usize = 8;

/// Línea que se está escribiendo en el prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellLine {
    /// Texto a la izquierda del cursor.
    pub left: String,
    pub pwd: PathBuf,
}

/// Extrae las secuencias de línea de la salida del PTY (pueden llegar partidas en varias
/// lecturas). `None` = la línea se ejecutó o se abandonó: no hay nada que sugerir.
#[derive(Default)]
pub struct LineScanner {
    matched: usize,
    inside: bool,
    buf: Vec<u8>,
}

impl LineScanner {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Option<ShellLine>> {
        let mut out = Vec::new();
        for &b in bytes {
            if self.inside {
                if b == 0x07 {
                    self.inside = false;
                    out.push(parse(&std::mem::take(&mut self.buf)));
                } else if self.buf.len() < 16 * 1024 {
                    self.buf.push(b);
                }
                continue;
            }
            if b == OSC_LINE[self.matched] {
                self.matched += 1;
                if self.matched == OSC_LINE.len() {
                    self.matched = 0;
                    self.inside = true;
                }
            } else {
                self.matched = usize::from(b == OSC_LINE[0]);
            }
        }
        out
    }
}

fn parse(buf: &[u8]) -> Option<ShellLine> {
    let text = String::from_utf8_lossy(buf);
    let (left, pwd) = text.split_once('\x1f')?;
    Some(ShellLine {
        left: left.to_string(),
        pwd: PathBuf::from(pwd),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
}

/// Lista de sugerencias para la palabra que se escribe.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggest {
    pub items: Vec<Entry>,
    pub selected: usize,
    /// Se eligió con ↑↓: entonces Enter acepta la elegida (si no, Enter ejecuta lo escrito).
    pub navigated: bool,
    /// Lo escrito tras la última `/` de la palabra, tal cual (con escapes): lo que se borra
    /// si la sugerencia no empieza exactamente igual.
    typed: String,
}

impl Suggest {
    /// Teclas a enviar al shell para aceptar la sugerencia elegida.
    pub fn accept(&self) -> Vec<u8> {
        let Some(entry) = self.items.get(self.selected) else {
            return Vec::new();
        };
        let mut full = escape(&entry.name);
        if entry.dir {
            full.push('/');
        }
        match full.strip_prefix(&self.typed) {
            Some(rest) => rest.as_bytes().to_vec(),
            // Distinta en mayúsculas o coincidencia por el medio: se borra lo escrito y se
            // escribe el nombre entero.
            None => {
                let mut bytes = vec![0x7f; self.typed.chars().count()];
                bytes.extend(full.as_bytes());
                bytes
            }
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        self.navigated = true;
    }
}

/// Qué sugerir para la línea: carpetas tras `cd`/`pushd`, o carpetas y archivos cuando la
/// palabra parece una ruta (`/`, `~`, `.`). Sin distinguir mayúsculas; primero lo que
/// empieza igual y luego lo que lo contiene.
pub fn suggest(line: &ShellLine, home: &Path) -> Option<Suggest> {
    let words = split_words(&line.left);
    let ends_in_space = line.left.ends_with(' ') && !line.left.ends_with("\\ ");
    let word = if ends_in_space {
        ""
    } else {
        words.last().map_or("", String::as_str)
    };
    let first = words.first().map_or("", String::as_str);
    let after_command = words.len() > 1 || ends_in_space;
    let dirs_only = after_command && matches!(first, "cd" | "pushd" | "rmdir");
    let looks_like_path = word.contains('/') || word.starts_with('~') || word.starts_with('.');
    if !(dirs_only || (looks_like_path && (after_command || word.contains('/')))) {
        return None;
    }
    let (base, typed) = match word.rfind('/') {
        Some(i) => (&word[..=i], &word[i + 1..]),
        None if word == "~" => return None,
        None => ("", word),
    };
    let base = unescape(base);
    let dir = if base.is_empty() {
        line.pwd.clone()
    } else if let Some(rest) = base.strip_prefix("~/") {
        home.join(rest)
    } else if base.starts_with('/') {
        PathBuf::from(&base)
    } else {
        line.pwd.join(&base)
    };
    let prefix = unescape(typed).to_lowercase();
    let mut found: Vec<(u8, Entry)> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !prefix.starts_with('.') {
                return None;
            }
            let lower = name.to_lowercase();
            let rank = if lower.starts_with(&prefix) {
                0
            } else if !prefix.is_empty() && lower.contains(&prefix) {
                1
            } else {
                return None;
            };
            // metadata sigue los enlaces: un enlace a carpeta cuenta como carpeta.
            let is_dir = std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir());
            (is_dir || !dirs_only).then_some((rank, Entry { name, dir: is_dir }))
        })
        .collect();
    found.sort_by(|(ra, a), (rb, b)| {
        (ra, !a.dir, a.name.to_lowercase()).cmp(&(rb, !b.dir, b.name.to_lowercase()))
    });
    let items: Vec<Entry> = found.into_iter().map(|(_, e)| e).take(MAX).collect();
    // Ya escrito entero y sin nada que añadir: no estorbar.
    if items.is_empty() || (items.len() == 1 && !items[0].dir && items[0].name == unescape(typed)) {
        return None;
    }
    Some(Suggest {
        items,
        selected: 0,
        navigated: false,
        typed: typed.to_string(),
    })
}

/// Palabras de la línea separadas por espacios (un espacio con `\` delante no separa).
fn split_words(line: &str) -> Vec<String> {
    let mut words = vec![String::new()];
    let mut escaped = false;
    for c in line.chars() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c.is_whitespace() {
            if !words.last().is_some_and(String::is_empty) {
                words.push(String::new());
            }
            continue;
        }
        words.last_mut().unwrap().push(c);
    }
    words.retain(|w| !w.is_empty());
    words
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

/// Escapa lo que el shell interpretaría (espacios, comillas, `$`, `*`…).
fn escape(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if " '\"\\()&;|<>$`!*?[]{}#~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(left: &str, pwd: &Path) -> ShellLine {
        ShellLine {
            left: left.into(),
            pwd: pwd.into(),
        }
    }

    #[test]
    fn scanner_finds_lines_split_across_reads() {
        let mut s = LineScanner::default();
        assert!(s.feed(b"hola \x1b]77").is_empty());
        let got = s.feed(b"77;cd pro\x1f/tmp\x07 y \x1b]7777;\x07");
        assert_eq!(
            got,
            vec![
                Some(line("cd pro", Path::new("/tmp"))),
                None, // línea ejecutada
            ]
        );
    }

    #[test]
    fn suggests_folders_case_insensitively_and_accepts_with_the_right_keys() {
        let dir = std::env::temp_dir().join(format!("forge-suggest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["Programar", "proyectos", "Mis Fotos", ".oculta"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("programa.txt"), "").unwrap();
        let home = Path::new("/nonexistent");

        // `cd pro`: solo carpetas, sin distinguir mayúsculas.
        let s = suggest(&line("cd pro", &dir), home).unwrap();
        let names: Vec<&str> = s.items.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Programar", "proyectos"]);
        // "pro" → "Programar/": distinta en mayúscula, se borra lo escrito y se escribe entero.
        assert_eq!(s.accept(), b"\x7f\x7f\x7fProgramar/".to_vec());
        let mut s2 = s.clone();
        s2.move_by(1);
        assert_eq!(
            s2.accept(),
            b"yectos/".to_vec(),
            "misma caja: solo lo que falta"
        );
        s2.move_by(1);
        assert_eq!(s2.selected, 0, "da la vuelta");
        assert!(
            s2.navigated && !s.navigated,
            "elegir con flechas se recuerda"
        );

        // `cd ` sin escribir nada: todas las carpetas visibles (no las ocultas).
        let all = suggest(&line("cd ", &dir), home).unwrap();
        assert_eq!(all.items.len(), 3);
        // Espacios escapados al aceptar.
        let fotos = suggest(&line("cd mis", &dir), home).unwrap();
        assert_eq!(fotos.accept(), b"\x7f\x7f\x7fMis\\ Fotos/".to_vec());

        // Una ruta tras otro comando: carpetas y archivos (carpetas primero).
        let cat = suggest(&line("cat ./progr", &dir), home).unwrap();
        let names: Vec<&str> = cat.items.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Programar", "programa.txt"]);

        // Un comando sin ruta no sugiere nada; tampoco algo ya escrito entero.
        assert!(suggest(&line("git sta", &dir), home).is_none());
        assert!(suggest(&line("cd", &dir), home).is_none());
        assert!(suggest(&line("cat ./programa.txt", &dir), home).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
