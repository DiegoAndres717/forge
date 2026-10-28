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
    /// Archivo abierto al lado del árbol.
    pub viewer: Option<Viewer>,
}

/// Cómo se ve el archivo abierto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Edit,
    Preview,
    Diff,
}

/// Un archivo abierto: su texto editable, o una imagen, o el motivo de no poder mostrarlo.
pub struct Viewer {
    pub path: String,
    /// Texto en edición y el último guardado (para saber si hay cambios sin guardar).
    pub content: String,
    pub saved: String,
    pub image: Option<egui::TextureHandle>,
    /// Por qué no se puede mostrar (binario, grande…).
    pub message: Option<String>,
    pub mode: ViewMode,
    pub diff: String,
    /// Otro archivo (o cerrar, con `None`) que espera a decidir qué hacer con lo sin guardar.
    pub pending: Option<Option<String>>,
    /// Última modificación del archivo cuando se abrió o guardó: si cambia por fuera (un agente
    /// lo ha editado), guardar no lo pisa sin avisar.
    pub modified: Option<std::time::SystemTime>,
}

impl Viewer {
    fn is_text(&self) -> bool {
        self.image.is_none() && self.message.is_none()
    }

    fn is_markdown(&self) -> bool {
        self.path.to_lowercase().ends_with(".md")
    }

