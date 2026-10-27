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
    let path = safe_path(root, rel)?;
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

/// Guarda `content` en `root/rel` (el archivo existe o su carpeta existe).
pub fn save(root: &Path, rel: &Path, content: &str) -> Result<(), String> {
    let path = safe_path(root, rel)?;
    if path.is_dir() {
        return Err("es una carpeta".into());
    }
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

/// Lo que ha cambiado `root/rel` respecto al último commit, en formato de diff. Un archivo
/// nuevo sale entero como añadido.
pub fn diff(root: &Path, rel: &Path) -> Result<String, String> {
    let path = safe_path(root, rel)?;
    crate::git::file_diff(root, &rel.to_string_lossy(), &path)
}

/// Imagen de la primera página de un PDF, generada con la vista rápida de macOS.
#[cfg(target_os = "macos")]
pub fn pdf_preview(root: &Path, rel: &Path) -> Result<PathBuf, String> {
    let path = safe_path(root, rel)?;
    let dir = std::env::temp_dir().join(format!("forge-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let status = std::process::Command::new("qlmanage")
        .args(["-t", "-s", "1400", "-o"])
        .arg(&dir)
        .arg(&path)
        .output()
        .map_err(|e| e.to_string())?;
    let png = dir.join(format!(
        "{}.png",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
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
