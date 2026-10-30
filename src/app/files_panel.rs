// Panel de archivos: las carpetas y archivos del proyecto como árbol, con lo que el agente
// ha creado o cambiado marcado, y borrado a la papelera del sistema (recuperable).
use super::*;
use forge_core::files::Entry;
use std::collections::HashSet;
use std::sync::Arc;

/// Lo que lee el hilo del árbol: carpetas abiertas, cambios de Git y, si se busca, todos los archivos.
pub(crate) type FilesData = (
    HashMap<String, Vec<Entry>>,
    HashMap<String, char>,
    Option<Vec<String>>,
);

/// Ajuste donde se guarda «no volver a preguntar» antes de mandar a la papelera.
pub(super) const TRASH_NO_ASK: &str = "files-trash-no-ask";

/// Alto de la barra que queda cuando las terminales están escondidas.
const HIDDEN_BAR: f32 = 30.0;

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
    /// Texto del buscador de archivos y todos los archivos del proyecto (solo se leen al buscar).
    pub search: String,
    /// Parte de la altura que ocupa el archivo (el resto, las terminales); 0 = la de siempre.
    pub split: f32,
    /// Terminales escondidas bajo el archivo abierto (⌘J).
    pub terminal_hidden: bool,
    pub all: Vec<String>,
    /// Archivo o carpeta nuevos que se están nombrando en el árbol.
    pub new_item: Option<NewItem>,
    /// Archivo abierto al lado del árbol.
    pub viewer: Option<Viewer>,
}

