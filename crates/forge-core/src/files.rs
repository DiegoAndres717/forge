//! Árbol de archivos de un proyecto: listar una carpeta y mandar a la papelera.

use std::path::{Component, Path, PathBuf};

/// Una entrada de una carpeta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// Lo que hay dentro de `root/rel`: carpetas primero, luego archivos, cada grupo por nombre.
/// Se salta `.git` (no es trabajo del agente y llena el árbol de ruido).
pub fn list_dir(root: &Path, rel: &Path) -> Result<Vec<Entry>, String> {
    let dir = safe_path(root, rel)?;
    let inside = dir
        .canonicalize()
        .is_ok_and(|d| root.canonicalize().is_ok_and(|r| d.starts_with(r)));
    if !inside {
        return Err(format!("ruta fuera del proyecto: {}", rel.display()));
    }
    let mut out: Vec<Entry> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // ponytail: un enlace roto o ilegible se muestra como archivo.
            let is_dir = e.path().is_dir();
            (!name.eq_ignore_ascii_case(".git")).then_some(Entry { name, is_dir })
        })
        .collect();
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Carpetas que no se recorren al buscar archivos: ruido que el dueño no quiere abrir.
const SKIPPED: [&str; 3] = [".git", "node_modules", "target"];

/// Tope de archivos al buscar, para que un proyecto enorme no congele la paleta.
const MAX_FILES: usize = 20_000;

/// Rutas (relativas, con `/`) de todos los archivos del proyecto, ordenadas.
/// No sigue enlaces a carpetas, así no sale del proyecto ni entra en bucles.
pub fn project_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(rel) = pending.pop() {
        let Ok(dir) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for e in dir.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if !SKIPPED.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                    pending.push(rel.join(&name));
                }
            } else if out.len() < MAX_FILES {
                out.push(rel.join(&name).to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

/// Muestra un archivo o carpeta del proyecto en el Finder (solo macOS).
pub fn reveal(root: &Path, rel: &Path) -> Result<(), String> {
    let path = strict_path(root, rel)?;
    if !cfg!(target_os = "macos") {
        return Err("mostrar en el Finder solo existe en macOS".into());
    }
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(drop)
        .map_err(|e| e.to_string())
}

/// El mensaje para el agente con los comentarios hechos sobre el diff de un archivo.
/// Una sola línea: así se escribe en la terminal del agente y se envía con Enter.
/// Cada comentario lleva la línea del diff a la que se refiere.
pub fn review_prompt(path: &str, comments: &[(String, String)]) -> String {
    let items: Vec<String> = comments
        .iter()
        .enumerate()
        .map(|(i, (line, text))| {
            let line = line.trim().replace('`', "'");
            format!("{}) on `{}`: {}", i + 1, line, text.trim())
        })
        .collect();
    format!(
        "Review comments on {path}, please address each one: {}",
        items.join(" ")
    )
}

/// `root/rel` solo si queda dentro de `root`: sin `..`, sin ruta absoluta.
pub fn safe_path(root: &Path, rel: &Path) -> Result<PathBuf, String> {
    if rel
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!("ruta fuera del proyecto: {}", rel.display()));
    }
    let path = root.join(rel);
    // Un enlace del proyecto que apunta fuera no deja salir: la carpeta que contiene el
    // destino tiene que seguir dentro (borrar el propio enlace sí vale).
    let inside = |p: &Path| match (p.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(root)) => p.starts_with(root),
        _ => false,
    };
    let is_root = rel.components().all(|c| matches!(c, Component::CurDir));
    if !is_root && !path.parent().is_some_and(inside) {
        return Err(format!("ruta fuera del proyecto: {}", rel.display()));
    }
    Ok(path)
}

/// Manda `root/rel` a la papelera del sistema (recuperable). Nunca el propio proyecto ni `.git`.
pub fn trash(root: &Path, rel: &Path) -> Result<(), String> {
    let path = safe_path(root, rel)?;
    let first = rel.components().find(|c| matches!(c, Component::Normal(_)));
    if first.is_none() {
        return Err("no se borra la carpeta del proyecto".into());
    }
    if first.is_some_and(|c| c.as_os_str().eq_ignore_ascii_case(".git")) {
        return Err("no se borra .git desde aquí".into());
    }
    if !path.exists() {
        return Err(format!("no existe: {}", rel.display()));
    }
    #[cfg(target_os = "macos")]
    return ::trash::delete(&path).map_err(|e| e.to_string());
    #[cfg(not(target_os = "macos"))]
    Err("la papelera solo está disponible en macOS".into())
}

/// Lo que se puede mostrar de un archivo.
#[derive(Debug, PartialEq, Eq)]
pub enum Loaded {
    Text(String),
    Image,
    Pdf,
    /// Binario, demasiado grande o ilegible: el motivo, para enseñarlo.
    Unsupported(String),
}

/// Tope para abrir un archivo de texto en el editor.
const MAX_TEXT: u64 = 2 * 1024 * 1024;

