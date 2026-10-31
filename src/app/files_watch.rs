// Observador del proyecto: avisa cuando algo cambia en disco (un agente escribe, un commit…)
// para que el árbol y el editor se actualicen al instante, sin preguntar cada poco tiempo.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ::notify::{RecursiveMode, Watcher};
use eframe::egui;

/// Carpetas que cambian sin parar y no interesan al explorador.
const NOISY: [&str; 10] = [
    "node_modules",
    "target",
    "dist",
    ".next",
    ".turbo",
    ".cache",
    ".venv",
    "build",
    "__pycache__",
    ".idea",
];

/// Tope de rutas distintas que se recuerdan entre dos lecturas: pasado esto basta con «algo
/// cambió» y se recarga todo.
const MAX_PATHS: usize = 2000;

/// Qué tocó un evento que sí importa.
#[derive(Debug, PartialEq, Eq)]
enum Touch {
    /// Un archivo o carpeta del proyecto.
    File,
    /// El estado de Git (commit, rama, índice).
    Git,
}

/// Si un cambio en `path` importa, y a qué. `None` = ruido (dependencias, compilación, objetos
/// de Git, archivos de bloqueo).
fn touch(root: &Path, path: &Path) -> Option<Touch> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<_> = rel.components().map(|c| c.as_os_str()).collect();
    if parts.first().is_some_and(|p| *p == ".git") {
        // Del interior de Git solo importa lo que cambia el estado; sus bloqueos, no.
        if parts
            .last()
            .is_some_and(|n| n.to_string_lossy().ends_with(".lock"))
        {
            return None;
        }
        return match parts.get(1).map(|p| p.to_string_lossy()) {
            Some(p) if p == "HEAD" || p == "index" || p == "refs" || p == "packed-refs" => {
                Some(Touch::Git)
            }
            _ => None,
        };
    }
    if parts
        .iter()
        .any(|p| NOISY.iter().any(|n| p.to_string_lossy() == *n))
    {
        return None;
    }
    Some(Touch::File)
}

/// Lo que el observador ha visto desde la última lectura.
#[derive(Default)]
struct Seen {
    paths: HashSet<PathBuf>,
    /// Pasó de `MAX_PATHS`: no se sabe cuáles, se recarga todo.
    overflow: bool,
    git: bool,
    /// Primer y último evento de la ráfaga que se está leyendo.
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Seen {
    /// Se lleva lo visto si la ráfaga ya se calmó (`QUIET` sin eventos nuevos); `None` si no hay
    /// nada o aún llegan más.
    fn drain(&mut self) -> Option<Seen> {
        let last = self.last?;
        // Algo que escribe sin parar (un log, un servidor) no puede dejar la vista sin
        // actualizar: pasado `MAX_WAIT` se lee igualmente.
        let long = self.first.is_some_and(|f| f.elapsed() >= MAX_WAIT);
        if last.elapsed() < QUIET && !long {
            return None;
        }
        Some(std::mem::take(self))
    }
}

/// Lo que cambió, ya junto: una ráfaga de eventos llega como una sola cosa.
#[derive(Default)]
pub struct Changes {
    pub paths: HashSet<PathBuf>,
    pub everything: bool,
    pub git: bool,
}

impl Changes {
    /// ¿Afecta a este archivo (ruta absoluta)?
    pub fn touches(&self, file: &Path) -> bool {
        self.everything || self.git || self.paths.contains(file)
    }
}

/// El observador de una carpeta de proyecto (recursivo); se para al soltarlo.
pub struct ProjectWatcher {
    root: PathBuf,
    _watcher: ::notify::RecommendedWatcher,
    seen: Arc<Mutex<Seen>>,
}

impl ProjectWatcher {
    /// Empieza a observar `root`; cada cambio despierta la ventana.
    pub fn start(root: &Path, ctx: &egui::Context) -> Option<Self> {
        let seen = Arc::new(Mutex::new(Seen::default()));
        // El sistema da rutas reales (sin enlaces): se compara con la ruta real del proyecto.
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let (shared, repaint, base) = (Arc::clone(&seen), ctx.clone(), root.clone());
        let mut watcher =
            ::notify::recommended_watcher(move |res: ::notify::Result<::notify::Event>| {
                let Ok(event) = res else { return };
                // Leer un archivo también «toca» su acceso: solo importan los cambios de verdad.
                if event.kind.is_access() {
                    return;
                }
                let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                let mut any = false;
                for path in &event.paths {
                    match touch(&base, path) {
                        Some(Touch::Git) => {
                            s.git = true;
                            any = true;
                        }
                        Some(Touch::File) => {
                            any = true;
                            if s.paths.len() >= MAX_PATHS {
                                s.overflow = true;
                            } else {
                                s.paths.insert(path.clone());
                            }
                        }
                        None => {}
                    }
                }
                if any {
                    let now = Instant::now();
                    s.first.get_or_insert(now);
                    s.last = Some(now);
                    drop(s);
                    // Un instante después de la ráfaga, para leerla junta.
                    repaint.request_repaint_after(QUIET);
                }
            })
            .ok()?;
        watcher.watch(&root, RecursiveMode::Recursive).ok()?;
        Some(Self {
            root,
            _watcher: watcher,
            seen,
        })
    }

