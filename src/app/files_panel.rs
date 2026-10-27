// Panel de archivos: las carpetas y archivos del proyecto como árbol, con lo que el agente
// ha creado o cambiado marcado, y borrado a la papelera del sistema (recuperable).
use super::*;
use forge_core::files::Entry;
use std::collections::HashSet;

/// Lo que el hilo de refresco devuelve: el contenido de cada carpeta abierta y los cambios de Git.
pub(crate) type FilesData = (HashMap<String, Vec<Entry>>, HashMap<String, char>);

/// Ajuste donde se guarda «no volver a preguntar» antes de mandar a la papelera.
pub(super) const TRASH_NO_ASK: &str = "files-trash-no-ask";

/// Árbol de archivos (se abre desde la barra superior).
#[derive(Default)]
pub struct FilesView {
    pub open: bool,
    /// Carpetas abiertas, con su ruta relativa al proyecto («» es la raíz).
    pub expanded: HashSet<String>,
    pub listing: HashMap<String, Vec<Entry>>,
    /// Estado de Git de cada archivo con cambios (`M`, `A`, `D`, `?`…).
    pub changes: HashMap<String, char>,
    pub error: Option<String>,
    /// Ruta que espera confirmación para ir a la papelera.
    pub confirm_trash: Option<String>,
    /// Casilla «no volver a preguntar» del aviso abierto.
    pub no_ask: bool,
    pub loading: Option<Receiver<FilesData>>,
    pub loaded: Option<Instant>,
    pub dirty: bool,
}

/// Letra y color del estado de un archivo (`git status`).
fn badge(code: char) -> (char, Color32) {
    match code {
        'M' => ('M', theme::YELLOW),
        'A' => ('A', theme::GREEN),
        'D' => ('D', theme::RED),
        'R' | 'C' => ('R', theme::PURPLE),
        '?' => ('U', theme::GREEN),
        'U' => ('!', theme::RED),
        c => (c, theme::TEXT_3),
    }
}

/// Estado que se muestra de un archivo: el del árbol de trabajo y, si no hay, el del índice.
fn change_code(f: &forge_core::git::FileChange) -> char {
    if f.unstaged != ' ' {
        f.unstaged
    } else {
        f.staged
    }
}

/// Une una carpeta y un nombre en una ruta relativa («» es la raíz).
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

