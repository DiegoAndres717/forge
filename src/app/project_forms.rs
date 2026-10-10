// Ajustes → Este proyecto: agentes (`.forge/agents.toml`) y revisión con IA
// (`.forge/routing.toml`) en formularios que se guardan solos.
use super::guard_form::Autosave;
use super::*;
use forge_core::agents::{AgentForm, load_form, save_form};
use forge_core::router::{Profile, RoutingForm};

pub(super) type AgentsForm = Autosave<Vec<AgentForm>>;
pub(super) type RoutingSettings = Autosave<RoutingForm>;

fn heading(ui: &mut egui::Ui, text: &str, help: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).strong().color(theme::TEXT));
        theme::help(ui, help);
    });
    ui.add_space(2.0);
}

/// Lo que hace cada perfil (según `router::route` y `router::plan`).
fn profile_info(p: Profile) -> &'static str {
    match p {
        Profile::Economy => {
            tr!("Haiku revisa todo y nunca pasa a modelos más caros. Lo más barato.")
        }
        Profile::Balanced => tr!(
            "Haiku revisa el código; si el cambio es de riesgo alto, Sonnet revisa también la arquitectura o la seguridad."
        ),
        Profile::Quality => tr!(
            "Sonnet revisa siempre y, desde riesgo medio, Opus revisa la arquitectura o la seguridad. Gasta más."
        ),
    }
}

