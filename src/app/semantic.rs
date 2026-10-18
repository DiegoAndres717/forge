// Búsqueda por significado de Forge Memory, del lado de la app: descargar o quitar el modelo
// (Ajustes) y calcular en segundo plano los vectores de las memorias nuevas o cambiadas.
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use forge_core::embed;
use forge_core::store::Store;

/// Cada cuánto se miran las memorias sin vector (la consulta es barata; el cálculo no).
const SWEEP_EVERY: Duration = Duration::from_secs(20);

/// Tras un fallo (modelo roto, una memoria que no se deja), se espera más antes de reintentar:
/// cargar el modelo cuesta ≈0,6 s y ≈450 MB.
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(600);

/// Vectores calculados: (id de la memoria, vector), o por qué no se pudo.
type Vectors = Result<Vec<(i64, Vec<f32>)>, String>;

#[derive(Default)]
pub struct Semantic {
    /// Carpeta del modelo; `None` si no hay dónde (o en los tests, salvo que la fijen).
    pub dir: Option<PathBuf>,
    download: Option<JoinHandle<Result<(), String>>>,
    pub error: Option<String>,
    sweep: Option<Receiver<Vectors>>,
    last_sweep: Option<Instant>,
    /// El último cálculo falló: el siguiente espera `RETRY_AFTER_ERROR`.
    failed: bool,
}

impl Semantic {
    pub fn new() -> Self {
        Self {
            // Los tests no tocan la carpeta real de modelos.
            dir: if cfg!(test) {
                None
            } else {
                embed::default_dir()
            },
            ..Default::default()
        }
    }

    pub fn installed(&self) -> bool {
        self.dir.as_deref().is_some_and(embed::installed)
    }

    /// Fracción descargada mientras se descarga.
    pub fn downloading(&self) -> Option<f32> {
        let dir = self.dir.as_deref()?;
        self.download.as_ref()?;
        let (done, total) = embed::progress(dir);
        Some(done as f32 / total.max(1) as f32)
    }

    pub fn install(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.dir.clone() else { return };
        if self.download.is_some() {
            return;
        }
        self.error = None;
        let ctx = ctx.clone();
        self.download = Some(std::thread::spawn(move || {
            let result = embed::install(&dir);
            ctx.request_repaint();
            result
        }));
    }

    pub fn remove(&mut self) {
        if let Some(dir) = &self.dir {
            self.error = embed::remove(dir).err();
        }
    }

    /// Cada fotograma: recoge la descarga, guarda los vectores calculados y lanza el
    /// siguiente cálculo cuando toca.
    pub fn tick(&mut self, store: &Store, ctx: &egui::Context) {
        if self.download.as_ref().is_some_and(|h| h.is_finished())
            && let Some(handle) = self.download.take()
        {
            self.error = match handle.join() {
                Ok(result) => result.err(),
                Err(_) => Some(forge_core::tr!("la descarga se interrumpió").into()),
            };
            self.last_sweep = None;
        }
        if self.download.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if let Some(rx) = &self.sweep {
            match rx.try_recv() {
                Ok(result) => {
                    self.failed = result.is_err();
                    match result {
                        Ok(vectors) => {
                            for (id, v) in vectors {
                                let _ = store.set_memory_vector(id, embed::MODEL, &v);
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                    self.sweep = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.sweep = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            return;
        }
        if !self.installed() {
            return;
        }
        // Sin esto, una memoria nueva esperaría al próximo movimiento del ratón.
        ctx.request_repaint_after(SWEEP_EVERY);
        let every = if self.failed {
            RETRY_AFTER_ERROR
        } else {
            SWEEP_EVERY
        };
        if self.last_sweep.is_some_and(|t| t.elapsed() < every) {
            return;
        }
        self.last_sweep = Some(Instant::now());
        let pending = store.pending_vectors(embed::MODEL, 200).unwrap_or_default();
        let Some(dir) = self.dir.clone().filter(|_| !pending.is_empty()) else {
            return;
        };
        // El cálculo en otro hilo (≈0,15 s por memoria); la base se toca solo aquí.
        let (tx, rx) = channel();
        self.sweep = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let vectors: Vectors = embed::Embedder::load(&dir).and_then(|embedder| {
                pending
                    .iter()
                    .map(|m| Ok((m.id, embedder.embed(&embed::memory_text(m))?)))
                    .collect()
            });
            let _ = tx.send(vectors);
            ctx.request_repaint();
        });
    }
}
