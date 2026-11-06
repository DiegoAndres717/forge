// Worktrees del proyecto en la barra lateral: la lista con su estado y la limpieza segura.
// Primero la vista previa; «Aplicar» solo hace lo que la vista previa mostró y sigue en pie.
use super::*;
use forge_core::tr;
use forge_core::worktrees::{self, Action, Cleanup, Worktree};
use std::sync::mpsc::{Receiver, channel};

pub(super) enum Loaded {
    List(Result<Vec<Worktree>, String>),
    Preview(Result<Vec<Cleanup>, String>),
    Applied(Result<Vec<Cleanup>, String>),
}

#[derive(Default)]
pub(super) struct WorktreesView {
    pub(super) open: bool,
    project: PathBuf,
    pub(super) list: Option<Vec<Worktree>>,
    pub(super) preview: Option<Vec<Cleanup>>,
    pub(super) applied: Option<Vec<Cleanup>>,
    error: Option<String>,
    pub(super) loading: Option<Receiver<Loaded>>,
    /// Worktrees de `forge worktree new` que Forge propone limpiar (rama fusionada o sin la
    /// terminal que los pidió): solo lo propone; la limpieza pasa por la vista previa.
    pub(super) offer: Vec<Cleanup>,
    offer_rx: Option<Receiver<Vec<Cleanup>>>,
    pub(super) offer_checked: Option<Instant>,
    /// Carpeta de los worktrees de Forge (`None`: la de siempre; las pruebas usan otra).
    pub(super) root: Option<PathBuf>,
}

/// ¿La limpieza hace algo con él?
fn acts(c: &Cleanup) -> bool {
    !matches!(c.action, Action::Keep(_))
}

impl WorktreesView {
    /// Corre `job` en segundo plano; lo que mande llega a `poll`.
    fn spawn(&mut self, ctx: &egui::Context, job: impl FnOnce(&dyn Fn(Loaded)) + Send + 'static) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.loading = Some(rx);
        std::thread::spawn(move || {
            job(&|loaded| {
                let _ = tx.send(loaded);
                ctx.request_repaint();
            })
        });
    }

    fn poll(&mut self) {
        let Some(rx) = self.loading.take() else {
            return;
        };
        loop {
            let loaded = match rx.try_recv() {
                Ok(loaded) => loaded,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.loading = Some(rx);
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            };
            let error = match loaded {
                Loaded::List(r) => r.map(|l| self.list = Some(l)).err(),
                Loaded::Preview(r) => r.map(|p| self.preview = Some(p)).err(),
                Loaded::Applied(r) => {
                    // Lo propuesto se vuelve a mirar ya (lo que se estuviera mirando es de antes).
                    self.offer_rx = None;
                    self.offer_checked = None;
                    r.map(|p| self.applied = Some(p)).err()
                }
            };
            if error.is_some() {
                self.error = error;
            }
        }
    }

    /// La vista previa de la limpieza (y la lista, si aún no está).
    fn start_preview(&mut self, ctx: &egui::Context, repo: PathBuf, in_use: Vec<PathBuf>) {
        self.applied = None;
        let list = self.list.is_none();
        self.spawn(ctx, move |send| {
            if list {
                send(Loaded::List(worktrees::list(&repo)));
            }
            let in_use = [in_use, worktrees::process_cwds()].concat();
            send(Loaded::Preview(worktrees::plan(
                &repo,
                std::slice::from_ref(&repo),
                &in_use,
            )));
        });
    }

    /// Cada minuto, en otro hilo: los registrados que la limpieza ya quitaría y cuya rama
    /// está fusionada o cuya terminal se cerró (`open`: las sesiones abiertas).
    fn check_offer(
        &mut self,
        ctx: &egui::Context,
        repo: PathBuf,
        open: Vec<String>,
        in_use: Vec<PathBuf>,
    ) {
        if let Some(rx) = &self.offer_rx {
            match rx.try_recv() {
                Ok(offer) => self.offer = offer,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
            self.offer_rx = None;
        }
        if self
            .offer_checked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        self.offer_checked = Some(Instant::now());
        let root = self.root.clone().or_else(worktrees::root);
        let (tx, rx) = channel();
        self.offer_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let records = root
                .map(|r| worktrees::records(&repo, &r))
                .unwrap_or_default();
            // Sin registrados no hace falta mirar nada más (ni los procesos).
            let offer = if records.is_empty() {
                Vec::new()
            } else {
                let in_use = [in_use, worktrees::process_cwds()].concat();
                worktrees::plan(&repo, std::slice::from_ref(&repo), &in_use)
                    .map(|plan| worktrees::offers(&plan, &records, &open))
                    .unwrap_or_default()
            };
            let _ = tx.send(offer);
            ctx.request_repaint();
        });
    }
}

