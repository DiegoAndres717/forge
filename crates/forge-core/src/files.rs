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

/// Crea un archivo vacío o una carpeta llamada `name` dentro de `root/parent`. Devuelve su
/// ruta relativa al proyecto.
pub fn create(root: &Path, parent: &Path, name: &str, is_dir: bool) -> Result<String, String> {
    let name = valid_name(name)?;
    let at_root = parent.components().all(|c| matches!(c, Component::CurDir));
    if at_root && name.eq_ignore_ascii_case(".git") {
        return Err("no se crea .git desde aquí".into());
    }
    let dir = safe_path(root, parent)?;
    let first = parent
        .components()
        .find(|c| matches!(c, Component::Normal(_)));
    if first.is_some_and(|c| c.as_os_str().eq_ignore_ascii_case(".git")) {
        return Err("no se crea nada dentro de .git desde aquí".into());
    }
    if !dir.is_dir() {
        return Err(format!("no es una carpeta: {}", parent.display()));
    }
    let path = dir.join(name);
    // `create_new` y `create_dir` fallan si ya existe: nunca pisan nada.
    let made = if is_dir {
        std::fs::create_dir(&path)
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map(|_| ())
    };
    made.map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("ya existe «{name}»"),
        _ => e.to_string(),
    })?;
    Ok(join_rel(parent, name))
}

/// El nombre sin espacios sobrantes, o error si no sirve para un archivo o carpeta.
fn valid_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(format!("nombre no válido: «{name}»"));
    }
    Ok(name)
}

/// `parent/name` siempre con «/», como el resto del árbol.
fn join_rel(parent: &Path, name: &str) -> String {
    let parent = parent.to_string_lossy().replace('\\', "/");
    let parent = parent.trim_matches('/');
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// Renombra `root/rel` a `new_name` dentro de su misma carpeta. Devuelve la ruta nueva.
pub fn rename(root: &Path, rel: &Path, new_name: &str) -> Result<String, String> {
    let new_name = valid_name(new_name)?;
    let first = rel.components().find(|c| matches!(c, Component::Normal(_)));
    let Some(first) = first else {
        return Err("no se renombra la carpeta del proyecto".into());
    };
    let parent = rel.parent().unwrap_or(Path::new(""));
    let at_root = parent.components().all(|c| matches!(c, Component::CurDir));
    if first.as_os_str().eq_ignore_ascii_case(".git")
        || (at_root && new_name.eq_ignore_ascii_case(".git"))
    {
        return Err("no se toca .git desde aquí".into());
    }
    let from = safe_path(root, rel)?;
    // Sin seguir enlaces: renombrar un enlace cambia el enlace, no su destino.
    if std::fs::symlink_metadata(&from).is_err() {
        return Err(format!("no existe: {}", rel.display()));
    }
    let to = from.with_file_name(new_name);
    if to == from {
        return Err("el nombre es el mismo".into());
    }
    // En un sistema que no distingue mayúsculas, «a» y «A» son lo mismo: eso sí se deja, pero
    // solo si el nombre cambia únicamente en las mayúsculas (nunca por un enlace al mismo sitio).
    let only_case = from
        .file_name()
        .is_some_and(|n| n.to_string_lossy().to_lowercase() == new_name.to_lowercase())
        && matches!((from.canonicalize(), to.canonicalize()), (Ok(a), Ok(b)) if a == b);
    // `symlink_metadata`: un enlace roto en el destino también cuenta como ocupado.
    if std::fs::symlink_metadata(&to).is_ok() && !only_case {
        return Err(format!("ya existe «{new_name}»"));
    }
    std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
    Ok(join_rel(parent, new_name))
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

/// Qué le pasó a una línea del archivo respecto al último commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Added,
    Modified,
    /// Se quitaron líneas justo después de esta.
    Deleted,
}

/// Marca del margen: línea (desde 1) del texto actual y lo que había antes en su lugar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GutterMark {
    pub line: usize,
    pub kind: MarkKind,
    pub removed: Vec<String>,
}

