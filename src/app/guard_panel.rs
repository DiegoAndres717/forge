// Panel de Guard (⌘G): etapas, controles, excepciones, historial y su sondeo.
use super::*;
use forge_core::tr;

pub(super) fn level_icon(level: guard::Level) -> (&'static str, Color32) {
    match level {
        guard::Level::Pass => (icon::CHECK_CIRCLE, theme::GREEN),
        guard::Level::Warn => (icon::WARNING_CIRCLE, theme::YELLOW),
        guard::Level::Block => (icon::X_CIRCLE, theme::RED),
        guard::Level::Pending => (icon::CIRCLE, theme::TEXT_3),
        guard::Level::Info => (icon::INFO, theme::TEXT_3),
        guard::Level::Excepted => (icon::ARROW_BEND_DOWN_RIGHT, theme::ORANGE),
    }
}

/// Fila de estado: icono en su color, texto neutro y detalle en gris.
pub(super) fn status_text(
    glyph: &str,
    color: Color32,
    label: &str,
    detail: &str,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        glyph,
        0.0,
        egui::TextFormat::simple(FontId::proportional(14.0), color),
    );
    job.append(
        label,
        8.0,
        egui::TextFormat::simple(FontId::proportional(13.0), theme::TEXT),
    );
    if !detail.is_empty() {
        job.append(
            detail,
            8.0,
            egui::TextFormat::simple(FontId::proportional(12.0), theme::TEXT_3),
        );
    }
    job
}

/// Icono, color y texto de estado de un check (incluida la omisión con excepción).
pub(super) fn check_status(
    c: &guard::CheckRun,
    level: Option<guard::Level>,
    exception: Option<&guard::Exception>,
) -> (&'static str, Color32, String) {
    let bad = match c.check.severity {
        guard::Severity::Blocking => (icon::X_CIRCLE, theme::RED),
        guard::Severity::Warning => (icon::WARNING_CIRCLE, theme::YELLOW),
    };
    let (glyph, color, status) = match &c.state {
        guard::CheckState::Waiting => (icon::CIRCLE, theme::TEXT_4, tr!("en espera").to_string()),
        guard::CheckState::Running => {
            let secs = c.started.map_or(0, |s| s.elapsed().as_secs());
            (
                icon::CIRCLE_NOTCH,
                theme::ACCENT,
                tr!("ejecutándose · {secs} s", secs = secs),
            )
        }
        guard::CheckState::Passed => match c.reused {
            Some(at) => (
                icon::CHECK_CIRCLE,
                theme::GREEN,
                tr!(
                    "evidencia del mismo candidato ({p0})",
                    p0 = store::ago_precise(at)
                ),
            ),
            None => (icon::CHECK_CIRCLE, theme::GREEN, seconds(c.duration)),
        },
        guard::CheckState::Failed(code) => (
            bad.0,
            bad.1,
            tr!(
                "código {code} · {p0}",
                p0 = seconds(c.duration),
                code = code
            ),
        ),
        guard::CheckState::TimedOut => (
            bad.0,
            bad.1,
            format!("tiempo agotado ({} s)", c.check.timeout_seconds),
        ),
        guard::CheckState::Cancelled => (icon::CIRCLE, theme::TEXT_4, tr!("cancelado").to_string()),
        guard::CheckState::Error(e) => (bad.0, bad.1, e.clone()),
    };
    match exception.filter(|_| level == Some(guard::Level::Excepted)) {
        Some(e) => (
            icon::ARROW_BEND_DOWN_RIGHT,
            theme::ORANGE,
            format!("{status} · {}", e.summary()),
        ),
        None => (glyph, color, status),
    }
}

/// Selector de etapa segmentado (como NSSegmentedControl).
pub(super) fn segmented(ui: &mut egui::Ui, current: Stage, cmds: &mut Vec<UiCmd>) {
    let height = 28.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, theme::FIELD);
    let width = rect.width() / Stage::ALL.len() as f32;
    for (n, stage) in Stage::ALL.into_iter().enumerate() {
        let r = Rect::from_min_size(
            egui::pos2(rect.min.x + width * n as f32, rect.min.y),
            Vec2::new(width, height),
        )
        .shrink(2.0);
        let response = ui.interact(r, egui::Id::new(("stage", n)), Sense::click());
        let selected = stage == current;
        if selected {
            ui.painter().rect_filled(r, 6.0, theme::SURFACE_HOVER);
        } else if response.hovered() {
            ui.painter().rect_filled(r, 6.0, theme::SELECTED);
        }
        let color = if selected { theme::TEXT } else { theme::TEXT_3 };
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            stage.label(),
            FontId::proportional(12.5),
            color,
        );
        if response.clicked() && !selected {
            cmds.push(UiCmd::GuardStage(stage));
        }
    }
}