/// Abre `root/rel` según lo que sea: texto editable, imagen (png/jpg), PDF o nada que mostrar.
pub fn load(root: &Path, rel: &Path) -> Result<Loaded, String> {
    let path = strict_path(root, rel)?;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err("es una carpeta".into());
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => return Ok(Loaded::Image),
        "pdf" => return Ok(Loaded::Pdf),
        _ => {}
    }
    if meta.len() > MAX_TEXT {
        return Ok(Loaded::Unsupported("el archivo pesa más de 2 MB".into()));
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Ok(Loaded::Unsupported("es un archivo binario".into()));
    }
    Ok(match String::from_utf8(bytes) {
        Ok(text) => Loaded::Text(text),
        Err(_) => Loaded::Unsupported("no está en UTF-8".into()),
    })
}

/// Como `safe_path`, pero si el archivo existe y es un enlace, su destino también tiene que
/// estar dentro del proyecto (abrir o escribir no pueden salir por un enlace).
fn strict_path(root: &Path, rel: &Path) -> Result<PathBuf, String> {
    let path = safe_path(root, rel)?;
    if path.exists() {
        let inside = path
            .canonicalize()
            .is_ok_and(|p| root.canonicalize().is_ok_and(|r| p.starts_with(r)));
        if !inside {
            return Err(format!("ruta fuera del proyecto: {}", rel.display()));
        }
    }
    Ok(path)
}

