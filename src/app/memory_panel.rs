// Panel de memoria del proyecto (⌘⇧M).
use super::*;
use forge_core::tr;

fn highlighted(text: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut active = false;
    for part in text.split_inclusive(['[', ']']) {
        let content = part.trim_end_matches(['[', ']']);
        if !content.is_empty() {
            job.append(
                content,
                0.0,
                egui::TextFormat::simple(
                    FontId::proportional(12.0),
                    if active {
                        theme::YELLOW
                    } else {
                        Color32::from_gray(0xa0)
                    },
                ),
            );
        }
        if part.ends_with('[') {
            active = true;
        }
        if part.ends_with(']') {
            active = false;
        }
    }
    job
}

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
                RichText::new("Forge Memory")
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
                tr!("Compartida con los agentes que abres desde Forge (MCP).").to_string(),
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

        // Buscador y tipo; en un panel estrecho, uno debajo del otro.
        ui.horizontal_wrapped(|ui| {
            let search = ui.add(
                theme::field(egui::TextEdit::singleline(&mut ws.memory.query))
                    .hint_text(tr!(
                        "{p0}  Buscar (sin tildes también)",
                        p0 = icon::MAGNIFYING_GLASS
                    ))
                    // Sitio para los dos desplegables (tipo y etiquetas).
                    .desired_width((ui.available_width() - 270.0).clamp(140.0, 320.0)),
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
            // Misma altura que el buscador de al lado.
            ui.spacing_mut().interact_size.y = 34.0;
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
            // Etiquetas en un desplegable (con muchas, una fila de botones llenaba el panel).
            if !ws.memory.tags.is_empty() {
                let selected = if ws.memory.tag.is_empty() {
                    tr!("Etiquetas").to_string()
                } else {
                    format!("#{}", ws.memory.tag)
                };
                egui::ComboBox::from_id_salt("memory-tag")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                if ui
                                    .selectable_label(ws.memory.tag.is_empty(), tr!("Todas"))
                                    .clicked()
                                {
                                    ws.memory.tag.clear();
                                    ws.memory.dirty = true;
                                }
                                for (tag, n) in ws.memory.tags.clone() {
                                    let on = ws.memory.tag == tag;
                                    if ui.selectable_label(on, format!("#{tag}  {n}")).clicked() {
                                        ws.memory.tag = tag;
                                        ws.memory.dirty = true;
                                    }
                                }
                            });
                    })
                    .response
                    .on_hover_text(tr!("Filtrar por etiqueta"));
            }
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
                        theme::field(egui::TextEdit::singleline(&mut form.title))
                            .hint_text(tr!("Título"))
                            .desired_width(f32::INFINITY),
                    );
                    ui.add(
                        theme::field(egui::TextEdit::multiline(&mut form.body))
                            .hint_text(tr!(
                                "Qué y por qué (p. ej. causa del error y cómo se resolvió)"
                            ))
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add(
                        theme::field(egui::TextEdit::singleline(&mut form.tags))
                            .hint_text(tr!("Etiquetas, separadas por comas"))
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !form.title.trim().is_empty(),
                                theme::primary_button(tr!("Guardar")),
                            )
                            .clicked()
                        {
                            cmds.push(UiCmd::MemorySave);
                        }
                        cancel = theme::secondary(ui, tr!("Cancelar")).clicked();
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
                for (kind, label) in KINDS {
                    let entries: Vec<_> = ws
                        .memory
                        .results
                        .iter()
                        .filter(|m| m.kind == kind)
                        .collect();
                    if entries.is_empty() {
                        continue;
                    }
                    ui.label(
                        RichText::new(forge_core::i18n::t(label))
                            .small()
                            .color(theme::TEXT_3),
                    );
                    for m in entries {
                        // El tipo ya lo dice el grupo; al lado, si los agentes la usan.
                        let read = ws.memory.reads.get(&m.id).copied();
                        // Un título largo no se sale del panel: se corta con «…» (entero al
                        // abrir la nota). El encabezado plegable no parte líneas.
                        let room = ((ui.available_width() - 110.0) / 7.0).max(12.0) as usize;
                        let title = if m.title.chars().count() > room {
                            format!("{}…", m.title.chars().take(room - 1).collect::<String>())
                        } else {
                            m.title.clone()
                        };
                        let mut header = egui::text::LayoutJob::default();
                        header.append(
                            &title,
                            0.0,
                            egui::TextFormat::simple(FontId::proportional(13.0), theme::TEXT),
                        );
                        header.append(
                            &format!(
                                "{}{}",
                                match read {
                                    Some((1, _)) => tr!("1 lectura").into(),
                                    Some((n, _)) => tr!("{n} lecturas", n = n),
                                    None => tr!("sin leer").into(),
                                },
                                forge_core::memory::status_note(m)
                            ),
                            8.0,
                            egui::TextFormat::simple(FontId::proportional(11.0), theme::TEXT_3),
                        );
                        let cut = title != m.title;
                        let note = egui::CollapsingHeader::new(header)
                            .id_salt(("memory", m.id))
                            .show(ui, |ui| {
                                if cut {
                                    ui.add(
                                        egui::Label::new(RichText::new(&m.title).strong()).wrap(),
                                    );
                                }
                                if let Some(snippet) = &m.snippet {
                                    ui.add(egui::Label::new(highlighted(snippet)).wrap());
                                }
                                ui.add(egui::Label::new(&m.body).wrap());
                                let mut tags = if m.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {}", m.tags)
                                };
                                if let Some((_, last)) = read {
                                    tags.push_str(&tr!(
                                        " · leída por última vez {p0}",
                                        p0 = store::ago_precise(last)
                                    ));
                                }
                                ui.horizontal_wrapped(|ui| {
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
                                    // Archivar en vez de borrar: deja de llegar a los
                                    // agentes y se puede restaurar.
                                    let (label, status) = if m.status == "active" {
                                        (tr!("Archivar"), "archived")
                                    } else {
                                        (tr!("Restaurar"), "active")
                                    };
                                    if ui.small_button(label).clicked() {
                                        cmds.push(UiCmd::MemoryStatus(m.id, status));
                                    }
                                    if ui.small_button(tr!("Borrar")).clicked() {
                                        cmds.push(UiCmd::MemoryDelete(m.id));
                                    }
                                });
                            });
                        if cut {
                            note.header_response.on_hover_text(&m.title);
                        }
                    }
                }
            });
    }
}
