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
                Loaded::Applied(r) => r.map(|p| self.applied = Some(p)).err(),
            };
            if error.is_some() {
                self.error = error;
            }
        }
    }
}

impl App {
    /// Fila «Worktrees» y, abierta, la lista y la limpieza (debajo de Planes).
    pub(super) fn worktrees_ui(&mut self, ui: &mut egui::Ui, active: usize) {
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
                ..Default::default()
            };
        }
        view.poll();
        let others = view
            .list
            .as_ref()
            .map(|l| l.iter().filter(|w| !w.main).count().to_string())
            .unwrap_or_default();
        let row = theme::row(
            ui,
            icon::TREE_STRUCTURE,
            theme::TEXT_3,
            tr!("Worktrees"),
            &others,
            view.open,
        )
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
        if !view.open {
            return;
        }
        let small = |ui: &mut egui::Ui, text: String, color| {
            ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).wrap());
        };
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
            let (repo, in_use) = (project.clone(), in_use());
            view.applied = None;
            view.spawn(ui.ctx(), move |send| {
                let in_use = [in_use, worktrees::process_cwds()].concat();
                send(Loaded::Preview(worktrees::plan(
                    &repo,
                    std::slice::from_ref(&repo),
                    &in_use,
                )));
            });
        }
        if let Some(plan) = &view.preview {
            for c in plan.iter().filter(|c| !c.worktree.main) {
                let color = if acts(c) {
                    theme::TEXT_2
                } else {
                    theme::TEXT_4
                };
                small(
                    ui,
                    format!("{}: {}", c.worktree.label(), c.describe()),
                    color,
                );
            }
            let approved: Vec<Cleanup> = plan.iter().filter(|c| acts(c)).cloned().collect();
            if approved.is_empty() {
                small(ui, tr!("No hay nada que limpiar").into(), theme::TEXT_3);
            } else if view.loading.is_none()
                && theme::secondary(ui, tr!("Aplicar limpieza")).clicked()
            {
                view.preview = None;
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