impl App {
    /// Agentes del proyecto: cuáles se ven, el predeterminado, su comando y los propios.
    pub(super) fn agents_form_ui(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
        let Some(i) = self.active else { return };
        let project = self.workspaces[i].project.path.clone();
        if self
            .agents_form
            .as_ref()
            .is_none_or(|g| g.project != project)
        {
            match load_form(&project) {
                Ok(form) => self.agents_form = Some(AgentsForm::new(project.clone(), form)),
                Err(e) => {
                    theme::section(ui, tr!("Agentes"));
                    theme::card(ui, |ui| {
                        ui.label(
                            RichText::new(tr!("No se pudo leer .forge/agents.toml"))
                                .color(theme::RED),
                        );
                        ui.label(RichText::new(&e).size(11.5).color(theme::TEXT_3));
                        if theme::secondary(ui, tr!("Abrir archivo")).clicked() {
                            cmds.push(UiCmd::EditFile("agents.toml"));
                        }
                    });
                    return;
                }
            }
        }
        let detected = self.agents.lock().map(|d| d.clone()).unwrap_or_default();
        let now = ui.input(|i| i.time);
        let Some(g) = self.agents_form.as_mut() else {
            return;
        };
        let mut changed = false;
        theme::section(ui, tr!("Agentes"));
        theme::card(ui, |ui| {
            ui.label(
                RichText::new(tr!("Qué agentes salen en la barra lateral de este proyecto y cómo se abren. Se guarda solo en .forge/agents.toml."))
                    .size(11.5)
                    .color(theme::TEXT_3),
            );
            let installed = |a: &AgentForm| {
                let program = a.command.split_whitespace().next().unwrap_or("");
                detected.get(program).is_some_and(|d| d.path.is_some())
            };
            // Los conocidos que no están instalados, plegados (no hacen falta casi nunca).
            let show_all_id = egui::Id::new("agents-show-all");
            let show_all = ui
                .data(|d| d.get_temp::<bool>(show_all_id))
                .unwrap_or(false);
            let hidden = g.form.iter().filter(|a| a.builtin && !installed(a)).count();
            // Sin ninguno marcado, el predeterminado es el primero activo e instalado.
            let auto_default = (!g.form.iter().any(|a| a.default))
                .then(|| g.form.iter().position(|a| a.enabled && installed(a)))
                .flatten();
            let mut remove = None;
            let mut make_default = None;
            for (n, a) in g.form.iter_mut().enumerate() {
                if a.builtin && !installed(a) && !show_all {
                    continue;
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .checkbox(&mut a.enabled, "")
                        .on_hover_text(tr!("Si lo desactivas, no sale en la barra lateral ni en ⌘K"))
                        .changed();
                    if a.builtin {
                        ui.label(RichText::new(&a.name).strong());
                    } else {
                        changed |= ui
                            .add(egui::TextEdit::singleline(&mut a.name).hint_text(tr!("Nombre")).desired_width(130.0))
                            .changed();
                        changed |= ui
                            .add(egui::TextEdit::singleline(&mut a.id).hint_text("id").desired_width(90.0))
                            .on_hover_text(tr!("Identificador sin espacios"))
                            .changed();
                    }
                    if a.builtin && !installed(a) {
                        ui.label(RichText::new(tr!("no instalado")).size(11.0).color(theme::TEXT_4));
                    }
                    let (label, tip) = if a.default {
                        (format!("{}  {}", icon::STAR, tr!("Predeterminado")), tr!("El que se abre con ⌘⇧A. Pulsa para quitarlo."))
                    } else if auto_default == Some(n) {
                        (format!("{}  {}", icon::STAR, tr!("Predeterminado (automático)")), tr!("Ninguno está elegido, así que se usa el primero instalado. Pulsa para fijarlo."))
                    } else {
                        (tr!("Hacer predeterminado").to_string(), tr!("Que sea el que se abre con ⌘⇧A y el botón de agente"))
                    };
                    if ui.selectable_label(a.default, label).on_hover_text(tip).clicked() {
                        make_default = Some((n, !a.default));
                    }
                    if !a.builtin && theme::icon_button(ui, icon::TRASH, tr!("Quitar agente")).clicked() {
                        remove = Some(n);
                    }
                });
                if a.enabled {
                    let field = |ui: &mut egui::Ui,
                                 text: &mut String,
                                 label: &str,
                                 hint: &str,
                                 help: &str| {
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                Vec2::new(80.0, 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(80.0);
                                    ui.label(RichText::new(label).size(11.5).color(theme::TEXT_3))
                                        .on_hover_text(help);
                                },
                            );
                            ui.add(
                                egui::TextEdit::singleline(text)
                                    .hint_text(hint)
                                    .desired_width(f32::INFINITY)
                                    .code_editor(),
                            )
                            .on_hover_text(help)
                            .changed()
                        })
                        .inner
                    };
                    changed |= field(
                        ui,
                        &mut a.command,
                        tr!("Abrir"),
                        tr!("p. ej. aider --model sonnet"),
                        tr!(
                            "Cómo se abre en la terminal; puedes añadir opciones, p. ej. claude --model opus"
                        ),
                    );
                    if !a.builtin || !a.resume_command.is_empty() {
                        changed |= field(
                            ui,
                            &mut a.resume_command,
                            tr!("Reanudar"),
                            tr!("opcional"),
                            tr!("Cómo seguir la última sesión (se usa al restaurar el proyecto)"),
                        );
                    }
                }
            }
            if let Some((n, on)) = make_default {
                for (k, a) in g.form.iter_mut().enumerate() {
                    a.default = on && k == n;
                }
                changed = true;
            }
            if let Some(n) = remove {
                g.form.remove(n);
                changed = true;
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if theme::secondary(
                    ui,
                    format!("{}  {}", icon::PLUS, tr!("Añadir agente propio")),
                )
                .on_hover_text(tr!(
                    "Cualquier programa de terminal: aider, goose, un script tuyo…"
                ))
                .clicked()
                {
                    g.form.push(AgentForm::custom());
                }
                if hidden > 0 {
                    let text = if show_all {
                        tr!("Ocultar los no instalados").to_string()
                    } else {
                        tr!("Mostrar los no instalados ({n})", n = hidden)
                    };
                    if ui.button(text).clicked() {
                        ui.data_mut(|d| d.insert_temp(show_all_id, !show_all));
                    }
                }
                if theme::secondary(ui, tr!("Abrir archivo"))
                    .on_hover_text(tr!(
                        "Para opciones avanzadas (variables de entorno…): lo abre en tu editor"
                    ))
                    .clicked()
                {
                    cmds.push(UiCmd::EditFile("agents.toml"));
                }
            });
            let complete = g
                .form
                .iter()
                .all(|a| !a.id.trim().is_empty() && !a.command.trim().is_empty());
            // Guardado: la barra lateral y ⌘K se actualizan al momento.
            if g.tick(now, changed, complete, |p, f| save_form(p, f)) {
                cmds.push(UiCmd::ReloadConfig);
            }
            g.status(ui, now, tr!("Completa el id y el comando para guardar."));
        });
    }

    /// Revisión con IA: perfil, presupuesto y topes.
    pub(super) fn routing_form_ui(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
        let Some(i) = self.active else { return };
        let project = self.workspaces[i].project.path.clone();
        if self
            .routing_form
            .as_ref()
            .is_none_or(|g| g.project != project)
        {
            match RoutingForm::load(&project) {
                Ok(form) => self.routing_form = Some(RoutingSettings::new(project.clone(), form)),
                Err(e) => {
                    theme::section(ui, tr!("Revisión con IA"));
                    theme::card(ui, |ui| {
                        ui.label(
                            RichText::new(tr!("No se pudo leer .forge/routing.toml"))
                                .color(theme::RED),
                        );
                        ui.label(RichText::new(&e).size(11.5).color(theme::TEXT_3));
                        if theme::secondary(ui, tr!("Abrir archivo")).clicked() {
                            cmds.push(UiCmd::EditFile("routing.toml"));
                        }
                    });
                    return;
                }
            }
        }
        // ¿Algún paso de Guard la pide? Si no, se dice: si no, nadie la usa.
        let used = self
            .guard_form
            .as_ref()
            .is_some_and(|g| g.form.stages.iter().any(|s| s.enabled && s.ai_review));
        let now = ui.input(|i| i.time);
        let Some(g) = self.routing_form.as_mut() else {
            return;
        };
        let mut changed = false;
        theme::section(ui, tr!("Revisión con IA"));
        theme::card(ui, |ui| {
            ui.label(
                RichText::new(tr!("Un modelo revisa cada cambio antes de que salga, según su riesgo. Se guarda solo en .forge/routing.toml."))
                    .size(11.5)
                    .color(theme::TEXT_3),
            );
            if !used {
                ui.label(
                    RichText::new(tr!("{p0} Ahora no se usa: actívala en «Revisión con IA» de algún paso de Project Guard (arriba).", p0 = icon::INFO))
                        .size(11.5)
                        .color(theme::ORANGE),
                );
            }
            heading(
                ui,
                tr!("Perfil"),
                tr!("Qué modelos revisan y cuándo se pasa a uno más capaz."),
            );
            ui.horizontal_wrapped(|ui| {
                for (p, name) in [
                    (Profile::Economy, tr!("Económico")),
                    (Profile::Balanced, tr!("Equilibrado")),
                    (Profile::Quality, tr!("Calidad")),
                ] {
                    if ui
                        .selectable_label(g.form.profile == p, name)
                        .on_hover_text(profile_info(p))
                        .clicked()
                    {
                        g.form.profile = p;
                        changed = true;
                    }
                }
            });
            ui.label(
                RichText::new(profile_info(g.form.profile))
                    .size(11.5)
                    .color(theme::TEXT_3),
            );

            heading(
                ui,
                tr!("Gasto"),
                tr!(
                    "Las revisiones con modelos de pago gastan dinero (o tu plan). Estos topes lo limitan."
                ),
            );
            let money = |ui: &mut egui::Ui, v: &mut f64, label: &str, help: &str| {
                ui.horizontal_wrapped(|ui| {
                    let r = ui.add(
                        egui::DragValue::new(v)
                            .range(0.0..=10_000.0)
                            .speed(0.1)
                            .prefix("$")
                            .fixed_decimals(2),
                    );
                    ui.label(label);
                    theme::help(ui, help);
                    r.changed()
                })
                .inner
            };
            changed |= money(
                ui,
                &mut g.form.monthly_budget_usd,
                tr!("al mes como máximo"),
                tr!("Al llegar a este gasto en el mes, Forge deja de llamar a modelos de pago."),
            );
            changed |= money(
                ui,
                &mut g.form.max_cost_per_call_usd,
                tr!("por revisión como máximo"),
                tr!("Tope de cada revisión (se pasa a Claude con --max-budget-usd)."),
            );

            heading(ui, tr!("Opciones"), tr!("Ajustes finos del perfil."));
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut g.form.prefer_local, tr!("Usar un modelo local si hay")).changed();
                theme::help(ui, tr!("Si tienes Ollama instalado, la primera clasificación del cambio se hace en tu Mac, gratis."));
            });
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut g.form.allow_escalation, tr!("Pasar a un modelo más capaz si hay riesgo")).changed();
                theme::help(ui, tr!("Un cambio de riesgo (pagos, seguridad, migraciones…) lo revisa además un modelo más capaz. No aplica en Económico."));
            });
            ui.add_space(4.0);
            if theme::secondary(ui, tr!("Abrir archivo"))
                .on_hover_text(tr!(
                    "Para elegir proveedor y modelo por tarea: lo abre en tu editor"
                ))
                .clicked()
            {
                cmds.push(UiCmd::EditFile("routing.toml"));
            }
            g.tick(now, changed, true, |p, f| f.save(p));
            g.status(ui, now, "");
        });
    }
}
