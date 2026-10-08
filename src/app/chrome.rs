// Marco de la ventana: inicio, barra de herramientas, barra de estado y avisos.
use super::*;

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
            RichText::new("Proyectos con sus terminales, agentes, procesos y Project Guard.")
                .size(14.0)
                .color(theme::TEXT_3),
        );
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, format!("{}  Abrir proyecto…", icon::FOLDER_PLUS))
                .on_hover_text("⌘O")
                .clicked()
            {
                cmds.push(UiCmd::OpenFolder);
            }
            ui.label(RichText::new("o arrastra una carpeta a la ventana").color(theme::TEXT_3));
        });
        ui.add_space(26.0);
        if recent.is_empty() {
            return;
        }
        ui.label(
            RichText::new("Recientes")
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
                                format!("{} · carpeta no encontrada", tilde(&row.path))
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
                                    format!("● abierto · {running} en marcha")
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
                                    .button(format!("{}  Quitar de recientes", icon::TRASH))
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
                    RichText::new("Clic derecho en un proyecto para quitarlo de recientes.")
                        .size(11.0)
                        .color(theme::TEXT_4),
                );
            });
    }

    /// Barra de herramientas unificada: proyecto y rama, estado del Guard, RAM y acciones.
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
        ui.painter().rect_filled(rect, 0.0, theme::TOOLBAR);
        theme::hairline(ui, rect, false);
        // Sin barra lateral, los semáforos quedan sobre la barra de herramientas.
        let left = if self.sidebar { 12.0 } else { 84.0 };
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
        let tip = if self.sidebar {
            "Ocultar barra lateral (⌘B)"
        } else {
            "Mostrar barra lateral (⌘B)"
        };
        if theme::icon_button(&mut ui, icon::SIDEBAR_SIMPLE, tip).clicked() {
            self.sidebar = !self.sidebar;
        }
        ui.add_space(6.0);
        let Some(i) = self.active else {
            ui.label(
                RichText::new("Inicio")
                    .size(14.0)
                    .color(theme::TEXT)
                    .strong(),
            );
            return;
        };
        let ws = &mut self.workspaces[i];
        ui.label(
            RichText::new(ws.project.name())
                .size(14.0)
                .color(theme::TEXT)
                .strong(),
        );
        if let Some(branch) = ws.branch() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("{}  {branch}", icon::GIT_BRANCH))
                    .size(12.0)
                    .color(theme::TEXT_3),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::icon_button(ui, icon::BRAIN, "Memoria del proyecto (⌘⇧M)").clicked() {
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
            if theme::icon_button(ui, icon::SQUARE_SPLIT_VERTICAL, "Dividir abajo (⌘⇧D)").clicked()
            {
                cmds.push(UiCmd::Ws(WsAction::Split(Dir::Column)));
            }
            if theme::icon_button(
                ui,
                icon::SQUARE_SPLIT_HORIZONTAL,
                "Dividir a la derecha (⌘D)",
            )
            .clicked()
            {
                cmds.push(UiCmd::Ws(WsAction::Split(Dir::Row)));
            }
            if theme::icon_button(ui, icon::TERMINAL_WINDOW, "Nueva terminal (⌘T)").clicked() {
                cmds.push(UiCmd::Ws(WsAction::NewTerminal));
            }
            if let Some(bytes) = *self.ram.lock().unwrap() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("{}  {}", icon::MEMORY, human_bytes(bytes)))
                        .size(12.0)
                        .color(theme::TEXT_3),
                )
                .on_hover_text(
                    "Memoria de los procesos de este proyecto (terminales, agentes y procesos)",
                );
            }
        });
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
            "⌘T Terminal    ⌘D Dividir    ⌘⇧D Abajo    ⌘⌥← → Foco    ⌘G Guard    ⌘⇧M Memoria    ⌘⇧A Agente"
        } else {
            "⌘O Abrir carpeta    ⌘1…9 Proyectos    ⌘B Barra lateral"
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
    pub(super) fn ram_tick(&mut self, ctx: &egui::Context) {
        let Some(i) = self.active else {
            *self.ram.lock().unwrap() = None;
            return;
        };
        if self
            .ram_checked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
        {
            return;
        }
        self.ram_checked = Some(Instant::now());
        let (pids, ram, ctx) = (self.workspaces[i].pids(), self.ram.clone(), ctx.clone());
        std::thread::spawn(move || {
            let bytes = processes::memory_usage(&pids);
            *ram.lock().unwrap() = Some(bytes);
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
            .on_hover_text("Clic para cerrar");
        if response.clicked() {
            self.error = None;
        }
    }
}