/// Marcas del margen del texto que se está escribiendo (aún sin guardar) frente al último
/// commit. Sin Git, sin commit o archivo sin seguimiento: ninguna.
pub fn live_gutter_marks(root: &Path, rel: &Path, content: &str) -> Vec<GutterMark> {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
    };
    // «./» hace la ruta relativa a la carpeta del proyecto, no a la raíz del repo.
    let spec = format!("HEAD:./{}", rel.to_string_lossy().replace('\\', "/"));
    let Some(head) = run(&["show", &spec]).ok().filter(|o| o.status.success()) else {
        return Vec::new();
    };
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    rel.hash(&mut hasher);
    let dir = std::env::temp_dir().join(format!(
        "forge-gutter-{}-{:x}",
        std::process::id(),
        hasher.finish()
    ));
    if std::fs::create_dir_all(&dir).is_err() {
        return Vec::new();
    }
    let (old, new) = (dir.join("antes"), dir.join("ahora"));
    let wrote = std::fs::write(&old, &head.stdout).and_then(|_| std::fs::write(&new, content));
    // `--no-index` sale con 1 si hay diferencias: solo importa lo que escribe.
    let out = wrote.ok().and_then(|_| {
        run(&[
            "diff",
            "--no-index",
            "--no-color",
            "--",
            &old.to_string_lossy(),
            &new.to_string_lossy(),
        ])
        .ok()
    });
    let _ = std::fs::remove_dir_all(&dir);
    out.map(|o| gutter_marks(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Marcas del margen sacadas de un diff unificado.
pub fn gutter_marks(diff: &str) -> Vec<GutterMark> {
    // Un archivo que Git no sigue sale entero como añadido: no hay nada que marcar.
    if diff.starts_with("--- /dev/null") {
        return Vec::new();
    }
    let mut marks = Vec::new();
    let mut in_hunk = false; // las cabeceras «---»/«+++» solo existen antes del primer bloque
    let mut next = 1usize; // próxima línea del texto actual
    let mut removed: Vec<String> = Vec::new();
    let mut added = 0usize;
    // Cierra el bloque de cambios que se estaba leyendo.
    let flush =
        |marks: &mut Vec<GutterMark>, next: usize, removed: &mut Vec<String>, added: &mut usize| {
            let first = next - *added;
            if *added == 0 {
                if !removed.is_empty() {
                    marks.push(GutterMark {
                        line: next.saturating_sub(1).max(1),
                        kind: MarkKind::Deleted,
                        removed: std::mem::take(removed),
                    });
                }
            } else {
                for k in 0..*added {
                    let modified = k == 0 && !removed.is_empty();
                    marks.push(GutterMark {
                        line: first + k,
                        kind: if modified {
                            MarkKind::Modified
                        } else {
                            MarkKind::Added
                        },
                        removed: if modified {
                            std::mem::take(removed)
                        } else {
                            Vec::new()
                        },
                    });
                }
            }
            removed.clear();
            *added = 0;
        };
    for line in diff.lines() {
        if let Some(h) = line.strip_prefix("@@") {
            in_hunk = true;
            flush(&mut marks, next, &mut removed, &mut added);
            // «@@ -a,b +c,d @@»: c es la primera línea del bloque en el texto actual.
            next = h
                .split('+')
                .nth(1)
                .and_then(|r| r.split([',', ' ']).next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
        } else if (!in_hunk && (line.starts_with("---") || line.starts_with("+++")))
            || line.starts_with('\\')
        {
            continue; // cabeceras y «\ No newline at end of file»
        } else if let Some(text) = line.strip_prefix('-') {
            if added > 0 {
                flush(&mut marks, next, &mut removed, &mut added);
            }
            removed.push(text.to_string());
        } else if line.starts_with('+') {
            added += 1;
            next += 1;
        } else {
            flush(&mut marks, next, &mut removed, &mut added);
            next += 1;
        }
    }
    flush(&mut marks, next, &mut removed, &mut added);
    marks
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
    fn the_gutter_marks_added_modified_and_deleted_lines() {
        let diff = "--- a/f\n+++ b/f\n@@ -1,6 +1,7 @@\n keep\n-old\n+new\n+extra\n keep2\n-gone\n keep3\n+tail\n";
        let marks = gutter_marks(diff);
        assert_eq!(
            marks,
            vec![
                GutterMark {
                    line: 2,
                    kind: MarkKind::Modified,
                    removed: vec!["old".into()]
                },
                GutterMark {
                    line: 3,
                    kind: MarkKind::Added,
                    removed: vec![]
                },
                GutterMark {
                    line: 4,
                    kind: MarkKind::Deleted,
                    removed: vec!["gone".into()]
                },
                GutterMark {
                    line: 6,
                    kind: MarkKind::Added,
                    removed: vec![]
                },
            ]
        );
    }

    #[test]
    fn gutter_marks_do_not_mistake_content_for_diff_headers() {
        // «++i;» añadida sale como «+++i;» y «-- nota» quitada como «--- nota».
        let diff = "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n--- nota\n+++i;\n b\n";
        let marks = gutter_marks(diff);
        assert_eq!(
            marks,
            vec![GutterMark {
                line: 2,
                kind: MarkKind::Modified,
                removed: vec!["-- nota".into()]
            }]
        );
        // Quitar las primeras líneas deja la marca en la línea 1, no en la 0.
        let top = "--- a/f\n+++ b/f\n@@ -1,2 +1,1 @@\n-x\n y\n";
        assert_eq!(gutter_marks(top)[0].line, 1);
    }

    #[test]
    fn live_gutter_marks_follow_the_unsaved_text() {
        let root = tmp("live-gutter");
        let sh = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .output()
                    .unwrap()
                    .status
                    .success()
            )
        };
        sh(&["init", "-q"]);
        std::fs::write(root.join("f.txt"), "uno\ndos\ntres\n").unwrap();
        sh(&["add", "f.txt"]);
        sh(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "x",
        ]);
        // Sin tocar el archivo en disco: solo el texto que se está escribiendo.
        let marks = live_gutter_marks(&root, Path::new("f.txt"), "uno\nDOS\ntres\ncuatro\n");
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].kind, MarkKind::Modified);
        assert_eq!(marks[0].removed, vec!["dos".to_string()]);
        assert_eq!(marks[1].line, 4);
        assert_eq!(marks[1].kind, MarkKind::Added);
        // Texto igual al del commit: sin marcas. Archivo sin seguimiento: tampoco.
        assert!(live_gutter_marks(&root, Path::new("f.txt"), "uno\ndos\ntres\n").is_empty());
        assert!(live_gutter_marks(&root, Path::new("nuevo.txt"), "a\nb\n").is_empty());
    }

    #[test]
    fn a_file_git_does_not_track_has_no_gutter_marks() {
        let diff = "--- /dev/null\n+++ b/f\n+uno\n+dos\n";
        assert!(gutter_marks(diff).is_empty());
    }

    #[test]
    fn create_makes_files_and_folders_and_refuses_unsafe_names() {
        let root = tmp("create");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        // Archivo en la raíz y en una carpeta anidada; carpeta nueva.
        assert_eq!(
            create(&root, Path::new(""), "a.txt", false).unwrap(),
            "a.txt"
        );
        assert!(root.join("a.txt").is_file());
        assert_eq!(
            create(&root, Path::new("src/deep"), "mod.rs", false).unwrap(),
            "src/deep/mod.rs"
        );
        assert_eq!(
            create(&root, Path::new("src"), "nueva", true).unwrap(),
            "src/nueva"
        );
        assert!(root.join("src/nueva").is_dir());
        // No pisa lo que ya existe (ni su contenido).
        std::fs::write(root.join("a.txt"), "hola").unwrap();
        assert!(create(&root, Path::new(""), "a.txt", false).is_err());
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "hola");
        assert!(create(&root, Path::new("src"), "nueva", true).is_err());
        // Nombres que se salen o no valen.
        for bad in ["", "  ", "..", ".", "a/b", "a\\b", "../fuera"] {
            assert!(create(&root, Path::new(""), bad, false).is_err(), "{bad:?}");
        }
        // Ni fuera del proyecto, ni dentro de .git, ni en una carpeta que no existe.
        assert!(create(&root, Path::new(".."), "x", false).is_err());
        assert!(create(&root, Path::new(""), ".git", true).is_err());
        assert!(create(&root, Path::new(""), ".GIT", true).is_err());
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(create(&root, Path::new(".git"), "x", false).is_err());
        assert!(create(&root, Path::new("no-existe"), "x", false).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_moves_files_and_folders_within_their_folder_and_refuses_collisions() {
        let root = tmp("rename");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("src/deep/a.txt"), "uno").unwrap();
        std::fs::write(root.join("b.txt"), "dos").unwrap();
        // Un archivo anidado conserva su carpeta y su contenido.
        assert_eq!(
            rename(&root, Path::new("src/deep/a.txt"), "c.rs").unwrap(),
            "src/deep/c.rs"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/deep/c.rs")).unwrap(),
            "uno"
        );
        assert!(!root.join("src/deep/a.txt").exists());
        // Una carpeta lleva su contenido.
        assert_eq!(
            rename(&root, Path::new("src/deep"), "hondo").unwrap(),
            "src/hondo"
        );
        assert!(root.join("src/hondo/c.rs").is_file());
        // Cambiar solo mayúsculas vale aunque el sistema no distinga.
        assert_eq!(rename(&root, Path::new("b.txt"), "B.txt").unwrap(), "B.txt");
        // Nada de pisar otro elemento, ni nombre igual, ni nombres o rutas peligrosos.
        std::fs::write(root.join("x.txt"), "x").unwrap();
        assert!(rename(&root, Path::new("x.txt"), "src").is_err());
        assert!(rename(&root, Path::new("x.txt"), "B.txt").is_err());
        assert_eq!(std::fs::read_to_string(root.join("B.txt")).unwrap(), "dos");
        assert!(rename(&root, Path::new("x.txt"), "x.txt").is_err());
        for bad in ["", "..", "a/b", "../x", "a\\b"] {
            assert!(rename(&root, Path::new("x.txt"), bad).is_err(), "{bad:?}");
        }
        assert!(rename(&root, Path::new("../fuera"), "y").is_err());
        assert!(rename(&root, Path::new(""), "y").is_err(), "no el proyecto");
        assert!(rename(&root, Path::new("no-existe"), "y").is_err());
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(rename(&root, Path::new(".git"), "otro").is_err());
        assert!(rename(&root, Path::new("x.txt"), ".git").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn rename_never_overwrites_through_a_symlink() {
        let root = tmp("rename-link");
        std::fs::write(root.join("real"), "datos").unwrap();
        std::os::unix::fs::symlink("real", root.join("ln")).unwrap();
        std::os::unix::fs::symlink("no-existe", root.join("roto")).unwrap();
        std::fs::write(root.join("otro"), "o").unwrap();
        // Ni el enlace pisa su destino, ni el destino pisa al enlace, ni un enlace roto.
        assert!(rename(&root, Path::new("ln"), "real").is_err());
        assert!(rename(&root, Path::new("real"), "ln").is_err());
        assert!(rename(&root, Path::new("otro"), "roto").is_err());
        assert_eq!(std::fs::read_to_string(root.join("real")).unwrap(), "datos");
        assert!(std::fs::symlink_metadata(root.join("ln")).is_ok());
        assert!(std::fs::symlink_metadata(root.join("roto")).is_ok());
        let _ = std::fs::remove_dir_all(&root);
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
