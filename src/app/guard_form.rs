// Ajustes → Este proyecto → Project Guard: las reglas de `.forge/rules.toml` en un
// formulario. Se guarda solo poco después de cada cambio (sin botón de guardar).
use super::*;
use forge_core::guard::{CheckForm, RulesForm, STANDARD_CHECKS};

/// Tras este rato sin cambios, se guarda.
const SAVE_AFTER: f64 = 0.8;

/// Un formulario de Ajustes que se guarda solo: lo editado, lo último guardado y el estado.
pub(super) struct Autosave<T> {
    pub(super) project: PathBuf,
    pub(super) form: T,
    /// Lo último guardado (o leído): si `form` difiere, hay cambios por guardar.
    saved: T,
    changed_at: f64,
    saved_at: Option<f64>,
    pub(super) error: Option<String>,
}

impl<T: Clone + PartialEq> Autosave<T> {
    pub(super) fn new(project: PathBuf, form: T) -> Self {
        Self {
            project,
            saved: form.clone(),
            form,
            changed_at: 0.0,
            saved_at: None,
            error: None,
        }
    }

    /// Tras un cambio (`changed`), espera un momento y guarda con `save` si `complete`.
    /// Devuelve si acaba de guardar.
    pub(super) fn tick(
        &mut self,
        now: f64,
        changed: bool,
        complete: bool,
        save: impl FnOnce(&Path, &T) -> Result<(), String>,
    ) -> bool {
        if changed {
            self.changed_at = now;
            self.error = None;
        }
        if self.form != self.saved && complete && now - self.changed_at > SAVE_AFTER {
            match save(&self.project, &self.form) {
                Ok(()) => {
                    self.saved = self.form.clone();
                    self.saved_at = Some(now);
                    return true;
                }
                Err(e) => self.error = Some(e),
            }
        }
        false
    }

    /// "Guardando…", "✓ Guardado" o el error.
    pub(super) fn status(&self, ui: &mut egui::Ui, now: f64, incomplete: &str) {
        let pending = self.form != self.saved;
        ui.add_space(4.0);
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).size(11.5).color(theme::RED));
        } else if pending && now - self.changed_at > SAVE_AFTER {
            ui.label(RichText::new(incomplete).size(11.5).color(theme::TEXT_3));
        } else if pending {
            ui.label(
                RichText::new(tr!("Guardando…"))
                    .size(11.5)
                    .color(theme::TEXT_3),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(300));
        } else if self.saved_at.is_some_and(|t| now - t < 2.5) {
            ui.label(
                RichText::new(tr!("✓ Guardado"))
                    .size(11.5)
                    .color(theme::GREEN),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

pub(super) type GuardForm = Autosave<RulesForm>;

/// Una casilla con su ⓘ.
fn check(ui: &mut egui::Ui, on: &mut bool, label: &str, help: &str) -> bool {
    ui.horizontal(|ui| {
        let changed = ui.checkbox(on, label).changed();
        theme::help(ui, help);
        changed
    })
    .inner
}

fn heading(ui: &mut egui::Ui, text: &str, help: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).strong().color(theme::TEXT));
        theme::help(ui, help);
    });
    ui.add_space(2.0);
}

fn number(ui: &mut egui::Ui, n: &mut usize, label: &str, help: &str) -> bool {
    ui.horizontal_wrapped(|ui| {
        let changed = ui
            .add(egui::DragValue::new(n).range(0..=1_000_000).speed(10.0))
            .changed();
        ui.label(label);
        theme::help(ui, help);
        changed
    })
    .inner
}