pub(super) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

pub(super) fn verdict_text(verdict: guard::Verdict, stage: Stage) -> (String, Color32) {
    let target = match stage {
        Stage::Commit => "commit",
        Stage::Push => "push",
        Stage::PullRequest => tr!("pull request"),
    };
    match verdict {
        guard::Verdict::Ready => (tr!("listo para {target}", target = target), theme::GREEN),
        guard::Verdict::Warnings => (
            tr!("listo para {target} con advertencias", target = target),
            theme::YELLOW,
        ),
        guard::Verdict::Blocked => (tr!("bloqueado").into(), theme::RED),
        guard::Verdict::Running => (tr!("revisando…").into(), theme::ACCENT),
        guard::Verdict::Pending => (tr!("pendiente de revisión").into(), theme::TEXT_3),
        guard::Verdict::NothingToCheck => (tr!("sin cambios").into(), theme::TEXT_3),
    }
}

/// Escribe el reporte en `.forge/reports/` (ignorado por Git) y devuelve el aviso.
pub(super) fn export_report(
    project: &Path,
    report: &forge_core::evidence::Report,
) -> Result<String, String> {
    let dir = project.join(".forge/reports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n").map_err(|e| e.to_string())?;
    }
    let date: String = forge_core::evidence::utc(report.finished_at)
        .chars()
        .filter_map(|c| match c {
            '0'..='9' => Some(c),
            ' ' => Some('_'),
            '-' => Some('-'),
            _ => None,
        })
        .collect();
    let candidate = report
        .candidate
        .as_ref()
        .map_or("sin-candidato", |c| c.id.as_str());
    let file = dir.join(format!(
        "{}-{}-{candidate}.md",
        date.trim_end_matches('_'),
        report.stage.as_str()
    ));
    std::fs::write(&file, report.markdown()).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(format!("Reporte exportado: {}", file.display()))
}

/// Estado corto del Guard para barras y listas.
pub(super) fn verdict_short(verdict: guard::Verdict) -> (String, Color32) {
    match verdict {
        guard::Verdict::Ready => (tr!("Listo").into(), theme::GREEN),
        guard::Verdict::Warnings => (tr!("Con avisos").into(), theme::YELLOW),
        guard::Verdict::Blocked => (tr!("Bloqueado").into(), theme::RED),
        guard::Verdict::Running => (tr!("Revisando…").into(), theme::ACCENT),
        guard::Verdict::Pending => (tr!("Pendiente").into(), theme::TEXT_3),
        guard::Verdict::NothingToCheck => (tr!("Sin cambios").into(), theme::TEXT_3),
    }
}

/// 3.2 s → "3,2 s"
pub(super) fn seconds(d: Option<Duration>) -> String {
    format!("{:.1} s", d.unwrap_or_default().as_secs_f32()).replace('.', ",")
}

pub(super) fn human_secs(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("{} h", secs / 3600),
    }
}