impl App {
    /// Relee el árbol del proyecto activo cada pocos segundos (en otro hilo).
    pub(super) fn files_tick(&mut self, ctx: &egui::Context) {
        let Some(i) = self.active else { return };
        let files = &mut self.workspaces[i].files;
        if let Some(rx) = &files.loading
            && let Ok((listing, changes)) = rx.try_recv()
        {
            files.loading = None;
            files.loaded = Some(Instant::now());
            files.listing = listing;
            files.changes = changes;
        }
        let stale = files.dirty
            || files
                .loaded
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3));
        if !files.open || files.loading.is_some() || !stale {
            return;
        }
        files.dirty = false;
        let mut dirs: Vec<String> = files.expanded.iter().cloned().collect();
        dirs.push(String::new());
        let (tx, rx) = channel();
        let (root, repaint) = (self.workspaces[i].project.path.clone(), ctx.clone());
        std::thread::spawn(move || {
            let listing = dirs
                .into_iter()
                // Una carpeta que ya no existe simplemente no sale en el árbol.
                .filter_map(|d| {
                    forge_core::files::list_dir(&root, Path::new(&d))
                        .ok()
                        .map(|entries| (d, entries))
                })
                .collect();
            let changes = forge_core::git::status(&root)
                .map(|s| {
                    s.files
                        .iter()
                        .map(|f| (f.path.clone(), change_code(f)))
                        .collect()
                })
                .unwrap_or_default();
            let _ = tx.send((listing, changes));
            repaint.request_repaint();
        });
        self.workspaces[i].files.loading = Some(rx);
        ctx.request_repaint_after(Duration::from_secs(3));
    }

    pub(super) fn files_ui(
        ws: &mut Workspace,
        ui: &mut egui::Ui,
        rect: Rect,
        cmds: &mut Vec<UiCmd>,
    ) {
        ui.painter().rect_filled(rect, 0.0, theme::SIDEBAR);
        ui.painter().vline(
            rect.min.x + 0.5,
            rect.y_range(),
            egui::Stroke::new(1.0, theme::SEPARATOR),
        );
        let mut ui =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(16.0, 14.0))));
        ui.spacing_mut().item_spacing.y = 6.0;
        let files = &mut ws.files;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icon::FOLDER_OPEN)
                    .size(18.0)
                    .color(theme::ORANGE),
            );
            ui.label(
                RichText::new(tr!("Archivos"))
                    .size(15.0)
                    .color(theme::TEXT)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, tr!("Cerrar")).clicked() {
                    cmds.push(UiCmd::ToggleFiles);
                }
            });
        });
        if let Some(e) = &files.error {
            ui.label(RichText::new(e).size(12.0).color(theme::RED));
        }
        if let Some(path) = files.confirm_trash.clone() {
            ui.group(|ui| {
                ui.label(
                    RichText::new(tr!("¿Mandar «{p0}» a la papelera?", p0 = path.as_str()))
                        .size(12.5),
                );
                ui.label(
                    RichText::new(tr!("Podrás recuperarlo desde la papelera."))
                        .size(11.0)
                        .color(theme::TEXT_3),
                );
                ui.checkbox(&mut files.no_ask, tr!("No volver a preguntar"));
                ui.horizontal(|ui| {
                    if theme::secondary(ui, tr!("Cancelar")).clicked() {
                        files.confirm_trash = None;
                    }
                    if theme::primary(ui, format!("{}  {}", icon::TRASH, tr!("A la papelera")))
                        .clicked()
                    {
                        cmds.push(UiCmd::FilesTrash(path.clone(), files.no_ask));
                    }
                });
            });
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                Self::file_rows(files, ui, "", 0, cmds);
            });
    }

    /// Las filas de una carpeta; las subcarpetas abiertas se dibujan debajo de la suya.
    fn file_rows(
        files: &FilesView,
        ui: &mut egui::Ui,
        dir: &str,
        depth: usize,
        cmds: &mut Vec<UiCmd>,
    ) {
        let Some(entries) = files.listing.get(dir) else {
            return;
        };
        for entry in entries {
            let path = join(dir, &entry.name);
            let open = entry.is_dir && files.expanded.contains(&path);
            let code = files.changes.get(&path).copied();
            // Una carpeta se marca si algo dentro ha cambiado.
            let inside = entry.is_dir
                && files.changes.keys().any(|c| {
                    c.strip_prefix(path.as_str())
                        .is_some_and(|r| r.starts_with('/'))
                });
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 14.0);
                let glyph = match (entry.is_dir, open) {
                    (true, true) => icon::FOLDER_OPEN,
                    (true, false) => icon::FOLDER,
                    _ => icon::FILE_TEXT,
                };
                let (letter, color) = code.map_or(('\0', theme::TEXT), badge);
                let label = ui.add(
                    egui::Label::new(
                        RichText::new(format!("{glyph}  {}", entry.name))
                            .size(12.5)
                            .color(if code.is_some() { color } else { theme::TEXT_2 }),
                    )
                    .truncate()
                    .sense(Sense::click()),
                );
                if label.clicked() {
                    if entry.is_dir {
                        cmds.push(UiCmd::FilesToggleDir(path.clone()));
                    } else if code.is_some() {
                        cmds.push(UiCmd::GitDiff(path.clone(), false));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button_sized(
                        ui,
                        icon::TRASH,
                        &tr!("Mandar «{p0}» a la papelera", p0 = path.as_str()),
                        20.0,
                        theme::TEXT_3,
                    )
                    .clicked()
                    {
                        cmds.push(UiCmd::FilesTrashAsk(path.clone()));
                    }
                    if code.is_some() {
                        ui.label(
                            RichText::new(letter.to_string())
                                .monospace()
                                .strong()
                                .color(color),
                        );
                    } else if inside {
                        ui.label(RichText::new("●").size(8.0).color(theme::YELLOW));
                    }
                });
            });
            if open {
                Self::file_rows(files, ui, &path, depth + 1, cmds);
            }
        }
    }

    /// Acciones del panel de archivos.
    pub(super) fn apply_files(&mut self, cmd: UiCmd) {
        let Some(i) = self.active else { return };
        let no_ask = self
            .store
            .as_ref()
            .and_then(|s| s.setting(TRASH_NO_ASK).ok().flatten())
            .is_some_and(|v| v == "1");
        let ws = &mut self.workspaces[i];
        ws.files.dirty = true;
        ws.files.error = None;
        match cmd {
            UiCmd::FilesToggleDir(dir) => {
                if !ws.files.expanded.remove(&dir) {
                    ws.files.expanded.insert(dir);
                }
            }
            UiCmd::FilesTrashAsk(path) if no_ask => self.trash_path(i, &path),
            UiCmd::FilesTrashAsk(path) => {
                ws.files.no_ask = false;
                ws.files.confirm_trash = Some(path);
            }
            UiCmd::FilesTrash(path, remember) => {
                if remember && let Some(store) = self.store.as_ref() {
                    let _ = store.set_setting(TRASH_NO_ASK, "1");
                }
                self.trash_path(i, &path);
            }
            _ => {}
        }
    }

    fn trash_path(&mut self, i: usize, rel: &str) {
        let ws = &mut self.workspaces[i];
        ws.files.confirm_trash = None;
        ws.files.error = forge_core::files::trash(&ws.project.path, Path::new(rel)).err();
    }
}
