// Marco de la ventana: inicio, barra de herramientas, barra de estado y avisos.
use super::*;
use forge_core::tr;

/// Ancho de los botones junto a los semáforos (barra lateral y Ajustes).
const WINDOW_BUTTONS_WIDTH: f32 = 58.0;

impl App {
    /// Inicio: proyectos recientes como lista agrupada, con abrir y arrastrar carpeta.
    pub(super) fn home(&mut self, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
        let recent = self.db(|s| s.recent()).unwrap_or_default();
        let width = (rect.width() - 64.0).clamp(320.0, 720.0);
        let inner = Rect::from_min_size(
            egui::pos2(rect.center().x - width / 2.0, rect.min.y + 48.0),
            Vec2::new(width, rect.height() - 64.0),
        );
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        ui.label(
            RichText::new("Forge")
                .size(30.0)
                .color(theme::TEXT)
                .strong(),
        );
        ui.label(
            RichText::new(tr!(
                "Proyectos con sus terminales, agentes, procesos y Project Guard."
            ))
            .size(14.0)
            .color(theme::TEXT_3),
        );
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, tr!("{p0}  Abrir proyecto…", p0 = icon::FOLDER_PLUS))
                .on_hover_text("⌘O")
                .clicked()
            {
                cmds.push(UiCmd::OpenFolder);
            }
            ui.label(
                RichText::new(tr!("o arrastra una carpeta a la ventana")).color(theme::TEXT_3),
            );
        });
        ui.add_space(22.0);
        // Ideas generales (no son de ningún proyecto).
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::LIGHTBULB).color(theme::YELLOW));
            ui.label(
                RichText::new(tr!("Ideas generales"))
                    .size(13.0)
                    .color(theme::TEXT_2)
                    .strong(),
            );
            if self.general.open_count > 0 {
                ui.label(
                    RichText::new(tr!("{p0} pendientes", p0 = self.general.open_count))
                        .size(12.0)
                        .color(theme::TEXT_3),
                );
            }
        });
        ui.add_space(4.0);
        theme::card(&mut ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("general-ideas")
                .max_height(220.0)
                .show(ui, |ui| Self::ideas_list(&mut self.general, None, ui, cmds));
        });
        ui.add_space(22.0);
        if recent.is_empty() {
            return;
        }
        ui.label(
            RichText::new(tr!("Recientes"))
                .size(13.0)
                .color(theme::TEXT_2)
                .strong(),
        );
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0x34, 0x34, 0x36)))
                    .corner_radius(10.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (n, row) in recent.iter().enumerate() {
                            let open = self
                                .workspaces
                                .iter()
                                .position(|w| w.project.path == row.path);
                            let exists = row.path.is_dir();
                            let (rect, response) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 58.0),
                                Sense::click(),
                            );
                            let painter = ui.painter();
                            if response.hovered() && exists {
                                let top = if n == 0 { 10 } else { 0 };
                                let bottom = if n + 1 == recent.len() { 10 } else { 0 };
                                let radius = egui::CornerRadius {
                                    nw: top,
                                    ne: top,
                                    sw: bottom,
                                    se: bottom,
                                };
                                painter.rect_filled(rect, radius, theme::SURFACE_HOVER);
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if n > 0 {
                                painter.hline(
                                    rect.min.x + 52.0..=rect.max.x,
                                    rect.min.y,
                                    egui::Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3a, 0x3c)),
                                );
                            }
                            let color = if exists { theme::ACCENT } else { theme::TEXT_4 };
                            painter.text(
                                egui::pos2(rect.min.x + 26.0, rect.center().y),
                                egui::Align2::CENTER_CENTER,
                                icon::FOLDER,
                                FontId::proportional(22.0),
                                color,
                            );
                            painter.text(
                                egui::pos2(rect.min.x + 52.0, rect.min.y + 19.0),
                                egui::Align2::LEFT_CENTER,
                                &row.name,
                                FontId::proportional(14.0),
                                theme::TEXT,
                            );
                            let sub = if exists {
                                tilde(&row.path)
                            } else {
                                tr!("{p0} · carpeta no encontrada", p0 = tilde(&row.path))
                            };
                            painter.text(
                                egui::pos2(rect.min.x + 52.0, rect.min.y + 39.0),
                                egui::Align2::LEFT_CENTER,
                                sub,
                                FontId::proportional(11.5),
                                if exists { theme::TEXT_3 } else { theme::RED },
                            );
                            // A la derecha: abierto, rama y cuándo se abrió.
                            let mut x = rect.max.x - 16.0;
                            let mut right = |text: String, color: Color32| {
                                let galley =
                                    painter.layout_no_wrap(text, FontId::proportional(12.0), color);
                                x -= galley.size().x;
                                painter.galley(
                                    egui::pos2(x, rect.center().y - galley.size().y / 2.0),
                                    galley,
                                    color,
                                );
                                x -= 14.0;
                            };
                            right(store::ago(row.last_opened), theme::TEXT_3);
                            if let Some(branch) =
                                exists.then(|| project::git_branch(&row.path)).flatten()
                            {
                                right(format!("{}  {branch}", icon::GIT_BRANCH), theme::TEXT_3);
                            }
                            if let Some(i) = open {
                                let running = self.workspaces[i].processes.active_count();
                                let text = if running > 0 {
                                    tr!("● abierto · {running} en marcha", running = running)
                                } else {
                                    "● abierto".into()
                                };
                                right(text, theme::GREEN);
                            }
                            if response.clicked() && exists {
                                cmds.push(match open {
                                    Some(i) => UiCmd::Activate(i),
                                    None => UiCmd::Open(row.path.clone()),
                                });
                            }
                            response.context_menu(|ui| {
                                if ui
                                    .button(tr!("{p0}  Quitar de recientes", p0 = icon::TRASH))
                                    .clicked()
                                {
                                    if let Some(i) = open {
                                        cmds.push(UiCmd::Close(i));
                                    }
                                    cmds.push(UiCmd::Forget(row.path.clone()));
                                }
                            });
                        }
                    });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tr!(
                        "Clic derecho en un proyecto para quitarlo de recientes."
                    ))
                    .size(11.0)
                    .color(theme::TEXT_4),
                );
            });
    }

    /// Barra de herramientas unificada: proyecto y rama, estado del Guard, RAM y acciones.
    /// Búsqueda (abre ⌘K): con texto y atajo, o solo la lupa (`compact`).
    fn search_pill(ui: &mut egui::Ui, rect: Rect, compact: bool, cmds: &mut Vec<UiCmd>) {
        let text = if compact {
            icon::MAGNIFYING_GLASS.to_string()
        } else {
            format!("{}  {}", icon::MAGNIFYING_GLASS, tr!("Buscar"))
        };
        let mut button = egui::Button::new(RichText::new(text).size(12.0).color(theme::TEXT_3))
            .fill(theme::SURFACE)
            .corner_radius(13.0);
        if !compact {
            button = button.right_text(RichText::new("⌘K").size(11.0).color(theme::TEXT_4));
        }
        // En su propia capa: no mueve el cursor de la barra (la rama sigue a la izquierda).
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        if child
            .add_sized(rect.size(), button)
            .on_hover_text(tr!("Buscar acciones, proyectos, agentes, procesos… (⌘K)"))
            .clicked()
        {
            cmds.push(UiCmd::OpenPalette);
        }
    }

    /// Junto a los semáforos de la ventana (como Warp): barra lateral y Ajustes.
    pub(super) fn window_buttons(&mut self, ui: &mut egui::Ui) {
        let start = self.traffic_end + 12.0;
        let strip = Rect::from_min_max(
            egui::pos2(start, 0.0),
            egui::pos2(start + WINDOW_BUTTONS_WIDTH, theme::TOOLBAR_HEIGHT),
        );
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(strip)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        ui.spacing_mut().item_spacing.x = 2.0;
        let tip = if self.sidebar {
            tr!("Ocultar barra lateral (⌘B)")
        } else {
            tr!("Mostrar barra lateral (⌘B)")
        };
        // Seleccionado mientras la barra lateral está abierta (como Warp).
        if theme::icon_toggle(&mut ui, icon::SIDEBAR_SIMPLE, tip, self.sidebar).clicked() {
            self.sidebar = !self.sidebar;
        }
        // Seleccionado mientras Ajustes está abierto; otro clic lo cierra.
        if theme::icon_toggle(
            &mut ui,
            icon::GEAR_SIX,
            tr!("Ajustes (⌘,)"),
            self.settings_open,
        )
        .clicked()
        {
            self.settings_open = !self.settings_open;
        }
    }

    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
        ui.painter().rect_filled(rect, 0.0, theme::TOOLBAR);
        theme::hairline(ui, rect, false);
        // Sin barra lateral, los semáforos y sus botones quedan sobre la barra de herramientas.
        let left = self.traffic_end + 12.0 + WINDOW_BUTTONS_WIDTH + 12.0;
        let inner = Rect::from_min_max(
            rect.min + Vec2::new(left, 0.0),
            rect.max - Vec2::new(10.0, 0.0),
        );
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        ui.spacing_mut().item_spacing.x = 4.0;
        // Búsqueda centrada en la ventana (como Warp); si no cabe con texto entre la rama y
        // los botones de la derecha, queda solo la lupa junto a esos botones.
        let right_edge = inner.max.x - self.toolbar_right - 12.0;
        let width = (rect.width() * 0.3).clamp(200.0, 380.0);
        let mut pill = Rect::from_center_size(rect.center(), Vec2::new(width, 26.0));
        let compact = pill.min.x < left + 90.0 || pill.max.x > right_edge;
        if compact {
            pill = Rect::from_min_size(
                egui::pos2(right_edge - 30.0, rect.center().y - 13.0),
                Vec2::new(28.0, 26.0),
            );
        }
        if pill.min.x > left {
            Self::search_pill(&mut ui, pill, compact, cmds);
        }
        let Some(i) = self.active else {
            ui.label(RichText::new(tr!("Inicio")).size(13.0).color(theme::TEXT_2));
            return;
        };
        let bytes = self
            .ram
            .lock()
            .ok()
            .and_then(|r| r.get(&self.workspaces[i].project.path).copied());
        let ws = &mut self.workspaces[i];
        let right_start = ui.max_rect().max.x;
        let right = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::icon_toggle(ui, icon::GIT_BRANCH, "Git (⌘⇧G)", ws.git.open).clicked() {
                cmds.push(UiCmd::ToggleGit);
            }
            if theme::icon_toggle(
                ui,
                icon::BRAIN,
                tr!("Memoria del proyecto (⌘⇧M)"),
                ws.memory.open,
            )
            .clicked()
            {
                cmds.push(UiCmd::ToggleMemory);
            }
            let (text, color) = match &ws.guard.run {
                None => ("Guard".to_string(), theme::TEXT_3),
                Some(run) => run.snapshot(|r| verdict_short(r.verdict())),
            };
            let guard = ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("{}  {text}", icon::SHIELD_CHECK))
                            .size(12.0)
                            .color(color),
                    )
                    .fill(if ws.guard.open {
                        theme::SURFACE_HOVER
                    } else {
                        theme::SURFACE
                    })
                    .corner_radius(13.0)
                    .min_size(Vec2::new(0.0, 26.0)),
                )
                .on_hover_text("Project Guard (⌘G)");
            if guard.clicked() {
                cmds.push(UiCmd::ToggleGuard);
            }
            ui.add_space(6.0);
            if theme::icon_button(ui, icon::SQUARE_SPLIT_VERTICAL, tr!("Dividir abajo (⌘⇧D)"))
                .clicked()
            {
                cmds.push(UiCmd::Ws(WsAction::Split(Dir::Column)));
            }
            if theme::icon_button(
                ui,
                icon::SQUARE_SPLIT_HORIZONTAL,
                tr!("Dividir a la derecha (⌘D)"),
            )
            .clicked()
            {
                cmds.push(UiCmd::Ws(WsAction::Split(Dir::Row)));
            }
            if theme::icon_button(ui, icon::TERMINAL_WINDOW, tr!("Nueva terminal (⌘T)")).clicked()
            {
                cmds.push(UiCmd::Ws(WsAction::NewTerminal));
            }
            if let Some(bytes) = bytes {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("{}  {}", icon::MEMORY, human_bytes(bytes)))
                        .size(12.0)
                        .color(theme::TEXT_3),
                )
                .on_hover_text(tr!(
                    "Memoria de los procesos de este proyecto (terminales, agentes y procesos)"
                ));
            }
        });
        self.toolbar_right = (right_start - right.response.rect.min.x).max(0.0);
    }

    /// Barra de estado con los atajos principales (plan §4.2).
    pub(super) fn status_bar(&self, ui: &egui::Ui, rect: Rect) {
        ui.painter().rect_filled(rect, 0.0, theme::TOOLBAR);
        ui.painter().hline(
            rect.x_range(),
            rect.min.y + 0.5,
            egui::Stroke::new(1.0, theme::SEPARATOR),
        );
        let hints = if self.active.is_some() {
            tr!(
                "⌘K Acciones    ⌃Tab Reciente    ⌘T Terminal    ⌘D Dividir    ⌘⇧D Abajo    ⌘⌥← → Foco    ⌘G Guard    ⌘⇧M Memoria    ⌘⇧A Agente"
            )
        } else {
            tr!("⌘O Abrir carpeta    ⌘1…9 Proyectos    ⌃Tab Reciente    ⌘B Barra lateral")
        };
        ui.painter().text(
            egui::pos2(rect.min.x + 12.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            hints,
            FontId::proportional(11.0),
            theme::TEXT_4,
        );
        if let Some(i) = self.active
            && let Some((spent, budget)) =
                self.workspaces[i].guard.ai_spend.filter(|(s, _)| *s > 0.0)
        {
            ui.painter().text(
                egui::pos2(rect.max.x - 12.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("IA este mes ${spent:.2} de ${budget:.2}").replace('.', ","),
                FontId::proportional(11.0),
                theme::TEXT_4,
            );
        }
    }

    /// Zonas vacías de barra de título: arrastran la ventana; doble clic hace zoom (como macOS).
    pub(super) fn window_drag(&self, ui: &egui::Ui, rect: Rect) {
        let response = ui.interact(rect, egui::Id::new("window-drag"), Sense::click_and_drag());
        if response.double_clicked() {
            let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        } else if response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    /// Mide en segundo plano la RAM de los procesos del proyecto activo.
    /// RAM de cada proyecto despierto, cada 5 s en segundo plano.
    pub(super) fn ram_tick(&mut self, ctx: &egui::Context) {
        if self
            .ram_checked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.ram_checked = Some(Instant::now());
        let roots: Vec<(PathBuf, Vec<u32>)> = self
            .workspaces
            .iter()
            .filter(|w| !w.is_dormant())
            .map(|w| (w.project.path.clone(), w.pids()))
            .collect();
        let (ram, ctx) = (self.ram.clone(), ctx.clone());
        std::thread::spawn(move || {
            // ponytail: un `ps` por proyecto; con decenas despiertos, leer la tabla una vez.
            let usage: HashMap<PathBuf, u64> = roots
                .into_iter()
                .map(|(path, pids)| (path, processes::memory_usage(&pids)))
                .collect();
            *ram.lock().unwrap() = usage;
            ctx.request_repaint();
        });
    }

    /// Aviso de error flotante (abajo a la derecha); clic para cerrarlo.
    pub(super) fn error_banner(&mut self, ui: &egui::Ui, area: Rect) {
        let Some(error) = &self.error else { return };
        let width = area.width().min(520.0) - 24.0;
        let painter = ui.painter();
        let galley = painter.layout(
            error.clone(),
            FontId::proportional(12.5),
            theme::TEXT,
            width - 44.0,
        );
        let size = Vec2::new(width, galley.size().y + 20.0);
        let rect = Rect::from_min_size(area.right_bottom() - size - Vec2::new(12.0, 12.0), size);
        painter.rect_filled(
            rect.translate(Vec2::new(0.0, 2.0)),
            10.0,
            Color32::from_black_alpha(90),
        );
        painter.rect_filled(rect, 10.0, Color32::from_rgb(0x3a, 0x22, 0x22));
        painter.rect_stroke(
            rect,
            10.0,
            egui::Stroke::new(1.0, theme::RED.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.min + Vec2::new(18.0, 18.0),
            egui::Align2::CENTER_CENTER,
            icon::WARNING_CIRCLE,
            FontId::proportional(16.0),
            theme::RED,
        );
        painter.galley(rect.min + Vec2::new(34.0, 10.0), galley, theme::TEXT);
        let response = ui
            .interact(rect, egui::Id::new("error"), Sense::click())
            .on_hover_text(tr!("Clic para cerrar"));
        if response.clicked() {
            self.error = None;
        }
    }
}
