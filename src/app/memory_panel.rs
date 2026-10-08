// Panel de memoria del proyecto (⌘⇧M).
use super::*;
use forge_core::tr;

impl App {
    /// Panel de memoria: búsqueda, filtro por tipo, notas nuevas y estado de la conexión con agentes.
    pub(super) fn memory_ui(
        ws: &mut Workspace,
        ui: &mut egui::Ui,
        rect: Rect,
        cmds: &mut Vec<UiCmd>,
    ) {
        use forge_core::memory::{KINDS, kind_label};
        ui.painter().rect_filled(rect, 0.0, theme::SIDEBAR);
        ui.painter().vline(
            rect.min.x + 0.5,
            rect.y_range(),
            egui::Stroke::new(1.0, theme::SEPARATOR),
        );
        let mut ui =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(16.0, 14.0))));
        ui.spacing_mut().item_spacing.y = 8.0;

        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::BRAIN).size(18.0).color(theme::PURPLE));
            ui.label(
                RichText::new(tr!("Memoria"))
                    .size(15.0)
                    .color(theme::TEXT)
                    .strong(),
            );
            ui.label(
                RichText::new(tr!(
                    "{p0} · {p1} notas",
                    p0 = ws.project.name(),
                    p1 = ws.memory.count
                ))
                .size(13.0)
                .color(theme::TEXT_3),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, tr!("Cerrar (⌘⇧M)")).clicked() {
                    cmds.push(UiCmd::ToggleMemory);
                }
            });
        });

        // Conexión con los agentes.
        let memory = &ws.project.config.memory;
        let server = ws.mcp_server();
        let (sharing, color) = match &server {
            Some(_) if memory.provider == forge_core::project::MemoryProvider::Mcp => (
                tr!(
                    "Agentes conectados a la memoria externa: {p0}",
                    p0 = memory.command.as_deref().unwrap_or_default()
                ),
                tone_color(processes::Tone::Ok),
            ),
            Some(_) => (
                tr!("Compartida con Claude Code y Codex al abrirlos desde Forge (MCP).")
                    .to_string(),
                tone_color(processes::Tone::Ok),
            ),
            None => (
                tr!("No se comparte con los agentes ([memory] share_with_agents = false).")
                    .to_string(),
                Color32::from_gray(0x90),
            ),
        };
        ui.label(RichText::new(sharing).small().color(color));
        if let Some(server) = server {
            let command = std::iter::once(server.command.clone())
                .chain(server.args.clone())
                .collect::<Vec<_>>()
                .join(" ");
            if ui
                .link(RichText::new(tr!("{p0}  Copiar comando MCP", p0 = icon::COPY)).size(12.0))
                .on_hover_text(tr!(
                    "Para OpenCode u otros clientes (opencode mcp add):\n{command}",
                    command = command
                ))
                .clicked()
            {
                ui.ctx().copy_text(command);
            }
        }

        ui.horizontal(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut ws.memory.query)
                    .hint_text(tr!(
                        "{p0}  Buscar (sin tildes también)",
                        p0 = icon::MAGNIFYING_GLASS
                    ))
                    .desired_width(200.0),
            );
            if search.changed() {
                ws.memory.dirty = true;
            }
            let selected = ws
                .memory
                .kind
                .as_deref()
                .map_or(tr!("Todos"), kind_label)
                .to_string();
            egui::ComboBox::from_id_salt("memory-kind")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(ws.memory.kind.is_none(), tr!("Todos"))
                        .clicked()
                    {
                        ws.memory.kind = None;
                        ws.memory.dirty = true;
                    }
                    for (id, label) in KINDS {
                        if ui
                            .selectable_label(
                                ws.memory.kind.as_deref() == Some(id),
                                forge_core::i18n::t(label),
                            )
                            .clicked()
                        {
                            ws.memory.kind = Some(id.to_string());
                            ws.memory.dirty = true;
                        }
                    }
                });
        });
        if ws.memory.form.is_none()
            && theme::secondary(&mut ui, tr!("{p0}  Nueva nota", p0 = icon::PLUS)).clicked()
        {
            ws.memory.form = Some(NoteForm {
                kind: "decision".into(),
                ..Default::default()
            });
        }
        if let Some(form) = &mut ws.memory.form {
            let mut cancel = false;
            egui::Frame::new()
                .fill(Color32::from_gray(0x26))
                .corner_radius(6.0)
                .inner_margin(10.0)
                .show(&mut ui, |ui| {
                    egui::ComboBox::from_id_salt("note-kind")
                        .selected_text(kind_label(&form.kind).to_string())
                        .show_ui(ui, |ui| {
                            for (id, label) in KINDS {
                                ui.selectable_value(
                                    &mut form.kind,
                                    id.to_string(),
                                    forge_core::i18n::t(label),
                                );
                            }
                        });
                    ui.add(
                        egui::TextEdit::singleline(&mut form.title)
                            .hint_text(tr!("Título"))
                            .desired_width(f32::INFINITY),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut form.body)
                            .hint_text(tr!(
                                "Qué y por qué (p. ej. causa del error y cómo se resolvió)"
                            ))
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut form.tags)
                            .hint_text(tr!("Etiquetas, separadas por comas"))
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !form.title.trim().is_empty(),
                                egui::Button::new(tr!("Guardar")),
                            )
                            .clicked()
                        {
                            cmds.push(UiCmd::MemorySave);
                        }
                        cancel = ui.button(tr!("Cancelar")).clicked();
                    });
                });
            if cancel {
                ws.memory.form = None;
            }
        }
        ui.separator();

        if ws.memory.results.is_empty() {
            let text = if ws.memory.query.trim().is_empty() {
                tr!(
                    "Todavía no hay notas. Guarda decisiones, errores resueltos o convenciones; los agentes también pueden hacerlo."
                )
            } else {
                tr!("Sin resultados.")
            };
            ui.label(RichText::new(text).color(Color32::from_gray(0x90)));
        }
        egui::ScrollArea::vertical()
            .id_salt("memory-scroll")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                for m in &ws.memory.results {
                    let header = RichText::new(format!("[{}] {}", kind_label(&m.kind), m.title));
                    egui::CollapsingHeader::new(header)
                        .id_salt(("memory", m.id))
                        .show(ui, |ui| {
                            if let Some(snippet) = &m.snippet {
                                ui.label(
                                    RichText::new(snippet)
                                        .small()
                                        .italics()
                                        .color(Color32::from_gray(0xa0)),
                                );
                            }
                            ui.label(&m.body);
                            let tags = if m.tags.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", m.tags)
                            };
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "#{} · {} · {}{tags}",
                                        m.id,
                                        m.source,
                                        store::ago_precise(m.created_at)
                                    ))
                                    .small()
                                    .color(Color32::from_gray(0x80)),
                                );
                                if ui.small_button(tr!("Borrar")).clicked() {
                                    cmds.push(UiCmd::MemoryDelete(m.id));
                                }
                            });
                        });
                }
            });
    }
}