impl App {
    /// Guarda las ejecuciones terminadas (evidencia + historial) y vigila si el candidato
    /// cambia después de validar.
    pub(super) fn guard_tick(&mut self, ctx: &egui::Context) {
        let mut to_save = Vec::new();
        for ws in &mut self.workspaces {
            let Some(run) = &ws.guard.run else { continue };
            let (finished, frozen, stage, unstaged) =
                run.snapshot(|r| (r.finished(), r.frozen.clone(), r.stage, r.unstaged));
            let Some(frozen) = frozen.filter(|_| finished) else {
                continue;
            };
            if !ws.guard.saved {
                ws.guard.saved = true;
                ws.guard.history = None;
                let (report, evidence, usage) =
                    run.snapshot(|r| (r.report(), r.new_evidence(), r.usage.clone()));
                to_save.push((ws.project.path.clone(), report, evidence, usage));
            }
            if let Some(rx) = &ws.guard.stale_rx {
                if let Ok(changed) = rx.try_recv() {
                    ws.guard.stale |= changed;
                    ws.guard.stale_rx = None;
                }
            } else if !ws.guard.stale
                && ws
                    .guard
                    .stale_checked
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
            {
                ws.guard.stale_checked = Some(Instant::now());
                let (tx, rx) = channel();
                let (path, ctx) = (ws.project.path.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let tree = guard::repo_root(&path)
                        .and_then(|repo| candidate::candidate_tree(&repo, stage, unstaged, None));
                    // Si no se puede calcular, no se marca como cambiado (sin falsas alarmas).
                    let _ = tx.send(tree.is_ok_and(|t| t != frozen.tree));
                    ctx.request_repaint();
                });
                ws.guard.stale_rx = Some(rx);
            }
            ctx.request_repaint_after(Duration::from_secs(3));
        }
        for (path, report, evidence, usage) in to_save {
            self.db(|s| s.save_usage(&path, &usage));
            self.db(|s| s.save_evidence(&path, &evidence));
            self.db(|s| s.save_validation(&path, &report));
        }
        // Resultados de memoria del panel abierto.
        if let Some(i) = self.active.filter(|i| {
            let m = &self.workspaces[*i].memory;
            m.open && m.dirty
        }) {
            let path = self.workspaces[i].project.path.clone();
            let (query, kind) = (
                self.workspaces[i].memory.query.clone(),
                self.workspaces[i].memory.kind.clone(),
            );
            let results = self
                .db(|s| s.search_memories(&path, &query, kind.as_deref(), 50))
                .unwrap_or_default();
            let count = self.db(|s| s.count_memories(&path)).unwrap_or(0);
            let memory = &mut self.workspaces[i].memory;
            (memory.results, memory.count, memory.dirty) = (results, count, false);
        }
        // Historial del panel abierto.
        if let Some(i) = self.active.filter(|i| {
            let g = &self.workspaces[*i].guard;
            g.open && g.history.is_none()
        }) {
            let path = self.workspaces[i].project.path.clone();
            let history = self.db(|s| s.validations(&path, 15)).unwrap_or_default();
            let spent = self.db(|s| s.month_spent(&path)).unwrap_or(0.0);
            let budget =
                forge_core::router::RouterConfig::load(&path).map_or(0.0, |c| c.monthly_budget_usd);
            self.workspaces[i].guard.history = Some(history);
            self.workspaces[i].guard.ai_spend = Some((spent, budget));
        }
    }

    /// Panel Project Guard: resumen y acciones arriba, controles en una tarjeta debajo,
    /// y hooks, consumo e historial al final.
    pub(super) fn guard_ui(
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
        ui.spacing_mut().item_spacing.y = 8.0;

        // Cabecera.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icon::SHIELD_CHECK)
                    .size(18.0)
                    .color(theme::ACCENT),
            );
            ui.label(
                RichText::new("Project Guard")
                    .size(15.0)
                    .color(theme::TEXT)
                    .strong(),
            );
            ui.label(
                RichText::new(ws.project.name())
                    .size(13.0)
                    .color(theme::TEXT_3),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, tr!("Cerrar (⌘G)")).clicked() {
                    cmds.push(UiCmd::ToggleGuard);
                }
            });
        });
        segmented(&mut ui, ws.guard.stage, cmds);

        let running = ws
            .guard
            .run
            .as_ref()
            .is_some_and(|r| !r.snapshot(guard::GuardRun::finished));
        if running {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }

        // Resumen y acciones.
        theme::card(&mut ui, |ui| {
            let Some(run) = &ws.guard.run else {
                ui.label(RichText::new(tr!("Comprueba si el proyecto está listo con reglas deterministas: tamaño del diff, archivos prohibidos, secretos y tus checks.")).color(theme::TEXT_2));
                ui.add_space(4.0);
                if theme::primary(ui, tr!("{p0}  Ejecutar", p0 = icon::PLAY)).clicked() {
                    cmds.push(UiCmd::GuardRun);
                }
                return;
            };
            let state = run.state.lock().unwrap();
            let verdict = state.verdict();
            let (word, color) = verdict_text(verdict, state.stage);
            ui.horizontal(|ui| {
                let glyph = match verdict {
                    guard::Verdict::Ready => icon::CHECK_CIRCLE,
                    guard::Verdict::Warnings | guard::Verdict::Pending => icon::WARNING_CIRCLE,
                    guard::Verdict::Blocked => icon::X_CIRCLE,
                    guard::Verdict::Running => icon::CIRCLE_NOTCH,
                    guard::Verdict::NothingToCheck => icon::CIRCLE,
                };
                ui.label(RichText::new(glyph).size(26.0).color(color));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(
                        RichText::new(capitalize(&word))
                            .size(17.0)
                            .color(theme::TEXT)
                            .strong(),
                    );
                    let (done, total) = state.progress();
                    let ago = state.started_at.elapsed().map_or(0, |d| d.as_secs());
                    ui.label(
                        RichText::new(tr!(
                            "{done} de {total} controles · {p0} · hace {p1}",
                            p0 = state.stage.label(),
                            p1 = human_secs(ago),
                            done = done,
                            total = total
                        ))
                        .size(12.0)
                        .color(theme::TEXT_3),
                    );
                });
            });
            if ws.guard.stage != state.stage {
                ui.label(
                    RichText::new(tr!(
                        "Resultado de {p0}: pulsa Ejecutar para {p1}.",
                        p0 = state.stage.label(),
                        p1 = ws.guard.stage.label()
                    ))
                    .size(12.0)
                    .color(theme::YELLOW),
                );
            }
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                // Nunca se ofrece crear el commit con evidencia de otra versión del código.
                let can_commit = state.stage == Stage::Commit
                    && matches!(verdict, guard::Verdict::Ready | guard::Verdict::Warnings)
                    && state.finished()
                    && !ws.guard.stale;
                if can_commit {
                    let label = if state.unstaged {
                        tr!("Preparar todo y crear commit")
                    } else {
                        tr!("Crear commit")
                    };
                    if theme::primary(ui, format!("{}  {label}", icon::GIT_BRANCH)).clicked() {
                        cmds.push(UiCmd::GuardCommit);
                    }
                }
                let can_pr = state.stage == Stage::PullRequest
                    && matches!(verdict, guard::Verdict::Ready | guard::Verdict::Warnings)
                    && state.finished()
                    && !ws.guard.stale;
                if can_pr
                    && ws.guard.pr.is_none()
                    && theme::primary(ui, tr!("{p0}  Crear PR", p0 = icon::GIT_PULL_REQUEST))
                        .clicked()
                {
                    cmds.push(UiCmd::PrForm(true));
                }
                if running {
                    if theme::secondary(ui, tr!("{p0}  Cancelar", p0 = icon::STOP)).clicked() {
                        cmds.push(UiCmd::GuardCancel);
                    }
                } else if can_commit || can_pr {
                    if theme::secondary(ui, tr!("{p0}  Repetir", p0 = icon::ARROW_CLOCKWISE))
                        .clicked()
                    {
                        cmds.push(UiCmd::GuardRun);
                    }
                } else if theme::primary(ui, tr!("{p0}  Ejecutar", p0 = icon::PLAY)).clicked() {
                    cmds.push(UiCmd::GuardRun);
                }
                if theme::secondary(ui, tr!("{p0}  Ver diff", p0 = icon::GIT_DIFF)).clicked() {
                    cmds.push(UiCmd::GuardDiff);
                }
                if state.finished()
                    && state.frozen.is_some()
                    && theme::secondary(ui, tr!("{p0}  Exportar", p0 = icon::EXPORT)).clicked()
                {
                    cmds.push(UiCmd::GuardExport(None));
                }
            });
            if let Some(c) = &state.frozen {
                let place = if c.isolated {
                    tr!(" · checks en copia aislada")
                } else {
                    ""
                };
                ui.label(
                    RichText::new(format!(
                        "{} · HEAD {}{place}",
                        c.id,
                        c.head.chars().take(7).collect::<String>()
                    ))
                    .size(11.0)
                    .monospace()
                    .color(theme::TEXT_4),
                )
                .on_hover_text(tr!(
                    "Árbol {p0}\nDiff {p1}",
                    p0 = c.tree,
                    p1 = c.diff_hash
                ));
            }
        });

        if let Some(pr) = &mut ws.guard.pr {
            theme::card(&mut ui, |ui| {
                ui.label(
                    RichText::new(tr!(
                        "{p0}  Pull request hacia {p1}",
                        p0 = icon::GIT_PULL_REQUEST,
                        p1 = pr.base
                    ))
                    .strong(),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut pr.title)
                        .hint_text(tr!("Título del PR"))
                        .desired_width(f32::INFINITY),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut pr.body)
                        .hint_text(tr!("Descripción"))
                        .desired_rows(10)
                        .desired_width(f32::INFINITY),
                );
                ui.checkbox(&mut pr.draft, tr!("Crear como borrador"));
                ui.label(
                    RichText::new(tr!("Se publica con gh pr create en un panel nuevo (si la rama no está subida, gh lo ofrece)."))
                        .size(11.0)
                        .color(theme::TEXT_3),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !pr.title.trim().is_empty(),
                            egui::Button::new(tr!("Crear en GitHub")),
                        )
                        .clicked()
                    {
                        cmds.push(UiCmd::PrCreate);
                    }
                    if theme::secondary(ui, tr!("Cancelar")).clicked() {
                        cmds.push(UiCmd::PrForm(false));
                    }
                });
            });
        }
        if ws.guard.stale {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x3d, 0x33, 0x12))
                .stroke(egui::Stroke::new(1.0, theme::YELLOW.gamma_multiply(0.5)))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(&mut ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(tr!("{p0}  El proyecto cambió después de la validación", p0 = icon::WARNING_CIRCLE)).color(theme::YELLOW).strong());
                    ui.label(RichText::new(tr!("La evidencia ya no corresponde al estado actual. Ejecuta la revisión de nuevo.")).size(12.0).color(theme::TEXT_2));
                });
        }
        if !ws.project.path.join(".forge/rules.toml").exists() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(tr!("Sin .forge/rules.toml: solo se analiza el diff."))
                        .size(12.0)
                        .color(theme::TEXT_3),
                );
                if ui
                    .link(RichText::new(tr!("Crear rules.toml")).size(12.0))
                    .on_hover_text("Detecta lint, typecheck, tests y build")
                    .clicked()
                {
                    cmds.push(UiCmd::CreateRules);
                }
            });
        }
        if let Some(e) = &ws.guard.error {
            ui.label(RichText::new(e).size(12.0).color(theme::RED));
        }
        if let Some(form) = &mut ws.guard.allow {
            theme::card(&mut ui, |ui| {
                ui.label(RichText::new(tr!("Omitir: {p0}", p0 = form.label)).strong());
                ui.label(
                    RichText::new(tr!(
                        "Queda registrada con tu usuario, la fecha y el diff actual."
                    ))
                    .size(12.0)
                    .color(theme::TEXT_3),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut form.reason)
                        .hint_text(tr!(
                            "Motivo (obligatorio), p. ej. «archivos generados por Drizzle»"
                        ))
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                for scope in guard::Scope::ALL {
                    ui.radio_value(&mut form.scope, scope, scope.label());
                }
                ui.horizontal(|ui| {
                    let valid = form.reason.trim().chars().count() >= 5;
                    if ui
                        .add_enabled(
                            valid,
                            egui::Button::new(
                                RichText::new(tr!("Registrar excepción")).color(Color32::WHITE),
                            )
                            .fill(theme::ACCENT),
                        )
                        .clicked()
                    {
                        cmds.push(UiCmd::GuardAllow);
                    }
                    if theme::secondary(ui, tr!("Cancelar")).clicked() {
                        cmds.push(UiCmd::GuardAllowForm(None));
                    }
                });
            });
        }

        // Controles.
        let footer = 96.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height((ui.available_height() - footer).max(80.0))
            .show(&mut ui, |ui| {
                let Some(run) = &ws.guard.run else { return };
                let state = run.state.lock().unwrap();
                if state.items.is_empty() && state.checks.is_empty() && state.error.is_none() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(tr!("Analizando el diff…")).color(theme::TEXT_3));
                    });
                    return;
                }
                theme::card(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    if let Some(e) = &state.error {
                        ui.label(
                            RichText::new(format!("{}  {e}", icon::X_CIRCLE)).color(theme::RED),
                        );
                    }
                    for (n, item) in state.items.iter().enumerate() {
                        let (glyph, color) = level_icon(item.level);
                        let text = status_text(glyph, color, &item.label, "");
                        if item.details.is_empty() {
                            ui.label(text);
                        } else {
                            egui::CollapsingHeader::new(text)
                                .id_salt(("guard-item", n))
                                .show(ui, |ui| {
                                    for d in &item.details {
                                        ui.label(
                                            RichText::new(d)
                                                .size(11.5)
                                                .monospace()
                                                .color(theme::TEXT_3),
                                        );
                                    }
                                });
                        }
                        if item.level == guard::Level::Block
                            && ui
                                .link(RichText::new(tr!("Omitir con motivo…")).size(11.5))
                                .on_hover_text(format!("Regla: {}", item.id))
                                .clicked()
                        {
                            cmds.push(UiCmd::GuardAllowForm(Some((
                                item.id.clone(),
                                item.label.clone(),
                            ))));
                        }
                    }
                    for c in &state.checks {
                        let name = c.check.name.clone().unwrap_or_else(|| c.check.id.clone());
                        let rule = guard::GuardRun::check_rule(c);
                        let level = state.check_level(c);
                        let (glyph, color, status) =
                            check_status(c, level, state.exception_for(&rule));
                        let text = status_text(glyph, color, &name, &status);
                        if c.output.is_empty() {
                            ui.label(text).on_hover_text(&c.check.command);
                        } else {
                            let failed = !matches!(c.state, guard::CheckState::Passed);
                            egui::CollapsingHeader::new(text)
                                .id_salt(("guard-check", &c.check.id, state.started_at))
                                .default_open(failed)
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(&c.check.command)
                                            .size(11.5)
                                            .monospace()
                                            .color(theme::TEXT_4),
                                    );
                                    egui::ScrollArea::vertical()
                                        .id_salt(("out", &c.check.id))
                                        .max_height(220.0)
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(&c.output)
                                                    .size(11.5)
                                                    .monospace()
                                                    .color(theme::TEXT_2),
                                            );
                                        });
                                });
                        }
                        if level == Some(guard::Level::Block)
                            && ui
                                .link(RichText::new(tr!("Omitir con motivo…")).size(11.5))
                                .on_hover_text(format!("Regla: {rule}"))
                                .clicked()
                        {
                            cmds.push(UiCmd::GuardAllowForm(Some((rule.clone(), name.clone()))));
                        }
                    }
                });
            });

        // Pie: hooks, consumo de IA e historial.
        ui.add_space(4.0);
        if ws
            .guard
            .hooks
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(5))
        {
            ws.guard.hooks = Some((Instant::now(), hooks::status(&ws.project.path)));
        }
        if let Some((_, status)) = &ws.guard.hooks {
            ui.horizontal(|ui| match status {
                Ok(list) => {
                    let installed = list.iter().all(|(_, s)| *s == hooks::HookState::Installed);
                    let (glyph, color, text) = if installed {
                        (
                            icon::CHECK_CIRCLE,
                            theme::GREEN,
                            tr!("Hooks de Git instalados"),
                        )
                    } else {
                        (
                            icon::CIRCLE,
                            theme::TEXT_4,
                            tr!("Hooks de Git no instalados"),
                        )
                    };
                    ui.label(
                        RichText::new(format!("{glyph}  {text}"))
                            .size(12.0)
                            .color(color),
                    )
                    .on_hover_text(tr!(
                        "pre-commit y pre-push ejecutan el Guard también fuera de Forge"
                    ));
                    let action = if installed {
                        tr!("Quitar")
                    } else {
                        tr!("Instalar")
                    };
                    if ui.link(RichText::new(action).size(12.0)).clicked() {
                        cmds.push(if installed {
                            UiCmd::HooksUninstall
                        } else {
                            UiCmd::HooksInstall
                        });
                    }
                }
                Err(e) => {
                    ui.label(
                        RichText::new(tr!("Hooks: {e}", e = e))
                            .size(11.5)
                            .color(theme::TEXT_4),
                    );
                }
            });
        }
        if let Some(notice) = &ws.guard.notice {
            ui.label(RichText::new(notice).size(11.5).color(theme::TEXT_3));
        }
        if let Some(history) = &ws.guard.history {
            egui::CollapsingHeader::new(
                RichText::new(tr!("Historial ({p0})", p0 = history.len()))
                    .size(12.0)
                    .color(theme::TEXT_2),
            )
            .id_salt("guard-history")
            .show(&mut ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("history-scroll")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for (id, r) in history {
                            let (_, color) = verdict_text(r.verdict, r.stage);
                            let candidate = r.candidate.as_ref().map_or("—", |c| c.id.as_str());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("●").size(9.0).color(color));
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {candidate} · {}",
                                        r.stage.label(),
                                        store::ago_precise(r.finished_at)
                                    ))
                                    .size(12.0)
                                    .color(theme::TEXT_2),
                                );
                                if ui.link(RichText::new(tr!("Exportar")).size(12.0)).clicked() {
                                    cmds.push(UiCmd::GuardExport(Some(*id)));
                                }
                            });
                        }
                    });
            });
        }
    }
}