/// Lo nuevo que se está nombrando: dónde va, si es carpeta y el nombre escrito hasta ahora.
pub struct NewItem {
    pub parent: String,
    pub is_dir: bool,
    pub name: String,
    /// Falta darle el foco a la casilla (solo el primer fotograma).
    pub focus: bool,
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
    /// Marcas del margen del editor (líneas añadidas, cambiadas o quitadas respecto al commit).
    pub marks: Vec<forge_core::files::GutterMark>,
    /// Comentarios sobre líneas del diff: (índice de la línea, la línea con su bloque, el
    /// comentario). Se envían al agente; si el diff se regenera, se descartan.
    pub comments: Vec<(usize, String, String)>,
    /// Comentario que se está escribiendo: (índice de línea del diff, texto).
    pub draft: Option<(usize, String)>,
    /// Texto coloreado la última vez (huella del texto, secciones): no se recalcula cada fotograma.
    pub highlight: Option<(u64, Arc<egui::Galley>)>,
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
            && let Ok((listing, changes, all)) = rx.try_recv()
        {
            files.loading = None;
            files.loaded = Some(Instant::now());
            files.listing = listing;
            files.changes = changes;
            if let Some(all) = all {
                files.all = all;
            }
        }
        let stale = files.dirty
            || files
                .loaded
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3));
        if !files.open || files.loading.is_some() || !stale {
            return;
        }
        files.dirty = false;
        let want_all = !files.search.trim().is_empty();
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
            let all = want_all.then(|| forge_core::files::project_files(&root));
            let _ = tx.send((listing, changes, all));
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
                if theme::icon_button(ui, icon::FOLDER_SIMPLE_PLUS, tr!("Nueva carpeta")).clicked()
                {
                    cmds.push(UiCmd::FilesNewItem(String::new(), true));
                }
                if theme::icon_button(ui, icon::FILE_PLUS, tr!("Nuevo archivo")).clicked() {
                    cmds.push(UiCmd::FilesNewItem(String::new(), false));
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
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icon::MAGNIFYING_GLASS)
                    .size(14.0)
                    .color(theme::TEXT_3),
            );
            ui.add(
                egui::TextEdit::singleline(&mut files.search)
                    .hint_text(tr!("Buscar archivos…"))
                    .desired_width(f32::INFINITY),
            );
        });
        let searching = !files.search.trim().is_empty();
        // Al buscar se ve otra lista: lo que se estaba nombrando se deja.
        let mut draft = files.new_item.take().filter(|_| !searching);
        egui::ScrollArea::vertical()
            .id_salt("file-tree")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                if searching {
                    let found = search_files(&files.all, &files.search);
                    if found.is_empty() {
                        ui.label(RichText::new(tr!("Sin resultados")).color(theme::TEXT_3));
                    }
                    for path in found {
                        let name = path.rsplit('/').next().unwrap_or(path);
                        let dir = path.strip_suffix(name).unwrap_or("").trim_end_matches('/');
                        let code = files.changes.get(path).copied();
                        Self::file_row(ui, path, name, dir, false, false, 0, code, false, cmds);
                    }
                } else {
                    Self::file_rows(files, &mut draft, ui, "", 0, cmds);
                }
            });
        files.new_item = draft;
    }

    /// Reparte el espacio con un archivo abierto: el archivo arriba y las terminales abajo, o
    /// solo el archivo con una barra para volver a las terminales (⌘J). Devuelve el rectángulo
    /// del archivo y el de las terminales (`None`: escondidas).
    pub(super) fn editor_split(
        ws: &mut Workspace,
        ui: &mut egui::Ui,
        rest: Rect,
        cmds: &mut Vec<UiCmd>,
    ) -> (Rect, Option<Rect>) {
        if ws.files.terminal_hidden {
            let (viewer, bar) = rest.split_top_bottom_at_y(rest.max.y - HIDDEN_BAR);
            ui.painter().rect_filled(bar, 0.0, theme::SIDEBAR);
            ui.painter().hline(
                bar.x_range(),
                bar.min.y,
                egui::Stroke::new(1.0, theme::SEPARATOR),
            );
            let response = ui
                .interact(bar, egui::Id::new("show-terminal"), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.painter().text(
                egui::pos2(bar.min.x + 16.0, bar.center().y),
                egui::Align2::LEFT_CENTER,
                format!(
                    "{}  {}  {}",
                    icon::TERMINAL_WINDOW,
                    tr!("Mostrar terminal"),
                    forge_core::i18n::keys("⌘J")
                ),
                FontId::proportional(12.5),
                if response.hovered() {
                    theme::TEXT
                } else {
                    theme::TEXT_3
                },
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr!("Mostrar terminal"))
            });
            if response.clicked() {
                cmds.push(UiCmd::ToggleTerminal);
            }
            return (viewer, None);
        }
        let y = Self::editor_y(rest, ws.files.split);
        let (viewer, terminals) = rest.split_top_bottom_at_y(y);
        (viewer, Some(terminals))
    }

    /// Dónde acaba el archivo y empiezan las terminales: a cada uno le quedan al menos 120 px.
    fn editor_y(rest: Rect, split: f32) -> f32 {
        let ratio = if split > 0.0 { split } else { 0.62 };
        let low = rest.min.y + 120.0;
        let high = (rest.max.y - 120.0).max(low);
        (rest.min.y + rest.height() * ratio).clamp(low, high)
    }

    /// El divisor arrastrable entre el archivo y las terminales.
    pub(super) fn editor_divider(ws: &mut Workspace, ui: &mut egui::Ui, rest: Rect) {
        let y = Self::editor_y(rest, ws.files.split);
        let grab = Rect::from_min_max(
            egui::pos2(rest.min.x, y - 3.0),
            egui::pos2(rest.max.x, y + 3.0),
        );
        let response = ui.interact(grab, egui::Id::new("editor-divider"), Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if let Some(pos) = response
            .interact_pointer_pos()
            .filter(|_| response.dragged())
        {
            ws.files.split = ((pos.y - rest.min.y) / rest.height()).clamp(0.15, 0.9);
        }
        let color = if response.hovered() || response.dragged() {
            theme::ACCENT
        } else {
            theme::SEPARATOR
        };
        ui.painter()
            .hline(rest.x_range(), y, egui::Stroke::new(1.0, color));
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
            ui.label(RichText::new(name).size(14.0).color(theme::TEXT).strong())
                .on_hover_text(&v.path);
            if unsaved {
                // Como VS Code: un punto junto al nombre mientras hay cambios sin guardar.
                ui.label(RichText::new("●").size(12.0).color(theme::YELLOW))
                    .on_hover_text(tr!("Cambios sin guardar (⌘S)"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, tr!("Cerrar archivo")).clicked() {
                    cmds.push(UiCmd::FilesCloseViewer);
                }
                if theme::icon_button(ui, icon::TERMINAL_WINDOW, tr!("Ocultar terminal (⌘J)"))
                    .clicked()
                {
                    cmds.push(UiCmd::ToggleTerminal);
                }
                if v.is_text() {
                    let mut mode = |ui: &mut egui::Ui, m: ViewMode, text: &str| {
                        if ui.selectable_label(v.mode == m, text).clicked() && v.mode != m {
                            cmds.push(UiCmd::FilesMode(m));
                        }
                    };
                    if v.is_markdown() {
                        mode(ui, ViewMode::Preview, tr!("Vista previa"));
                    }
                    mode(ui, ViewMode::Diff, tr!("Cambios"));
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
                            let Viewer {
                                content,
                                path,
                                highlight,
                                marks,
                                ..
                            } = v;
                            code_editor(ui, content, path, highlight, marks);
                        }
                        ViewMode::Preview => markdown(ui, &v.content),
                        ViewMode::Diff => {
                            let Viewer {
                                diff,
                                comments,
                                draft,
                                ..
                            } = v;
                            diff_lines(ui, diff, comments, draft, cmds);
                        }
                    }
                }
            });
    }

    /// Las filas de una carpeta; las subcarpetas abiertas se dibujan debajo de la suya.
    fn file_rows(
        files: &FilesView,
        draft: &mut Option<NewItem>,
        ui: &mut egui::Ui,
        dir: &str,
        depth: usize,
        cmds: &mut Vec<UiCmd>,
    ) {
        if draft.as_ref().is_some_and(|d| d.parent == dir) {
            Self::new_item_row(ui, draft, depth, cmds);
        }
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
            Self::file_row(
                ui,
                &path,
                &entry.name,
                "",
                entry.is_dir,
                open,
                depth,
                code,
                inside,
                cmds,
            );
            if open {
                Self::file_rows(files, draft, ui, &path, depth + 1, cmds);
            }
        }
    }

    /// La fila donde se escribe el nombre de lo nuevo: Intro lo crea, Esc o salir lo cancela.
    fn new_item_row(
        ui: &mut egui::Ui,
        draft: &mut Option<NewItem>,
        depth: usize,
        cmds: &mut Vec<UiCmd>,
    ) {
        let Some(item) = draft.as_mut() else { return };
        let glyph = if item.is_dir {
            icon::FOLDER_SIMPLE_PLUS
        } else {
            icon::FILE_PLUS
        };
        let mut done = None; // Some(true) crea, Some(false) cancela
        ui.horizontal(|ui| {
            ui.add_space(4.0 + depth as f32 * 14.0 + 14.0);
            ui.label(RichText::new(glyph).size(14.0).color(theme::TEXT_3));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut item.name)
                    .hint_text(if item.is_dir {
                        tr!("Nombre de la carpeta")
                    } else {
                        tr!("Nombre del archivo")
                    })
                    .desired_width(f32::INFINITY),
            );
            if item.focus {
                edit.request_focus();
                item.focus = false;
            }
            // Intro y Esc le quitan el foco a la casilla; hacer clic fuera también.
            if edit.lost_focus() {
                done = Some(ui.input(|i| i.key_pressed(Key::Enter)));
            }
        });
        match done {
            Some(true) => cmds.push(UiCmd::FilesCreate(
                item.parent.clone(),
                item.is_dir,
                item.name.clone(),
            )),
            Some(false) => cmds.push(UiCmd::FilesCancelNew),
            None => {}
        }
    }

    /// Una fila del árbol: ancha, con resalte y manita al pasar, y su menú con el clic derecho.
    /// `dir` solo se muestra (en gris) en los resultados de la búsqueda.
    #[allow(clippy::too_many_arguments)]
    fn file_row(
        ui: &mut egui::Ui,
        path: &str,
        name: &str,
        dir: &str,
        is_dir: bool,
        open: bool,
        depth: usize,
        code: Option<char>,
        inside: bool,
        cmds: &mut Vec<UiCmd>,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        // Las filas se pintan a mano: sin esto no tendrían nombre para los lectores de pantalla.
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
        let painter = ui.painter().clone();
        if response.hovered() {
            painter.rect_filled(rect, 5.0, theme::SELECTED);
        }
        let (letter, color) = code.map_or(('\0', theme::TEXT), badge);
        let mut x = rect.min.x + 4.0 + depth as f32 * 14.0;
        let mid = rect.center().y;
        if is_dir {
            let caret = if open {
                icon::CARET_DOWN
            } else {
                icon::CARET_RIGHT
            };
            painter.text(
                egui::pos2(x, mid),
                egui::Align2::LEFT_CENTER,
                caret,
                FontId::proportional(11.0),
                theme::TEXT_3,
            );
        }
        x += 14.0;
        let (glyph, glyph_color) = if is_dir {
            (
                if open {
                    icon::FOLDER_OPEN
                } else {
                    icon::FOLDER
                },
                theme::ORANGE,
            )
        } else {
            file_glyph(name)
        };
        painter.text(
            egui::pos2(x, mid),
            egui::Align2::LEFT_CENTER,
            glyph,
            FontId::proportional(14.0),
            glyph_color,
        );
        x += 22.0;
        let right = rect.max.x - if code.is_some() || inside { 20.0 } else { 4.0 };
        let name_color = if code.is_some() { color } else { theme::TEXT_2 };
        let clip = Rect::from_min_max(egui::pos2(x, rect.min.y), egui::pos2(right, rect.max.y));
        let painter = ui.painter().with_clip_rect(clip);
        let shown = painter.text(
            egui::pos2(x, mid),
            egui::Align2::LEFT_CENTER,
            name,
            FontId::proportional(12.5),
            name_color,
        );
        if !dir.is_empty() {
            painter.text(
                egui::pos2(shown.max.x + 8.0, mid),
                egui::Align2::LEFT_CENTER,
                dir,
                FontId::proportional(11.0),
                theme::TEXT_4,
            );
        }
        if code.is_some() {
            ui.painter().text(
                egui::pos2(rect.max.x - 8.0, mid),
                egui::Align2::RIGHT_CENTER,
                letter,
                FontId::monospace(12.0),
                color,
            );
        } else if inside {
            ui.painter()
                .circle_filled(egui::pos2(rect.max.x - 10.0, mid), 3.0, theme::YELLOW);
        }
        if response.clicked() {
            cmds.push(if is_dir {
                UiCmd::FilesToggleDir(path.to_string())
            } else {
                UiCmd::FilesOpen(path.to_string())
            });
        }
        response.context_menu(|ui| {
            ui.set_min_width(200.0);
            if !is_dir && ui.button(tr!("Abrir")).clicked() {
                cmds.push(UiCmd::FilesOpen(path.to_string()));
                ui.close();
            }
            if ui.button(tr!("Copiar ruta")).clicked() {
                ui.ctx().copy_text(path.to_string());
                ui.close();
            }
            if ui.button(tr!("Mostrar en el Finder")).clicked() {
                cmds.push(UiCmd::FilesReveal(path.to_string()));
                ui.close();
            }
            // Lo nuevo va dentro de la carpeta, o junto al archivo.
            let parent = if is_dir {
                path.to_string()
            } else {
                path.rsplit_once('/').map_or("", |(d, _)| d).to_string()
            };
            ui.separator();
            if ui.button(tr!("Nuevo archivo")).clicked() {
                cmds.push(UiCmd::FilesNewItem(parent.clone(), false));
                ui.close();
            }
            if ui.button(tr!("Nueva carpeta")).clicked() {
                cmds.push(UiCmd::FilesNewItem(parent, true));
                ui.close();
            }
            ui.separator();
            let trash = RichText::new(format!("{}  {}", icon::TRASH, tr!("A la papelera")))
                .color(theme::RED);
            if ui.button(trash).clicked() {
                cmds.push(UiCmd::FilesTrashAsk(path.to_string()));
                ui.close();
            }
        });
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
            UiCmd::ToggleTerminal => ws.files.terminal_hidden = !ws.files.terminal_hidden,
            UiCmd::FilesReveal(path) => {
                if let Err(e) = forge_core::files::reveal(&ws.project.path, Path::new(&path)) {
                    ws.files.error = Some(e);
                }
            }
            UiCmd::FilesNewItem(parent, is_dir) => {
                ws.files.expanded.insert(parent.clone());
                ws.files.new_item = Some(NewItem {
                    parent,
                    is_dir,
                    name: String::new(),
                    focus: true,
                });
            }
            UiCmd::FilesCreate(parent, is_dir, name) => {
                let root = ws.project.path.clone();
                match forge_core::files::create(&root, Path::new(&parent), &name, is_dir) {
                    Ok(rel) => {
                        ws.files.new_item = None;
                        if is_dir {
                            ws.files.expanded.insert(rel);
                        } else {
                            Self::open_file(ctx, ws, Some(rel), false);
                        }
                    }
                    // Se sigue nombrando: el aviso sale arriba y el nombre se puede corregir.
                    Err(e) => {
                        ws.files.error = Some(e);
                        if let Some(item) = ws.files.new_item.as_mut() {
                            item.focus = true;
                        }
                    }
                }
            }
            UiCmd::FilesCancelNew => ws.files.new_item = None,
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
                        // El diff nuevo puede tener otras líneas: lo comentado ya no vale.
                        v.comments.clear();
                        v.draft = None;
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
                                v.marks = gutter(&root, rel);
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
            marks: Vec::new(),
            comments: Vec::new(),
            draft: None,
            highlight: None,
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
                viewer.marks = gutter(&root, rel);
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

/// Un diff con las líneas añadidas en verde y las quitadas en rojo. Un clic en una línea
/// añadida o quitada abre su comentario; arriba se envían todos al agente.
fn diff_lines(
    ui: &mut egui::Ui,
    diff: &str,
    comments: &mut Vec<(usize, String, String)>,
    draft: &mut Option<(usize, String)>,
    cmds: &mut Vec<UiCmd>,
) {
    if diff.trim().is_empty() {
        ui.label(RichText::new(tr!("Sin cambios desde el último commit.")).color(theme::TEXT_3));
        return;
    }
    if !comments.is_empty() {
        let text = tr!("Enviar {n} comentarios al agente", n = comments.len());
        if theme::primary(ui, &text).clicked() {
            cmds.push(UiCmd::FilesSendComments);
        }
        ui.add_space(4.0);
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    // Último encabezado de bloque (@@ …): sitúa la línea comentada para el agente.
    let mut hunk = "";
    for (n, line) in diff.lines().enumerate() {
        if line.starts_with("@@") {
            hunk = line;
        }
        let changed = (line.starts_with('+') && !line.starts_with("+++"))
            || (line.starts_with('-') && !line.starts_with("---"));
        let color = match line.chars().next() {
            Some('+') if changed => theme::GREEN,
            Some('-') if changed => theme::RED,
            Some('@') => theme::PURPLE,
            _ => theme::TEXT_3,
        };
        let label = ui.add(
            egui::Label::new(RichText::new(line).monospace().size(12.0).color(color)).sense(
                if changed {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                },
            ),
        );
        if changed
            && label
                .on_hover_text(tr!("Clic para comentar esta línea"))
                .clicked()
        {
            // Con un comentario a medias, otro clic no lo pisa.
            if draft.as_ref().is_none_or(|(_, t)| t.trim().is_empty()) {
                *draft = Some((n, String::new()));
            }
        }
        for (_, _, text) in comments.iter().filter(|(i, _, _)| *i == n) {
            ui.label(
                RichText::new(format!("   {} {text}", icon::CHAT_TEXT))
                    .size(12.0)
                    .color(theme::ACCENT),
            );
        }
        if draft.as_ref().is_some_and(|(i, _)| *i == n) {
            let mut done = None;
            if let Some((_, text)) = draft.as_mut() {
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(text)
                            .hint_text(tr!("Tu comentario…"))
                            .desired_width(280.0),
                    );
                    // Foco solo al abrir; después el campo lo conserva mientras se escribe.
                    if text.is_empty() {
                        edit.request_focus();
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if (theme::secondary(ui, tr!("Añadir")).clicked() || enter)
                        && !text.trim().is_empty()
                    {
                        done = Some(true);
                    }
                    if theme::secondary(ui, tr!("Cancelar")).clicked() {
                        done = Some(false);
                    }
                });
            }
            match done {
                Some(true) => {
                    if let Some((_, text)) = draft.take() {
                        let place = if hunk.is_empty() || line.starts_with("@@") {
                            line.to_string()
                        } else {
                            format!("{hunk} {line}")
                        };
                        comments.push((n, place, text));
                    }
                }
                Some(false) => *draft = None,
                None => {}
            }
        }
    }
}