impl App {
    /// Fila «Worktrees» y, abierta, la lista y la limpieza (debajo de Planes).
    pub(super) fn worktrees_ui(&mut self, ui: &mut egui::Ui, active: usize) {
        // Plegada o abierta como la dejó el dueño (se recuerda entre sesiones).
        let remembered = self.section_open("worktrees");
        self.worktrees_body(ui, active, remembered);
        if self.worktrees.open != remembered {
            self.set_section_open("worktrees", self.worktrees.open);
        }
    }

    fn worktrees_body(&mut self, ui: &mut egui::Ui, active: usize, remembered: bool) {
        let project = self.workspaces[active].project.path.clone();
        // Carpetas de todas las terminales y proyectos abiertos (también otros worktrees);
        // solo al pedir la limpieza: leer la carpeta de cada terminal en cada fotograma sobra.
        let workspaces = &self.workspaces;
        let in_use =
            || -> Vec<PathBuf> { workspaces.iter().flat_map(|w| w.working_dirs()).collect() };
        let view = &mut self.worktrees;
        if view.project != project {
            *view = WorktreesView {
                project: project.clone(),
                open: remembered,
                ..Default::default()
            };
            if remembered {
                let repo = project.clone();
                view.spawn(ui.ctx(), move |send| {
                    send(Loaded::List(worktrees::list(&repo)))
                });
            }
        }
        view.poll();
        // Las sesiones y carpetas solo cuando toca mirar (no en cada fotograma).
        let due = view
            .offer_checked
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(60));
        if due || view.offer_rx.is_some() {
            let open: Vec<String> = workspaces.iter().flat_map(|w| w.sessions()).collect();
            view.check_offer(ui.ctx(), project.clone(), open, in_use());
        }
        let others = view
            .list
            .as_ref()
            .map(|l| l.iter().filter(|w| !w.main).count().to_string())
            .unwrap_or_default();
        let caret = if view.open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        };
        let row = theme::row(ui, caret, theme::TEXT_3, tr!("Worktrees"), &others, false)
            .on_hover_text(tr!(
                "Las copias de trabajo del repositorio (git worktree): rama, estado y lo que ocupan"
            ));
        if row.clicked() {
            view.open = !view.open;
            if view.open && view.loading.is_none() {
                let repo = project.clone();
                view.spawn(ui.ctx(), move |send| {
                    send(Loaded::List(worktrees::list(&repo)))
                });
            }
        }
        let small = |ui: &mut egui::Ui, text: String, color| {
            ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).wrap())
        };
        // Lo propone, no lo borra: el clic lleva a la vista previa de siempre.
        if !view.offer.is_empty() && view.preview.is_none() && view.loading.is_none() {
            let names: Vec<String> = view.offer.iter().map(|c| c.worktree.label()).collect();
            let text = tr!("Forge propone limpiar: {p}", p = names.join(", "));
            let offer = small(ui, text.to_string(), theme::ORANGE)
                .interact(egui::Sense::click())
                .on_hover_text(tr!(
                    "Worktrees creados con forge worktree new cuya rama ya está fusionada o cuya terminal se cerró. Clic: vista previa de la limpieza (no borra nada)"
                ));
            if offer.clicked() {
                view.open = true;
                view.start_preview(ui.ctx(), project.clone(), in_use());
            }
        }
        if !view.open {
            return;
        }
        if let Some(e) = &view.error {
            small(ui, e.clone(), theme::RED);
        }
        let Some(list) = &view.list else {
            if view.loading.is_some() {
                ui.spinner();
            }
            return;
        };
        for wt in list.iter().filter(|w| !w.main) {
            let dot = match (wt.dirty, wt.merged, wt.prunable) {
                (_, _, true) => theme::TEXT_4,
                (true, _, _) => theme::ORANGE,
                (_, Some(true), _) => theme::GREEN,
                _ => theme::TEXT_3,
            };
            let size = wt.size.map(human_bytes).unwrap_or_default();
            theme::sub_row(ui, dot, &wt.label(), &size).on_hover_text(format!(
                "{}\n{}",
                tilde(&wt.path),
                wt.state()
            ));
        }
        if list.iter().all(|w| w.main) {
            small(ui, tr!("Solo está el principal").into(), theme::TEXT_4);
            return;
        }
        ui.add_space(4.0);
        if view.loading.is_some() {
            ui.spinner();
        } else if theme::secondary(ui, tr!("Vista previa de limpieza")).clicked() {
            view.start_preview(ui.ctx(), project.clone(), in_use());
        }
        // Se cierra sin aplicar: «Cerrar» o Esc con el ratón sobre la barra lateral y sin una
        // terminal con el foco (así Esc no se le quita al agente, al que interrumpe).
        let esc = view.preview.is_some()
            && ui.rect_contains_pointer(ui.clip_rect())
            && ui.ctx().memory(|m| m.focused().is_none())
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if esc {
            view.preview = None;
        }
        if let Some(plan) = &view.preview {
            small(
                ui,
                tr!("Qué haría la limpieza con cada worktree. No toca nada hasta «Aplicar limpieza».")
                    .into(),
                theme::TEXT_3,
            );
            for c in plan.iter().filter(|c| !c.worktree.main) {
                let color = if acts(c) {
                    theme::TEXT_2
                } else {
                    theme::TEXT_4
                };
                small(
                    ui,
                    format!("{}: {}", c.worktree.label(), c.explain()),
                    color,
                )
                .on_hover_text(format!(
                    "{}\n{}",
                    tilde(&c.worktree.path),
                    c.worktree.state()
                ));
            }
            let approved: Vec<Cleanup> = plan.iter().filter(|c| acts(c)).cloned().collect();
            let mut apply = false;
            ui.horizontal_wrapped(|ui| {
                if approved.is_empty() {
                    small(ui, tr!("No hay nada que limpiar").into(), theme::TEXT_3);
                } else if view.loading.is_none() {
                    apply = theme::secondary(ui, tr!("Aplicar limpieza")).clicked();
                }
                if theme::secondary(ui, tr!("Cerrar"))
                    .on_hover_text(tr!("Cerrar la vista previa sin aplicar nada (Esc)"))
                    .clicked()
                {
                    view.preview = None;
                }
            });
            if apply {
                view.preview = None;
                view.offer.clear();
                let (repo, in_use) = (project.clone(), in_use());
                view.spawn(ui.ctx(), move |send| {
                    // Se vuelve a mirar todo: solo se hace lo que la vista previa mostró y
                    // aún vale (un worktree que cambió entretanto se queda).
                    let in_use = [in_use, worktrees::process_cwds()].concat();
                    let applied =
                        worktrees::plan(&repo, std::slice::from_ref(&repo), &in_use).map(|fresh| {
                            let mut plan: Vec<Cleanup> = fresh
                                .into_iter()
                                .filter(|c| {
                                    approved.iter().any(|a| {
                                        a.worktree.path == c.worktree.path && a.action == c.action
                                    })
                                })
                                .collect();
                            worktrees::apply(&repo, &mut plan);
                            plan
                        });
                    send(Loaded::Applied(applied));
                    send(Loaded::List(worktrees::list(&repo)));
                });
            }
        }
        if let Some(applied) = &view.applied {
            for c in applied {
                let (mark, color, error) = match &c.result {
                    Some(Err(e)) => ("✕", theme::RED, format!("\n{e}")),
                    _ => ("✓", theme::GREEN, String::new()),
                };
                let text = format!("{mark} {}: {}{error}", c.worktree.label(), c.describe());
                small(ui, text, color);
            }
            if applied.is_empty() {
                small(
                    ui,
                    tr!("Nada aplicado: cambiaron desde la vista previa").into(),
                    theme::TEXT_3,
                );
            }
        }
    }
}