    fn unsaved(&self) -> bool {
        self.is_text() && self.content != self.saved
    }
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
            .id_salt("file-tree")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                Self::file_rows(files, ui, "", 0, cmds);
            });
    }

    /// El archivo abierto: editor, vista previa o cambios, a la izquierda del árbol.
    pub(super) fn files_viewer_ui(
        ws: &mut Workspace,
        ui: &mut egui::Ui,
        rect: Rect,
        cmds: &mut Vec<UiCmd>,
    ) {
        ui.painter().rect_filled(rect, 0.0, theme::BG);
        ui.painter().vline(
            rect.min.x + 0.5,
            rect.y_range(),
            egui::Stroke::new(1.0, theme::SEPARATOR),
        );
        let Some(v) = ws.files.viewer.as_mut() else {
            return;
        };
        let mut ui =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(16.0, 14.0))));
        ui.spacing_mut().item_spacing.y = 8.0;
        let name = v.path.rsplit('/').next().unwrap_or(&v.path).to_string();
        let unsaved = v.unsaved();
        ui.horizontal(|ui| {
            let title = if unsaved {
                format!("{name}  ●")
            } else {
                name
            };
            ui.label(RichText::new(title).size(14.0).color(theme::TEXT).strong())
                .on_hover_text(&v.path);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, tr!("Cerrar archivo")).clicked() {
                    cmds.push(UiCmd::FilesCloseViewer);
                }
                if v.is_text() {
                    let save = ui.add_enabled(
                        unsaved,
                        egui::Button::new(RichText::new(tr!("Guardar (⌘S)")).size(12.0)),
                    );
                    if save.clicked() {
                        cmds.push(UiCmd::FilesSave);
                    }
                    let mut mode = |ui: &mut egui::Ui, m: ViewMode, text: &str| {
                        if ui.selectable_label(v.mode == m, text).clicked() && v.mode != m {
                            cmds.push(UiCmd::FilesMode(m));
                        }
                    };
                    mode(ui, ViewMode::Diff, tr!("Cambios"));
                    if v.is_markdown() {
                        mode(ui, ViewMode::Preview, tr!("Vista previa"));
                    }
                    mode(ui, ViewMode::Edit, tr!("Editar"));
                }
            });
        });
        if unsaved
            && ui
                .ctx()
                .input(|i| i.modifiers.command && i.key_pressed(Key::S))
        {
            cmds.push(UiCmd::FilesSave);
        }
        if v.pending.is_some() {
            ui.group(|ui| {
                ui.label(RichText::new(tr!("Hay cambios sin guardar.")).size(12.5));
                ui.horizontal(|ui| {
                    if theme::secondary(ui, tr!("Seguir editando")).clicked() {
                        cmds.push(UiCmd::FilesKeepEditing);
                    }
                    if theme::primary(ui, tr!("Descartar cambios")).clicked() {
                        cmds.push(UiCmd::FilesDiscard);
                    }
                });
            });
        }
        egui::ScrollArea::both()
            .id_salt("file-viewer")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                if let Some(why) = &v.message {
                    ui.label(RichText::new(why).color(theme::TEXT_3));
                } else if let Some(tex) = &v.image {
                    let width = ui.available_width().min(tex.size_vec2().x);
                    let scale = width / tex.size_vec2().x;
                    ui.image((tex.id(), tex.size_vec2() * scale));
                } else {
                    match v.mode {
                        ViewMode::Edit => {
                            ui.add(
                                egui::TextEdit::multiline(&mut v.content)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(30),
                            );
                        }
                        ViewMode::Preview => markdown(ui, &v.content),
                        ViewMode::Diff => diff_lines(ui, &v.diff),
                    }
                }
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
                    } else {
                        cmds.push(UiCmd::FilesOpen(path.clone()));
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
    pub(super) fn apply_files(&mut self, ctx: &egui::Context, cmd: UiCmd) {
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
            UiCmd::FilesOpen(path) => {
                // Desde la paleta el panel puede estar cerrado: se abre para ver el archivo.
                if !ws.files.open {
                    ws.files.open = true;
                    ws.guard.open = false;
                    ws.memory.open = false;
                    ws.ideas.open = false;
                    ws.git.open = false;
                }
                Self::open_file(ctx, ws, Some(path), false)
            }
            UiCmd::FilesCloseViewer => Self::open_file(ctx, ws, None, false),
            UiCmd::FilesDiscard => {
                let next = ws.files.viewer.as_mut().and_then(|v| v.pending.take());
                if let Some(next) = next {
                    Self::open_file(ctx, ws, next, true);
                }
            }
            UiCmd::FilesKeepEditing => {
                if let Some(v) = ws.files.viewer.as_mut() {
                    v.pending = None;
                }
            }
            UiCmd::FilesMode(mode) => {
                let root = ws.project.path.clone();
                if let Some(v) = ws.files.viewer.as_mut() {
                    v.mode = mode;
                    if mode == ViewMode::Diff {
                        v.diff = forge_core::files::diff(&root, Path::new(&v.path))
                            .unwrap_or_else(|e| e);
                    }
                }
            }
            UiCmd::FilesSave => {
                let root = ws.project.path.clone();
                if let Some(v) = ws.files.viewer.as_mut().filter(|v| v.is_text()) {
                    let rel = Path::new(&v.path);
                    let on_disk = std::fs::metadata(root.join(rel))
                        .and_then(|m| m.modified())
                        .ok();
                    if on_disk != v.modified {
                        ws.files.error = Some(tr!(
                            "El archivo cambió fuera del editor (¿un agente?). Ciérralo y ábrelo otra vez para no pisar esos cambios."
                        ).to_string());
                    } else {
                        match forge_core::files::save(&root, rel, &v.content) {
                            Ok(()) => {
                                v.saved = v.content.clone();
                                v.modified = std::fs::metadata(root.join(rel))
                                    .and_then(|m| m.modified())
                                    .ok();
                                if v.mode == ViewMode::Diff {
                                    v.diff =
                                        forge_core::files::diff(&root, rel).unwrap_or_else(|e| e);
                                }
                                ws.files.error = None;
                            }
                            Err(e) => ws.files.error = Some(e),
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Abre `next` en el visor (o lo cierra con `None`). Si lo que hay abierto tiene cambios sin
    /// guardar y no se fuerza, antes pregunta.
    // ponytail: se lee en el hilo de la interfaz (el texto tiene tope de 2 MB); el PDF tarda
    // algo más porque lo dibuja la vista rápida de macOS.
    fn open_file(ctx: &egui::Context, ws: &mut Workspace, next: Option<String>, force: bool) {
        if !force && let Some(v) = ws.files.viewer.as_mut().filter(|v| v.unsaved()) {
            v.pending = Some(next);
            return;
        }
        let Some(path) = next else {
            ws.files.viewer = None;
            return;
        };
        let root = ws.project.path.clone();
        let rel = Path::new(&path);
        let mut viewer = Viewer {
            path: path.clone(),
            content: String::new(),
            saved: String::new(),
            image: None,
            message: None,
            mode: ViewMode::Edit,
            diff: String::new(),
            pending: None,
            modified: std::fs::metadata(root.join(rel))
                .and_then(|m| m.modified())
                .ok(),
        };
        let picture = |file: &Path| {
            let img = image::open(file).map_err(|e| e.to_string())?.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            Ok::<_, String>(ctx.load_texture(&path, color, egui::TextureOptions::LINEAR))
        };
        match forge_core::files::load(&root, rel) {
            Ok(forge_core::files::Loaded::Text(text)) => {
                viewer.saved = text.clone();
                viewer.content = text;
                if viewer.is_markdown() {
                    viewer.mode = ViewMode::Preview;
                }
            }
            Ok(forge_core::files::Loaded::Image) => match picture(&root.join(rel)) {
                Ok(t) => viewer.image = Some(t),
                Err(e) => viewer.message = Some(e),
            },
            #[cfg(target_os = "macos")]
            Ok(forge_core::files::Loaded::Pdf) => {
                match forge_core::files::pdf_preview(&root, rel).and_then(|png| picture(&png)) {
                    Ok(t) => viewer.image = Some(t),
                    Err(e) => viewer.message = Some(e),
                }
            }
            #[cfg(not(target_os = "macos"))]
            Ok(forge_core::files::Loaded::Pdf) => {
                viewer.message = Some("la vista previa de PDF solo está en macOS".into());
            }
            Ok(forge_core::files::Loaded::Unsupported(why)) => viewer.message = Some(why),
            Err(e) => viewer.message = Some(e),
        }
        ws.files.viewer = Some(viewer);
    }

    fn trash_path(&mut self, i: usize, rel: &str) {
        let ws = &mut self.workspaces[i];
        ws.files.confirm_trash = None;
        ws.files.error = forge_core::files::trash(&ws.project.path, Path::new(rel)).err();
        if ws.files.error.is_none() && ws.files.viewer.as_ref().is_some_and(|v| v.path == rel) {
            ws.files.viewer = None;
        }
    }
}

/// Markdown sencillo: títulos, listas, bloques de código y párrafos.
fn markdown(ui: &mut egui::Ui, text: &str) {
    let mut code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        if code {
            ui.label(
                RichText::new(line)
                    .monospace()
                    .size(12.0)
                    .color(theme::TEXT_2),
            );
        } else if let Some((level, title)) = heading(line) {
            let size = [22.0, 18.0, 16.0, 14.5][level.min(4) - 1];
            ui.label(RichText::new(title).size(size).strong().color(theme::TEXT));
        } else if let Some(item) = line
            .trim_start()
            .strip_prefix("- ")
            .or_else(|| line.trim_start().strip_prefix("* "))
        {
            ui.label(RichText::new(format!("•  {item}")).size(13.0));
        } else if line.trim().is_empty() {
            ui.add_space(4.0);
        } else {
            ui.label(RichText::new(line).size(13.0));
        }
    }
}

/// «## Título» → (2, «Título»).
fn heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' '))
        .flatten()
        .map(|t| (hashes, t))
}

/// Un diff con las líneas añadidas en verde y las quitadas en rojo.
fn diff_lines(ui: &mut egui::Ui, diff: &str) {
    if diff.trim().is_empty() {
        ui.label(RichText::new(tr!("Sin cambios desde el último commit.")).color(theme::TEXT_3));
        return;
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    for line in diff.lines() {
        let color = match line.chars().next() {
            Some('+') if !line.starts_with("+++") => theme::GREEN,
            Some('-') if !line.starts_with("---") => theme::RED,
            Some('@') => theme::PURPLE,
            _ => theme::TEXT_3,
        };
        ui.label(RichText::new(line).monospace().size(12.0).color(color));
    }
}