/// El editor: texto con color de sintaxis, números de línea y la línea del cursor resaltada.
fn code_editor(
    ui: &mut egui::Ui,
    content: &mut String,
    path: &str,
    cache: &mut Option<(u64, Arc<egui::Galley>)>,
    marks: &[forge_core::files::GutterMark],
) {
    use std::hash::{Hash, Hasher};
    let file_name = path.rsplit('/').next().unwrap_or(path).to_string();
    let font = FontId::monospace(13.0);
    let digits = (content.lines().count().max(1) + 1)
        .to_string()
        .len()
        .max(2);
    let gutter = digits as f32 * 8.0 + 22.0;
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| {
        let text = text.as_str();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let key = hasher.finish();
        // Mientras el texto no cambie se reutiliza el mismo galley: nada se recalcula.
        if let Some((k, galley)) = cache.as_ref()
            && *k == key
        {
            return Arc::clone(galley);
        }
        let mut job = crate::highlight::layout(text, &file_name, font.clone(), theme::TEXT_2);
        job.wrap.max_width = f32::INFINITY; // el código no se parte: se desplaza de lado
        let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
        *cache = Some((key, Arc::clone(&galley)));
        galley
    };
    ui.horizontal_top(|ui| {
        let left = ui.cursor().min.x;
        ui.add_space(gutter);
        // Hueco para la línea del cursor: se rellena después, pero queda detrás del texto.
        let line_slot = ui.painter().add(egui::Shape::Noop);
        let out = egui::TextEdit::multiline(content)
            .font(FontId::monospace(13.0))
            .frame(egui::Frame::NONE)
            .desired_width(f32::INFINITY)
            .desired_rows(30)
            .layouter(&mut layouter)
            .show(ui);
        let painter = ui.painter().clone();
        let origin = out.galley_pos;
        // La línea del cursor, resaltada detrás del texto.
        if let Some(range) = out.cursor_range {
            let at = out.galley.pos_from_cursor(range.primary);
            let line = Rect::from_min_max(
                egui::pos2(left + gutter - 6.0, origin.y + at.min.y),
                egui::pos2(out.response.rect.max.x, origin.y + at.max.y),
            );
            painter.set(
                line_slot,
                egui::Shape::rect_filled(line, 0.0, theme::SELECTED),
            );
        }
        let visible = ui.clip_rect().y_range();
        // Un número por línea de verdad (no por fila: el código no se parte).
        let mut n = 1;
        let mut new_line = true;
        for placed in &out.galley.rows {
            // Solo se pintan los números de las líneas a la vista.
            let y = origin.y + placed.pos.y;
            if new_line && y + 20.0 >= visible.min && y <= visible.max {
                painter.text(
                    egui::pos2(left + gutter - 12.0, y),
                    egui::Align2::RIGHT_TOP,
                    n,
                    FontId::monospace(12.0),
                    theme::TEXT_4,
                );
            }
            if new_line && y + 20.0 >= visible.min && y <= visible.max {
                gutter_mark(ui, marks, n, left + gutter - 6.0, y, placed.height());
            }
            if new_line {
                n += 1;
            }
            new_line = placed.ends_with_newline;
        }
    });
}