impl App {
    pub(super) fn guard_form_ui(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
        let Some(i) = self.active else { return };
        let project = self.workspaces[i].project.path.clone();
        if self
            .guard_form
            .as_ref()
            .is_none_or(|g| g.project != project)
        {
            self.guard_form = Some(match RulesForm::load(&project) {
                Ok(form) => GuardForm::new(project.clone(), form),
                Err(e) => {
                    // Un archivo que no se puede leer no se pisa: se dice y se ofrece abrirlo.
                    theme::section(ui, "Project Guard");
                    theme::card(ui, |ui| {
                        ui.label(
                            RichText::new(tr!("No se pudo leer .forge/rules.toml"))
                                .color(theme::RED),
                        );
                        ui.label(RichText::new(&e).size(11.5).color(theme::TEXT_3));
                        if theme::secondary(ui, tr!("Abrir archivo")).clicked() {
                            cmds.push(UiCmd::EditFile("rules.toml"));
                        }
                    });
                    return;
                }
            });
        }
        let now = ui.input(|i| i.time);
        let Some(g) = self.guard_form.as_mut() else {
            return;
        };
        let mut changed = false;
        theme::section(ui, "Project Guard");
        theme::card(ui, |ui| {
            ui.label(
                RichText::new(tr!("Qué revisa Forge antes de que un cambio salga de tu máquina. Se guarda solo en .forge/rules.toml."))
                    .size(11.5)
                    .color(theme::TEXT_3),
            );

            heading(
                ui,
                tr!("Qué se exige en cada paso"),
                tr!(
                    "Antes de un commit, un push o un pull request, Guard pasa estas comprobaciones. Lint, tipos, tests y build usan las comprobaciones de abajo con ese id."
                ),
            );
            let names = [
                tr!("Lint"),
                tr!("Tipos"),
                tr!("Tests"),
                tr!("Build"),
                tr!("Revisión con IA"),
            ];
            let helps = [
                tr!("Revisa el estilo y errores comunes (p. ej. eslint, clippy)."),
                tr!("Comprueba los tipos (p. ej. tsc, cargo check)."),
                tr!("Corre los tests."),
                tr!("Comprueba que el proyecto compila."),
                tr!("Un modelo revisa el cambio (configúralo en Revisión con IA). Gasta tokens."),
            ];
            let stages = forge_core::guard::Stage::ALL;
            let narrow = ui.available_width() < 480.0;
            if narrow {
                for (s, stage) in g.form.stages.iter_mut().zip(stages) {
                    ui.label(RichText::new(stage.label()).color(theme::TEXT_2));
                    ui.horizontal_wrapped(|ui| {
                        changed |= ui.checkbox(&mut s.enabled, tr!("Activado")).changed();
                        ui.add_enabled_ui(s.enabled, |ui| {
                            for ((on, name), help) in [
                                &mut s.lint,
                                &mut s.typecheck,
                                &mut s.tests,
                                &mut s.build,
                                &mut s.ai_review,
                            ]
                            .into_iter()
                            .zip(names)
                            .zip(helps)
                            {
                                changed |= ui.checkbox(on, name).on_hover_text(help).changed();
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            } else {
                egui::Grid::new("guard-stages")
                    .num_columns(4)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("");
                        for stage in stages {
                            ui.label(RichText::new(stage.label()).color(theme::TEXT_2));
                        }
                        ui.end_row();
                        ui.horizontal(|ui| {
                            ui.label(tr!("Activado"));
                            theme::help(
                                ui,
                                tr!("Si está apagado, Guard no revisa nada en ese paso."),
                            );
                        });
                        for s in &mut g.form.stages {
                            changed |= ui.checkbox(&mut s.enabled, "").changed();
                        }
                        ui.end_row();
                        for (row, (name, help)) in names.iter().zip(helps).enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(*name);
                                theme::help(ui, help);
                            });
                            for s in &mut g.form.stages {
                                let enabled = s.enabled;
                                let on = [
                                    &mut s.lint,
                                    &mut s.typecheck,
                                    &mut s.tests,
                                    &mut s.build,
                                    &mut s.ai_review,
                                ]
                                .into_iter()
                                .nth(row)
                                .expect("una casilla por fila");
                                changed |= ui
                                    .add_enabled(enabled, egui::Checkbox::without_text(on))
                                    .changed();
                            }
                            ui.end_row();
                        }
                    });
            }
            for id in g.form.missing_checks() {
                ui.label(
                    RichText::new(tr!("{p0} Se pide «{id}» pero no hay ninguna comprobación con ese id: añádela abajo o pulsa Detectar.", p0 = icon::WARNING, id = id))
                        .size(11.5)
                        .color(theme::ORANGE),
                );
            }

            heading(
                ui,
                tr!("Tamaño del cambio"),
                tr!(
                    "Cambios muy grandes son difíciles de revisar. Los lockfiles y archivos generados no cuentan."
                ),
            );
            changed |= number(
                ui,
                &mut g.form.max_changed_lines,
                tr!("líneas como máximo"),
                tr!("Por encima de esto, Guard bloquea el cambio."),
            );
            changed |= number(
                ui,
                &mut g.form.warning_changed_lines,
                tr!("líneas para avisar"),
                tr!("Por encima de esto, Guard avisa pero deja seguir."),
            );
            changed |= number(
                ui,
                &mut g.form.max_files_changed,
                tr!("archivos como máximo"),
                tr!("Cuántos archivos puede tocar un cambio."),
            );

            heading(
                ui,
                tr!("Secretos y archivos"),
                tr!("Para que no se suban por error contraseñas, claves ni archivos privados."),
            );
            changed |= check(
                ui,
                &mut g.form.block_secrets,
                tr!("Bloquear secretos"),
                tr!(
                    "Busca claves de API, tokens y contraseñas en el cambio y lo bloquea si encuentra alguna."
                ),
            );
            changed |= check(
                ui,
                &mut g.form.block_env_files,
                tr!("Bloquear archivos .env"),
                tr!("No deja subir archivos .env (suelen tener contraseñas). .env.example sí."),
            );
            ui.horizontal(|ui| {
                ui.label(tr!("Archivos prohibidos"));
                theme::help(
                    ui,
                    tr!("Uno por línea. Admite comodines: *.pem, secrets/**, id_rsa…"),
                );
            });
            let mut text = g.form.forbidden_files.join("\n");
            if ui
                .add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .code_editor(),
                )
                .changed()
            {
                g.form.forbidden_files = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect();
                changed = true;
            }

            heading(
                ui,
                tr!("Comprobaciones"),
                tr!(
                    "Comandos que Guard ejecuta, en una copia aparte del proyecto. Con id lint, typecheck, tests o build se usan según las casillas de arriba; con otro id, en los pasos que marques."
                ),
            );
            let mut remove = None;
            for (n, c) in g.form.checks.iter_mut().enumerate() {
                if n > 0 {
                    ui.separator();
                }
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .add(egui::TextEdit::singleline(&mut c.name).hint_text(tr!("Nombre")).desired_width(150.0))
                        .changed();
                    changed |= ui
                        .add(egui::TextEdit::singleline(&mut c.id).hint_text("id").desired_width(90.0))
                        .on_hover_text(tr!("lint, typecheck, tests o build para usarla en las casillas; cualquier otro para elegir los pasos"))
                        .changed();
                    changed |= ui
                        .checkbox(&mut c.blocking, tr!("Bloquea"))
                        .on_hover_text(tr!("Si falla, no deja seguir. Si no, solo avisa."))
                        .changed();
                    if theme::icon_button(ui, icon::TRASH, tr!("Quitar comprobación")).clicked() {
                        remove = Some(n);
                    }
                });
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut c.command)
                            .hint_text(tr!("Comando, p. ej. npm test"))
                            .desired_width(f32::INFINITY)
                            .code_editor(),
                    )
                    .changed();
                if !STANDARD_CHECKS.contains(&c.id.trim()) && !c.id.trim().is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(tr!("Corre en:"))
                                .size(11.5)
                                .color(theme::TEXT_3),
                        );
                        for stage in stages {
                            let mut on = c.stages.contains(&stage);
                            if ui.checkbox(&mut on, stage.label()).changed() {
                                c.stages.retain(|s| *s != stage);
                                if on {
                                    c.stages.push(stage);
                                }
                                changed = true;
                            }
                        }
                    });
                }
            }
            if let Some(n) = remove {
                g.form.checks.remove(n);
                changed = true;
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if theme::secondary(ui, format!("{}  {}", icon::PLUS, tr!("Añadir comprobación"))).clicked() {
                    g.form.checks.push(CheckForm::new());
                }
                if theme::secondary(ui, format!("{}  {}", icon::MAGNIFYING_GLASS, tr!("Detectar")))
                    .on_hover_text(tr!("Busca los scripts lint, typecheck, test y build de package.json, o los comandos de Cargo"))
                    .clicked()
                {
                    let added = g.form.detect(&g.project);
                    changed |= added > 0;
                    if added == 0 {
                        g.error = Some(tr!("No encontré comprobaciones nuevas.").into());
                    }
                }
                if theme::secondary(ui, tr!("Abrir archivo"))
                    .on_hover_text(tr!("Para opciones avanzadas: lo abre en tu editor de texto"))
                    .clicked()
                {
                    cmds.push(UiCmd::EditFile("rules.toml"));
                }
            });

            // Una comprobación a medio escribir (sin id o comando) espera a estar completa.
            let complete = g
                .form
                .checks
                .iter()
                .all(|c| !c.id.trim().is_empty() && !c.command.trim().is_empty());
            g.tick(now, changed, complete, |p, f| f.save(p));
            g.status(ui, now, tr!("Completa el id y el comando para guardar."));
        });
    }
}
