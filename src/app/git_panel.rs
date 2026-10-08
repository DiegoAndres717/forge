// Panel de Git (⌘⇧G): rama, cambios preparados y sin preparar, commit, push/pull e
// historial. Lo que tarda o puede pedir algo (commit con hooks, push, pull, diff) se
// ejecuta en un panel de terminal para ver su salida.
use super::*;
use forge_core::git::FileChange;

/// Color y letra del estado de un archivo.
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

impl App {
    /// Relee el estado de Git del proyecto activo cada pocos segundos (en otro hilo).
    pub(super) fn git_tick(&mut self, ctx: &egui::Context) {
        let Some(i) = self.active else { return };
        let ws = &mut self.workspaces[i];
        if let Some(rx) = &ws.git.loading
            && let Ok(result) = rx.try_recv()
        {
            ws.git.loading = None;
            ws.git.loaded = Some(Instant::now());
            match result {
                Ok(data) => {
                    ws.git.data = Some(data);
                    ws.git.error = None;
                }
                Err(e) => ws.git.error = Some(e),
            }
        }
        let stale = ws.git.dirty
            || ws
                .git
                .loaded
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3));
        if !ws.git.open || ws.git.loading.is_some() || !stale {
            return;
        }
        ws.git.dirty = false;
        let (tx, rx) = channel();
        let (path, repaint) = (ws.project.path.clone(), ctx.clone());
        std::thread::spawn(move || {
            let data = forge_core::git::status(&path).map(|s| {
                let branches = forge_core::git::branches(&path).unwrap_or_default();
                let log = forge_core::git::log(&path, 20).unwrap_or_default();
                (s, branches, log)
            });
            let _ = tx.send(data);
            repaint.request_repaint();
        });
        ws.git.loading = Some(rx);
        ctx.request_repaint_after(Duration::from_secs(3));
    }

    pub(super) fn git_ui(ws: &mut Workspace, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
        ui.painter().rect_filled(rect, 0.0, theme::SIDEBAR);
        ui.painter().vline(
            rect.min.x + 0.5,
            rect.y_range(),
            egui::Stroke::new(1.0, theme::SEPARATOR),
        );
        let mut ui =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(16.0, 14.0))));
        ui.spacing_mut().item_spacing.y = 8.0;
        let git = &mut ws.git;

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icon::GIT_BRANCH)
                    .size(18.0)
                    .color(theme::ORANGE),
            );
            ui.label(RichText::new("Git").size(15.0).color(theme::TEXT).strong());
            if let Some((status, branches, _)) = &git.data {
                let current = status
                    .branch
                    .clone()
                    .unwrap_or_else(|| tr!("HEAD separado").to_string());
                egui::ComboBox::from_id_salt("git-branch")
                    .selected_text(RichText::new(&current).size(12.5))
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for b in branches {
                            if ui
                                .selectable_label(Some(b) == status.branch.as_ref(), b)
                                .clicked()
                                && Some(b) != status.branch.as_ref()
                            {
                                cmds.push(UiCmd::GitSwitch(b.clone()));
                            }
                        }
                    });
                if status.upstream.is_some() {
                    ui.label(
                        RichText::new(format!("↑{} ↓{}", status.ahead, status.behind))
                            .size(12.0)
                            .color(theme::TEXT_3),
                    )
                    .on_hover_text(tr!("Commits por subir ↑ y por bajar ↓"));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, tr!("Cerrar (⌘⇧G)")).clicked() {
                    cmds.push(UiCmd::ToggleGit);
                }
            });
        });
        ui.horizontal(|ui| {
            if theme::secondary(ui, format!("{}  Pull", icon::ARROW_DOWN)).clicked() {
                cmds.push(UiCmd::GitRun("git pull"));
            }
            if theme::secondary(ui, format!("{}  Push", icon::ARROW_UP)).clicked() {
                cmds.push(UiCmd::GitRun("git push"));
            }
            if git.new_branch.is_none()
                && theme::secondary(ui, format!("{}  {}", icon::PLUS, tr!("Nueva rama"))).clicked()
            {
                git.new_branch = Some(String::new());
            }
        });
        if let Some(name) = &mut git.new_branch {
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text(tr!("nombre-de-la-rama"))
                        .desired_width(200.0),
                );
                edit.request_focus();
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if (ui.button(tr!("Crear")).clicked() || enter) && !name.trim().is_empty() {
                    cmds.push(UiCmd::GitNewBranch(name.trim().to_string()));
                }
                if ui.button(tr!("Cancelar")).clicked() || ui.input(|i| i.key_pressed(Key::Escape))
                {
                    cmds.push(UiCmd::GitNewBranchCancel);
                }
            });
        }
        if let Some(e) = &git.error {
            ui.label(RichText::new(e).size(12.0).color(theme::RED));
        }
        let Some((status, _, log)) = &git.data else {
            ui.spinner();
            return;
        };
        let staged: Vec<&FileChange> = status.files.iter().filter(|f| f.is_staged()).collect();
        let unstaged: Vec<&FileChange> = status.files.iter().filter(|f| f.is_unstaged()).collect();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                // Preparados (lo que entra en el commit).
                ui.horizontal(|ui| {
                    theme::section(ui, &tr!("Preparados ({n})", n = staged.len()));
                    if !staged.is_empty() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(tr!("Quitar todo")).clicked() {
                                cmds.push(UiCmd::GitUnstage(
                                    staged.iter().map(|f| f.path.clone()).collect(),
                                ));
                            }
                        });
                    }
                });
                for f in &staged {
                    Self::git_row(ui, f, f.staged, true, cmds);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    theme::section(ui, &tr!("Cambios ({n})", n = unstaged.len()));
                    if !unstaged.is_empty() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(tr!("Preparar todo")).clicked() {
                                cmds.push(UiCmd::GitStage(
                                    unstaged.iter().map(|f| f.path.clone()).collect(),
                                ));
                            }
                        });
                    }
                });
                if status.files.is_empty() {
                    ui.label(
                        RichText::new(tr!("Sin cambios: el árbol de trabajo está limpio."))
                            .color(theme::TEXT_3),
                    );
                }
                for f in &unstaged {
                    Self::git_row(ui, f, f.unstaged, false, cmds);
                }

                // Commit.
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.add(
                    egui::TextEdit::multiline(&mut git.message)
                        .hint_text(tr!("Mensaje del commit"))
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                let ready = !staged.is_empty() && !git.message.trim().is_empty();
                ui.horizontal(|ui| {
                    let label = tr!("{p0}  Commit ({n})", p0 = icon::CHECK, n = staged.len());
                    if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                        cmds.push(UiCmd::GitCommit);
                    }
                    if staged.is_empty() && !unstaged.is_empty() {
                        ui.label(
                            RichText::new(tr!("Prepara archivos con +"))
                                .size(11.5)
                                .color(theme::TEXT_3),
                        );
                    }
                });

                // Historial.
                ui.add_space(10.0);
                theme::section(ui, tr!("Historial"));
                for c in log {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&c.hash)
                                .monospace()
                                .size(11.0)
                                .color(theme::ACCENT),
                        );
                        ui.add(egui::Label::new(RichText::new(&c.subject).size(12.5)).truncate())
                            .on_hover_text(format!("{} · {}", c.author, store::ago(c.when)));
                    });
                }
            });

        // Confirmación para descartar (no se puede deshacer).
        if let Some(file) = git.confirm_discard.clone() {
            let modal = egui::Modal::new(egui::Id::new("git-discard")).show(ui.ctx(), |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new(tr!("¿Descartar los cambios?")).strong());
                ui.label(&file.path);
                ui.label(
                    RichText::new(tr!("No se puede deshacer."))
                        .size(12.0)
                        .color(theme::TEXT_3),
                );
                ui.horizontal(|ui| {
                    if theme::primary(ui, tr!("Descartar")).clicked() {
                        cmds.push(UiCmd::GitDiscard(file.clone()));
                    }
                    if theme::secondary(ui, tr!("Cancelar")).clicked() {
                        git.confirm_discard = None;
                    }
                });
            });
            if modal.should_close() {
                git.confirm_discard = None;
            }
        }
    }

    fn git_row(ui: &mut egui::Ui, f: &FileChange, code: char, staged: bool, cmds: &mut Vec<UiCmd>) {
        let (letter, color) = badge(code);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(letter.to_string())
                    .monospace()
                    .strong()
                    .color(color),
            );
            let (dir, name) = f
                .path
                .rsplit_once('/')
                .map_or(("", f.path.as_str()), |(d, n)| (d, n));
            let text = RichText::new(name).size(12.5);
            let response = ui.add(egui::Label::new(text).truncate().sense(Sense::click()));
            if !dir.is_empty() {
                ui.label(RichText::new(dir).size(11.0).color(theme::TEXT_4));
            }
            if response.on_hover_text(tr!("Ver diff")).clicked() {
                cmds.push(UiCmd::GitDiff(f.path.clone(), staged));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let small = |ui: &mut egui::Ui, glyph: &str, tip: &str| {
                    theme::icon_button_sized(ui, glyph, tip, 20.0, theme::TEXT_3).clicked()
                };
                if staged {
                    if small(ui, icon::MINUS, tr!("Quitar de preparados")) {
                        cmds.push(UiCmd::GitUnstage(vec![f.path.clone()]));
                    }
                } else {
                    if small(ui, icon::PLUS, tr!("Preparar")) {
                        cmds.push(UiCmd::GitStage(vec![f.path.clone()]));
                    }
                    if small(ui, icon::ARROW_COUNTER_CLOCKWISE, tr!("Descartar cambios")) {
                        cmds.push(UiCmd::GitDiscardAsk(f.clone()));
                    }
                }
            });
        });
    }

    /// Acciones del panel de Git.
    pub(super) fn apply_git(&mut self, ctx: &egui::Context, cmd: UiCmd, area: Rect) {
        let Some(i) = self.active else { return };
        let repo = self.workspaces[i].project.path.clone();
        let ws = &mut self.workspaces[i];
        ws.git.dirty = true;
        let result = match cmd {
            UiCmd::GitStage(paths) => {
                forge_core::git::stage(&repo, &paths.iter().map(String::as_str).collect::<Vec<_>>())
            }
            UiCmd::GitUnstage(paths) => forge_core::git::unstage(
                &repo,
                &paths.iter().map(String::as_str).collect::<Vec<_>>(),
            ),
            UiCmd::GitDiscardAsk(file) => {
                ws.git.confirm_discard = Some(file);
                Ok(())
            }
            UiCmd::GitDiscard(file) => {
                ws.git.confirm_discard = None;
                forge_core::git::discard(&repo, &file)
            }
            UiCmd::GitSwitch(branch) => forge_core::git::switch(&repo, &branch),
            UiCmd::GitNewBranch(name) => forge_core::git::create_branch(&repo, &name).map(|_| {
                ws.git.new_branch = None;
            }),
            UiCmd::GitNewBranchCancel => {
                ws.git.new_branch = None;
                Ok(())
            }
            UiCmd::GitDiff(path, staged) => {
                let flag = if staged { " --cached" } else { "" };
                let command = format!("git diff{flag} -- {}", shell_quote(&path));
                Self::run_in_panel(ws, ctx, "diff", command, area);
                Ok(())
            }
            UiCmd::GitRun(command) => {
                Self::run_in_panel(ws, ctx, command, command.to_string(), area);
                Ok(())
            }
            UiCmd::GitCommit => {
                // El mensaje va por archivo (sin problemas de comillas) en el directorio de Git.
                let message = std::mem::take(&mut ws.git.message);
                let path = repo.join(".git/FORGE_COMMIT_MSG");
                match std::fs::write(&path, message.trim()) {
                    Ok(()) => {
                        let command =
                            format!("git commit -F {}", shell_quote(&path.to_string_lossy()));
                        Self::run_in_panel(ws, ctx, "commit", command, area);
                        Ok(())
                    }
                    Err(e) => {
                        ws.git.message = message;
                        Err(e.to_string())
                    }
                }
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.workspaces[i].git.error = Some(e);
        }
    }

    fn run_in_panel(
        ws: &mut Workspace,
        ctx: &egui::Context,
        name: &str,
        command: String,
        area: Rect,
    ) {
        let command = SavedCommand {
            name: name.into(),
            command,
            working_directory: None,
        };
        ws.run_command(ctx, &command, area);
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
