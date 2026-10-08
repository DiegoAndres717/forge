// Ideas (⌘⇧I): pendientes por proyecto que el usuario y los agentes anotan y tachan.
// La lista general (sin proyecto) se muestra en Inicio con el mismo componente.
use super::*;
use crate::workspace::IdeasView;
use forge_core::tr;

/// Lista a la que se refiere un comando: `Some(i)` = workspace i, `None` = general.
pub(super) type IdeasTarget = Option<usize>;

impl App {
    pub(super) fn ideas_ui(
        ws: &mut Workspace,
        target: IdeasTarget,
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
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icon::LIGHTBULB)
                    .size(18.0)
                    .color(theme::YELLOW),
            );
            ui.label(
                RichText::new(tr!("Ideas"))
                    .size(15.0)
                    .color(theme::TEXT)
                    .strong(),
            );
            ui.label(
                RichText::new(tr!(
                    "{p0} · {p1} pendientes",
                    p0 = ws.project.name(),
                    p1 = ws.ideas.open_count
                ))
                .size(13.0)
                .color(theme::TEXT_3),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, tr!("Cerrar (⌘⇧I)")).clicked() {
                    cmds.push(UiCmd::ToggleIdeas);
                }
            });
        });
        ui.label(
            RichText::new(
                tr!("Se guardan en Forge, no en el repositorio. Claude Code y Codex abiertos desde Forge las leen, anotan y tachan."),
            )
            .size(11.5)
            .color(theme::TEXT_3),
        );
        egui::ScrollArea::vertical()
            .id_salt("ideas-scroll")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                Self::ideas_list(&mut ws.ideas, target, ui, cmds)
            });
    }

    /// Campo para anotar y la lista (abiertas y, si se piden, las hechas tachadas).
    pub(super) fn ideas_list(
        view: &mut IdeasView,
        target: IdeasTarget,
        ui: &mut egui::Ui,
        cmds: &mut Vec<UiCmd>,
    ) {
        let input = ui.add(
            egui::TextEdit::singleline(&mut view.input)
                .hint_text(tr!("Nueva idea… (Enter para anotar)"))
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut view.focus) {
            input.request_focus();
        }
        if input.lost_focus()
            && ui.input(|i| i.key_pressed(Key::Enter))
            && !view.input.trim().is_empty()
        {
            cmds.push(UiCmd::IdeaAdd(target));
            input.request_focus(); // para anotar varias seguidas
        }

        let done = view.items.iter().filter(|i| i.done()).count();
        if view.items.is_empty() {
            ui.label(
                RichText::new(tr!("Sin ideas por ahora. Anota aquí lo que no quieras olvidar, o pídele a Claude: «guarda esta idea en Forge»."))
                    .color(theme::TEXT_3),
            );
        }
        for idea in view.items.iter().filter(|i| !i.done()) {
            Self::idea_row(idea, &mut view.editing, target, ui, cmds);
        }
        if done > 0 {
            ui.add_space(4.0);
            let label = if view.show_done {
                tr!(
                    "{p0}  Ocultar hechas ({done})",
                    p0 = icon::CARET_DOWN,
                    done = done
                )
            } else {
                tr!(
                    "{p0}  Mostrar hechas ({done})",
                    p0 = icon::CARET_RIGHT,
                    done = done
                )
            };
            if ui
                .add(egui::Button::new(RichText::new(label).color(theme::TEXT_3)).frame(false))
                .clicked()
            {
                view.show_done = !view.show_done;
            }
            if view.show_done {
                for idea in view.items.iter().filter(|i| i.done()) {
                    Self::idea_row(idea, &mut view.editing, target, ui, cmds);
                }
            }
        }
    }

    fn idea_row(
        idea: &forge_core::ideas::Idea,
        editing: &mut Option<(i64, String, String)>,
        target: IdeasTarget,
        ui: &mut egui::Ui,
        cmds: &mut Vec<UiCmd>,
    ) {
        if let Some((_, title, note)) = editing.as_mut().filter(|e| e.0 == idea.id) {
            theme::card(ui, |ui| {
                ui.add(egui::TextEdit::singleline(title).desired_width(f32::INFINITY));
                ui.add(
                    egui::TextEdit::multiline(note)
                        .hint_text(tr!("Detalle (opcional)"))
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    if ui.button(tr!("Guardar")).clicked() {
                        cmds.push(UiCmd::IdeaSave(target));
                    }
                    if ui.button(tr!("Cancelar")).clicked() {
                        cmds.push(UiCmd::IdeaEdit(target, None));
                    }
                });
            });
            return;
        }
        let (glyph, color, tip, next) = match idea.status.as_str() {
            "done" => (
                icon::CHECK_CIRCLE,
                theme::GREEN,
                tr!("Volver a pendiente"),
                "pending",
            ),
            "doing" => (
                icon::CIRCLE_HALF,
                theme::ORANGE,
                tr!("Marcar como hecha"),
                "done",
            ),
            _ => (
                icon::CIRCLE,
                theme::TEXT_3,
                tr!("Marcar como hecha"),
                "done",
            ),
        };
        ui.horizontal_top(|ui| {
            if theme::icon_button_sized(ui, glyph, &format!("{tip}: {}", idea.title), 22.0, color)
                .clicked()
            {
                cmds.push(UiCmd::IdeaSet(target, idea.id, next));
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let mut title = RichText::new(&idea.title).color(theme::TEXT);
                if idea.done() {
                    title = title.strikethrough().color(theme::TEXT_4);
                }
                let response = ui.add(egui::Label::new(title).wrap().sense(Sense::click()));
                if !idea.note.is_empty() {
                    ui.label(RichText::new(&idea.note).size(12.0).color(theme::TEXT_3));
                }
                let mut meta = format!(
                    "#{} · {} · {}",
                    idea.id,
                    idea.source,
                    store::ago(idea.created_at)
                );
                if idea.status == "doing" {
                    meta += &tr!(" · en curso ({p0})", p0 = idea.updated_by);
                } else if idea.done() {
                    meta += &tr!(
                        " · hecha por {p0} {p1}",
                        p0 = idea.updated_by,
                        p1 = store::ago(idea.updated_at)
                    );
                }
                ui.label(RichText::new(meta).size(11.0).color(theme::TEXT_4));
                response.context_menu(|ui| {
                    for (status, label) in forge_core::ideas::STATUSES {
                        if status != idea.status && ui.button(forge_core::i18n::t(label)).clicked()
                        {
                            cmds.push(UiCmd::IdeaSet(target, idea.id, status));
                        }
                    }
                    ui.separator();
                    if ui
                        .button(tr!("{p0}  Editar", p0 = icon::PENCIL_SIMPLE))
                        .clicked()
                    {
                        cmds.push(UiCmd::IdeaEdit(target, Some(idea.id)));
                    }
                    if ui.button(tr!("{p0}  Borrar", p0 = icon::TRASH)).clicked() {
                        cmds.push(UiCmd::IdeaDelete(target, idea.id));
                    }
                });
                if response.double_clicked() {
                    cmds.push(UiCmd::IdeaEdit(target, Some(idea.id)));
                }
            });
        });
    }

    /// Relee las listas visibles (los agentes escriben desde otros procesos).
    pub(super) fn ideas_tick(&mut self) {
        let every = |view: &IdeasView, secs| {
            view.dirty
                || view
                    .loaded
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(secs))
        };
        match self.active {
            Some(i) => {
                let view = &self.workspaces[i].ideas;
                // Abierto: lista completa cada 2 s; cerrado: solo el contador cada 5 s.
                let (full, count) = (view.open && every(view, 2), !view.open && every(view, 5));
                if !(full || count) {
                    return;
                }
                let path = self.workspaces[i].project.path.clone();
                let open_count = self.db(|s| s.open_ideas(Some(&path))).unwrap_or(0);
                let items = full.then(|| {
                    self.db(|s| s.list_ideas(Some(&path), true))
                        .unwrap_or_default()
                });
                let view = &mut self.workspaces[i].ideas;
                if let Some(items) = items {
                    view.items = items;
                }
                (view.open_count, view.dirty, view.loaded) =
                    (open_count, false, Some(Instant::now()));
            }
            None if every(&self.general, 2) => {
                let items = self.db(|s| s.list_ideas(None, true)).unwrap_or_default();
                let view = &mut self.general;
                view.open_count = items.iter().filter(|i| !i.done()).count() as i64;
                (view.items, view.dirty, view.loaded) = (items, false, Some(Instant::now()));
            }
            None => {}
        }
    }

    /// Comandos de ideas (de un workspace o de la lista general).
    pub(super) fn apply_idea(&mut self, cmd: UiCmd) {
        let target = match &cmd {
            UiCmd::IdeaAdd(t) | UiCmd::IdeaSave(t) | UiCmd::IdeaEdit(t, _) => *t,
            UiCmd::IdeaSet(t, ..) | UiCmd::IdeaDelete(t, _) => *t,
            _ => return,
        };
        let path = target
            .and_then(|i| self.workspaces.get(i))
            .map(|w| w.project.path.clone());
        if target.is_some() && path.is_none() {
            return;
        }
        let project = path.as_deref();
        let view = match target {
            Some(i) => &mut self.workspaces[i].ideas,
            None => &mut self.general,
        };
        view.dirty = true;
        match cmd {
            UiCmd::IdeaAdd(_) => {
                let title = std::mem::take(&mut view.input);
                if self
                    .db(|s| s.add_idea(project, &title, "", "usuario"))
                    .is_none()
                {
                    self.ideas_view(target).input = title; // se conserva lo escrito
                }
            }
            UiCmd::IdeaEdit(_, id) => {
                view.editing = id.and_then(|id| {
                    let idea = view.items.iter().find(|i| i.id == id)?;
                    Some((id, idea.title.clone(), idea.note.clone()))
                });
            }
            UiCmd::IdeaSave(_) => {
                let Some((id, title, note)) = view.editing.take() else {
                    return;
                };
                if self
                    .db(|s| s.update_idea(project, id, None, Some(&title), Some(&note), "usuario"))
                    .is_none()
                {
                    self.ideas_view(target).editing = Some((id, title, note));
                }
            }
            UiCmd::IdeaSet(_, id, status) => {
                self.db(|s| s.update_idea(project, id, Some(status), None, None, "usuario"));
            }
            UiCmd::IdeaDelete(_, id) => {
                self.db(|s| s.delete_idea(project, id));
            }
            _ => {}
        }
    }

    fn ideas_view(&mut self, target: IdeasTarget) -> &mut IdeasView {
        match target {
            Some(i) => &mut self.workspaces[i].ideas,
            None => &mut self.general,
        }
    }
}