/// Guarda `content` en `root/rel` (el archivo existe o su carpeta existe).
pub fn save(root: &Path, rel: &Path, content: &str) -> Result<(), String> {
    let path = strict_path(root, rel)?;
    if path.is_dir() {
        return Err("es una carpeta".into());
    }
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

/// Lo que ha cambiado `root/rel` respecto al último commit, en formato de diff. Un archivo
/// nuevo sale entero como añadido.
pub fn diff(root: &Path, rel: &Path) -> Result<String, String> {
    let path = strict_path(root, rel)?;
    crate::git::file_diff(root, &rel.to_string_lossy(), &path)
}

/// Imagen de la primera página de un PDF, generada con la vista rápida de macOS.
#[cfg(target_os = "macos")]
pub fn pdf_preview(root: &Path, rel: &Path) -> Result<PathBuf, String> {
    let path = strict_path(root, rel)?;
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    let dir = std::env::temp_dir().join(format!(
        "forge-pdf-{}-{:x}",
        std::process::id(),
        hasher.finish()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let png = dir.join(format!(
        "{}.png",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::remove_file(&png); // que una vista vieja no pase por la nueva
    let status = std::process::Command::new("qlmanage")
        .args(["-t", "-s", "1400", "-o"])
        .arg(&dir)
        .arg(&path)
        .output()
        .map_err(|e| e.to_string())?;
    if status.status.success() && png.exists() {
        Ok(png)
    } else {
        Err("no se pudo generar la vista previa del PDF".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_review_prompt_names_the_file_and_each_commented_line() {
        let comments = vec![
            ("+let x = 1;".to_string(), "rename x".to_string()),
            ("-old `code`".to_string(), " why removed? ".to_string()),
        ];
        assert_eq!(
            review_prompt("src/a.rs", &comments),
            "Review comments on src/a.rs, please address each one: \
             1) on `+let x = 1;`: rename x 2) on `-old 'code'`: why removed?"
        );
    }

    #[test]
    fn project_files_lists_nested_files_and_skips_noise() {
        let root = tmp("walk");
        for d in ["src/app", ".git", "node_modules/x", "target"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "README.md",
            "src/app/mod.rs",
            ".git/HEAD",
            "node_modules/x/i.js",
            "target/o",
        ] {
            std::fs::write(root.join(f), "").unwrap();
        }
        assert_eq!(project_files(&root), ["README.md", "src/app/mod.rs"]);
    }

    #[test]
    fn folders_come_first_then_files_by_name_and_git_is_hidden() {
        let root = tmp("order");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("Docs")).unwrap();
        std::fs::write(root.join("b.txt"), "").unwrap();
        std::fs::write(root.join("A.md"), "").unwrap();
        let names: Vec<_> = list_dir(&root, Path::new(""))
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.is_dir))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Docs".to_string(), true),
                ("src".to_string(), true),
                ("A.md".to_string(), false),
                ("b.txt".to_string(), false),
            ]
        );
    }

    #[test]
    fn paths_that_leave_the_project_are_refused() {
        let root = tmp("escape");
        assert!(list_dir(&root, Path::new("../")).is_err());
        assert!(list_dir(&root, Path::new("/etc")).is_err());
        assert!(trash(&root, Path::new("a/../../x")).is_err());
    }

    #[test]
    fn the_project_folder_and_git_are_never_trashed() {
        let root = tmp("keep");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(trash(&root, Path::new("")).is_err());
        assert!(trash(&root, Path::new(".")).is_err());
        assert!(trash(&root, Path::new(".git")).is_err());
        assert!(trash(&root, Path::new(".git/HEAD")).is_err());
        assert!(root.join(".git").exists());
    }

    #[test]
    fn git_is_protected_whatever_the_case() {
        let root = tmp("case");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        // macOS no distingue mayúsculas: `.GIT` es la misma carpeta.
        assert!(trash(&root, Path::new(".GIT")).is_err());
        assert!(trash(&root, Path::new(".Git/HEAD")).is_err());
        assert!(root.join(".git").exists());
    }

    #[test]
    #[cfg(unix)]
    fn a_link_pointing_outside_the_project_is_not_followed() {
        let root = tmp("link");
        let outside = tmp("link-outside");
        std::fs::write(outside.join("secreto.txt"), "").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("fuera")).unwrap();
        assert!(list_dir(&root, Path::new("fuera")).is_err());
        assert!(trash(&root, Path::new("fuera/secreto.txt")).is_err());
        assert!(outside.join("secreto.txt").exists());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let root = tmp("missing");
        assert!(trash(&root, Path::new("nada.txt")).is_err());
    }

    #[test]
    fn load_tells_text_images_pdfs_and_binaries_apart() {
        let root = tmp("load");
        std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("logo.PNG"), [0x89, b'P']).unwrap();
        std::fs::write(root.join("doc.pdf"), "%PDF").unwrap();
        std::fs::write(root.join("bin.dat"), [1u8, 0, 2]).unwrap();
        std::fs::write(root.join("latin.txt"), [0xff, 0xfe, b'a']).unwrap();
        std::fs::write(root.join("big.txt"), vec![b'a'; 3 * 1024 * 1024]).unwrap();
        let l = |n: &str| load(&root, Path::new(n)).unwrap();
        assert_eq!(l("a.rs"), Loaded::Text("fn main() {}\n".into()));
        assert_eq!(l("logo.PNG"), Loaded::Image);
        assert_eq!(l("doc.pdf"), Loaded::Pdf);
        for n in ["bin.dat", "latin.txt", "big.txt"] {
            assert!(matches!(l(n), Loaded::Unsupported(_)), "{n}");
        }
        assert!(load(&root, Path::new("../x")).is_err());
        assert!(load(&root, Path::new("nada.txt")).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn open_and_save_do_not_follow_a_link_out_of_the_project() {
        let root = tmp("openlink");
        let outside = tmp("openlink-outside");
        std::fs::write(outside.join("clave.txt"), "secreto").unwrap();
        std::os::unix::fs::symlink(outside.join("clave.txt"), root.join("atajo.txt")).unwrap();
        assert!(load(&root, Path::new("atajo.txt")).is_err());
        assert!(save(&root, Path::new("atajo.txt"), "x").is_err());
        assert!(diff(&root, Path::new("atajo.txt")).is_err());
        assert_eq!(
            std::fs::read_to_string(outside.join("clave.txt")).unwrap(),
            "secreto"
        );
    }

    #[test]
    fn a_name_with_glob_characters_is_taken_literally() {
        let root = tmp("glob");
        let sh = |c: &str| {
            assert!(
                std::process::Command::new("sh")
                    .args(["-c", c])
                    .current_dir(&root)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        sh("git init -q -b main && git config user.email t@t && git config user.name t");
        std::fs::write(root.join("a[1].txt"), "uno\n").unwrap();
        std::fs::write(root.join("a1.txt"), "otro\n").unwrap();
        sh("git add -A && git commit -q -m x");
        std::fs::write(root.join("a[1].txt"), "dos\n").unwrap();
        let d = diff(&root, Path::new("a[1].txt")).unwrap();
        assert!(
            d.contains("-uno") && d.contains("+dos") && !d.contains("otro"),
            "{d}"
        );
    }

    #[test]
    fn save_writes_the_file_and_refuses_to_leave_the_project() {
        let root = tmp("save");
        std::fs::write(root.join("a.txt"), "uno").unwrap();
        save(&root, Path::new("a.txt"), "dos").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "dos");
        assert!(save(&root, Path::new("../a.txt"), "x").is_err());
        assert!(save(&root, Path::new("/tmp/a.txt"), "x").is_err());
    }

    #[test]
    fn diff_shows_edits_and_new_files() {
        let root = tmp("diff");
        let sh = |c: &str| {
            assert!(
                std::process::Command::new("sh")
                    .args(["-c", c])
                    .current_dir(&root)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        sh("git init -q -b main && git config user.email t@t && git config user.name t");
        std::fs::write(root.join("a.txt"), "uno\n").unwrap();
        sh("git add -A && git commit -q -m uno");
        std::fs::write(root.join("a.txt"), "dos\n").unwrap();
        std::fs::write(root.join("nuevo.txt"), "hola\nadios\n").unwrap();
        let edited = diff(&root, Path::new("a.txt")).unwrap();
        assert!(
            edited.contains("-uno") && edited.contains("+dos"),
            "{edited}"
        );
        let new = diff(&root, Path::new("nuevo.txt")).unwrap();
        assert!(new.contains("+hola") && new.contains("+adios"), "{new}");
        assert_eq!(
            diff(&root, Path::new("a.txt"))
                .unwrap()
                .matches("+dos")
                .count(),
            1
        );
    }
}