/// Marcas del margen de un archivo; sin Git o sin cambios, ninguna.
fn gutter(root: &Path, rel: &Path) -> Vec<forge_core::files::GutterMark> {
    forge_core::files::diff(root, rel)
        .map(|d| forge_core::files::gutter_marks(&d))
        .unwrap_or_default()
}

/// Barra de color junto a la línea `n` si cambió: verde añadida, azul cambiada, triángulo rojo
/// si se quitó algo justo debajo. Al pasar el cursor sale lo que había antes.
fn gutter_mark(
    ui: &mut egui::Ui,
    marks: &[forge_core::files::GutterMark],
    n: usize,
    x: f32,
    y: f32,
    height: f32,
) {
    use forge_core::files::MarkKind;
    for (i, m) in marks.iter().enumerate().filter(|(_, m)| m.line == n) {
        let (color, bar) = match m.kind {
            MarkKind::Added => (
                theme::GREEN,
                Rect::from_min_size(egui::pos2(x, y), Vec2::new(3.0, height)),
            ),
            MarkKind::Modified => (
                theme::ACCENT,
                Rect::from_min_size(egui::pos2(x, y), Vec2::new(3.0, height)),
            ),
            MarkKind::Deleted => (
                theme::RED,
                Rect::from_min_size(egui::pos2(x, y + height - 2.0), Vec2::new(8.0, 4.0)),
            ),
        };
        ui.painter().rect_filled(bar, 0.0, color);
        if m.removed.is_empty() {
            continue;
        }
        let hit = ui.interact(
            bar.expand2(Vec2::new(4.0, 0.0)),
            ui.id().with(("gutter", i)),
            egui::Sense::hover(),
        );
        hit.on_hover_ui(|ui| {
            ui.label(RichText::new(tr!("Antes")).size(11.0).color(theme::TEXT_3));
            for l in &m.removed {
                ui.label(
                    RichText::new(format!("- {l}"))
                        .monospace()
                        .color(theme::RED),
                );
            }
        });
    }
}