    /// La carpeta observada (ruta real).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Lo que cambió, si la ráfaga ya se calmó (`QUIET` sin eventos nuevos). `None` si no hay
    /// nada o aún llegan más.
    pub fn take(&self) -> Option<Changes> {
        let mut s = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let seen = s.drain()?;
        Some(Changes {
            paths: seen.paths,
            everything: seen.overflow,
            git: seen.git,
        })
    }
}

/// Espera sin eventos nuevos antes de dar por terminada una ráfaga de cambios.
pub const QUIET: Duration = Duration::from_millis(150);

/// Lo máximo que una ráfaga sin pausa retrasa la actualización.
const MAX_WAIT: Duration = Duration::from_secs(1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_project_changes_wake_the_explorer() {
        let root = Path::new("/p");
        let t = |rel: &str| touch(root, &root.join(rel));
        assert_eq!(t("src/main.rs"), Some(Touch::File));
        assert_eq!(t("README.md"), Some(Touch::File));
        // Dependencias y compilación: ruido, a cualquier profundidad.
        assert_eq!(t("node_modules/a/index.js"), None);
        assert_eq!(t("web/node_modules/a/index.js"), None);
        assert_eq!(t("target/debug/forge"), None);
        // De Git solo importa lo que cambia el estado: commits, ramas, índice.
        assert_eq!(t(".git/HEAD"), Some(Touch::Git));
        assert_eq!(t(".git/index"), Some(Touch::Git));
        assert_eq!(t(".git/refs/heads/main"), Some(Touch::Git));
        assert_eq!(t(".git/objects/ab/cdef"), None);
        assert_eq!(t(".git/index.lock"), None);
        // Los archivos de dependencias fuera de .git sí son del proyecto.
        assert_eq!(t("Cargo.lock"), Some(Touch::File));
        assert_eq!(t("web/yarn.lock"), Some(Touch::File));
        assert_eq!(t(".git/packed-refs"), Some(Touch::Git));
        assert_eq!(t(".venv/lib/x.py"), None);
        assert_eq!(t("src/__pycache__/a.pyc"), None);
        // Fuera del proyecto: nada.
        assert_eq!(touch(root, Path::new("/otro/a.rs")), None);
    }

    #[test]
    fn a_burst_of_events_is_read_once_after_it_calms_down() {
        let mut seen = Seen::default();
        assert!(seen.drain().is_none(), "sin eventos no hay nada");
        seen.paths.insert(PathBuf::from("/p/a.rs"));
        seen.paths.insert(PathBuf::from("/p/b.rs"));
        seen.last = Some(Instant::now());
        assert!(seen.drain().is_none(), "aún llegan eventos");
        std::thread::sleep(QUIET + Duration::from_millis(30));
        assert_eq!(seen.drain().unwrap().paths.len(), 2);
        assert!(seen.drain().is_none(), "ya se leyó");
    }

    #[test]
    fn a_steady_stream_of_writes_does_not_block_the_refresh_forever() {
        let mut seen = Seen::default();
        // Un servidor escribiendo sin parar: cada evento es reciente, pero la ráfaga ya es larga.
        seen.paths.insert(PathBuf::from("/p/log.txt"));
        seen.first = Some(Instant::now() - MAX_WAIT - Duration::from_millis(10));
        seen.last = Some(Instant::now());
        assert!(seen.drain().is_some(), "pasado el tope se lee igualmente");
        // Una ráfaga corta y aún viva sí espera.
        seen.paths.insert(PathBuf::from("/p/a.rs"));
        seen.first = Some(Instant::now());
        seen.last = Some(Instant::now());
        assert!(seen.drain().is_none());
    }
}