/// Icono y color de un archivo según su extensión.
fn file_glyph(name: &str) -> (&'static str, Color32) {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e).to_lowercase();
    match ext.as_str() {
        "rs" => (icon::FILE_RS, theme::ORANGE),
        "ts" => (icon::FILE_TS, theme::ACCENT),
        "tsx" => (icon::FILE_TSX, theme::ACCENT),
        "js" | "mjs" | "cjs" => (icon::FILE_JS, theme::YELLOW),
        "jsx" => (icon::FILE_JSX, theme::YELLOW),
        "py" => (icon::FILE_PY, theme::ACCENT),
        "c" | "h" => (icon::FILE_C, theme::ACCENT),
        "cpp" | "cc" | "hpp" => (icon::FILE_CPP, theme::ACCENT),
        "css" | "scss" => (icon::FILE_CSS, theme::PURPLE),
        "html" | "htm" => (icon::FILE_HTML, theme::ORANGE),
        "vue" => (icon::FILE_VUE, theme::GREEN),
        "md" | "markdown" => (icon::FILE_MD, theme::TEXT_2),
        "json" | "toml" | "yaml" | "yml" | "ini" | "lock" => (icon::FILE_CODE, theme::TEXT_3),
        "sql" => (icon::FILE_SQL, theme::TEXT_3),
        "csv" => (icon::FILE_CSV, theme::GREEN),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" => (icon::FILE_IMAGE, theme::PURPLE),
        "svg" => (icon::FILE_SVG, theme::PURPLE),
        "pdf" => (icon::FILE_PDF, theme::RED),
        "zip" | "gz" | "tar" | "dmg" => (icon::FILE_ARCHIVE, theme::TEXT_3),
        "mp3" | "wav" => (icon::FILE_AUDIO, theme::TEXT_3),
        "mp4" | "mov" => (icon::FILE_VIDEO, theme::TEXT_3),
        "txt" => (icon::FILE_TXT, theme::TEXT_3),
        _ => (icon::FILE, theme::TEXT_3),
    }
}

/// Los archivos que coinciden con lo escrito en el buscador (difuso, mejores primero).
fn search_files<'a>(all: &'a [String], query: &str) -> Vec<&'a String> {
    let mut found: Vec<(i32, &String)> = all
        .iter()
        .filter_map(|p| super::palette::fuzzy(query, p).map(|s| (s, p)))
        .collect();
    found.sort_by_key(|(s, p)| (-s, p.len()));
    found.into_iter().take(100).map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_finds_files_by_a_part_of_their_path_best_first() {
        let all: Vec<String> = [
            "src/app/mod.rs",
            "src/main.rs",
            "README.md",
            "docs/model.md",
        ]
        .map(String::from)
        .to_vec();
        let found = search_files(&all, "main");
        assert_eq!(found.first().map(|s| s.as_str()), Some("src/main.rs"));
        assert!(!found.iter().any(|p| p.as_str() == "README.md"));
        assert!(search_files(&all, "zzzz").is_empty());
    }

    #[test]
    fn the_editor_split_leaves_room_for_both_parts() {
        let rest = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 600.0));
        // Sin valor guardado, el archivo ocupa algo más de la mitad.
        assert_eq!(App::editor_y(rest, 0.0), 372.0);
        // Arrastrar hasta los extremos nunca deja menos de 120 px a nadie.
        assert_eq!(App::editor_y(rest, 0.01), 120.0);
        assert_eq!(App::editor_y(rest, 0.99), 480.0);
        // Un hueco diminuto no rompe nada.
        let tiny = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 100.0));
        assert!(App::editor_y(tiny, 0.5).is_finite());
    }

    #[test]
    fn each_kind_of_file_gets_its_own_icon() {
        assert_eq!(file_glyph("lib.rs").0, icon::FILE_RS);
        assert_eq!(file_glyph("App.TSX").0, icon::FILE_TSX);
        assert_eq!(file_glyph("notes").0, icon::FILE);
    }
}
