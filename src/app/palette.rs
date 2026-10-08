// Paleta de comandos (⌘K): busca y ejecuta cualquier acción escribiendo parte de su nombre.
use super::*;

/// Paleta abierta.
#[derive(Default)]
pub(super) struct Palette {
    pub(super) query: String,
    selected: usize,
    /// Proyectos recientes (se leen al abrir, no en cada fotograma).
    recent: Vec<(String, PathBuf)>,
}

/// Qué hace una entrada al elegirla.
pub(super) enum Run {
    Action(Action),
    Cmds(Vec<UiCmd>),
}

struct Entry {
    icon: &'static str,
    label: String,
    /// Atajo o detalle, a la derecha.
    hint: String,
    run: Run,
}

const MAX_RESULTS: usize = 12;

/// Minúsculas y sin tildes, para que "vacunacion" encuentre "Vacunación".
fn fold(c: char) -> char {
    match c.to_lowercase().next().unwrap_or(c) {
        'á' | 'à' | 'ä' | 'â' => 'a',
        'é' | 'è' | 'ë' | 'ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' => 'o',
        'ú' | 'ù' | 'ü' | 'û' => 'u',
        'ñ' => 'n',
        c => c,
    }
}

/// Coincidencia difusa: las letras de la consulta aparecen en orden en el texto.
/// Elige la mejor alineación: puntúan las letras seguidas (+6) y los inicios de palabra
/// (+5): "guard" prefiere "Guard…" entero a saltar a la d de "de Git". `None` si no coincide.
pub(super) fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let t: Vec<char> = text.chars().map(fold).collect();
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(fold)
        .collect();
    let bonus = |i: usize| {
        1 + if i == 0 || !t[i - 1].is_alphanumeric() {
            5
        } else {
            0
        }
    };
    // best[i]: mejor puntuación con la letra actual de la consulta emparejada en t[i].
    let mut best: Vec<Option<i32>> = vec![Some(0); t.len() + 1];
    for (j, &c) in q.iter().enumerate() {
        let mut next = vec![None; t.len()];
        let mut before: Option<i32> = None; // mejor de best[..i-1]
        for i in 0..t.len() {
            if j > 0 && i >= 2 {
                before = before.max(best[i - 2]);
            }
            if t[i] != c {
                continue;
            }
            let from = if j == 0 {
                Some(0)
            } else {
                let joined = i.checked_sub(1).and_then(|k| best[k]).map(|s| s + 6);
                before.max(joined)
            };
            next[i] = from.map(|s| s + bonus(i));
        }
        best = next;
    }
    if q.is_empty() {
        Some(0)
    } else {
        best.into_iter().flatten().max()
    }
}

impl App {
    pub(super) fn open_palette(&mut self) {
        let open: Vec<PathBuf> = self
            .workspaces
            .iter()
            .map(|w| w.project.path.clone())
            .collect();
        let recent = self
            .db(|s| s.recent())
            .unwrap_or_default()
            .into_iter()
            .filter(|r| !open.contains(&r.path) && r.path.is_dir())
            .map(|r| (r.name, r.path))
            .collect();
        self.palette = Some(Palette {
            recent,
            ..Default::default()
        });
    }

    /// Todas las acciones disponibles ahora (dependen del proyecto activo).
    fn palette_entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let mut add = |icon, label: String, hint: &str, run| {
            // Detalles largos (rutas) se recortan por la izquierda para no tapar el nombre.
            let chars: Vec<char> = hint.chars().collect();
            let hint = if chars.len() > 36 {
                format!("…{}", chars[chars.len() - 35..].iter().collect::<String>())
            } else {
                hint.to_string()
            };
            out.push(Entry {
                icon,
                label,
                hint,
                run,
            })
        };
        let act = Run::Action;
        let cmd = |c| Run::Cmds(vec![c]);

        for (i, ws) in self.workspaces.iter().enumerate() {
            if Some(i) != self.active {
                let hint = match i {
                    _ if ws.is_dormant() => "dormido".to_string(),
                    0..9 => format!("⌘{}", i + 1),
                    _ => String::new(),
                };
                add(
                    icon::FOLDER,
                    format!("Ir a {}", ws.project.name()),
                    &hint,
                    cmd(UiCmd::Activate(i)),
                );
            }
        }
        if let Some(palette) = &self.palette {
            for (name, path) in &palette.recent {
                add(
                    icon::FOLDER_OPEN,
                    format!("Abrir proyecto {name}"),
                    &tilde(path),
                    cmd(UiCmd::Open(path.clone())),
                );
            }
        }

        if let Some(ws) = self.active.map(|i| &self.workspaces[i]) {
            // Agentes instalados.
            let detected = self.agents.lock().map(|d| d.clone()).unwrap_or_default();
            let default =
                agents::default_agent(&ws.project.agents, &detected).map(|a| a.id.clone());
            for agent in ws.project.agents.iter().filter(|a| a.enabled) {
                let installed = detected
                    .get(agent.program())
                    .is_some_and(|d| d.path.is_some());
                if !installed {
                    continue;
                }
                let hint = if default.as_deref() == Some(agent.id.as_str()) {
                    "⌘⇧A"
                } else {
                    ""
                };
                add(
                    icon::ROBOT,
                    format!("Abrir {}", agent.name),
                    hint,
                    cmd(UiCmd::OpenAgent(agent.id.clone(), false)),
                );
                if agent.resume.is_some() {
                    add(
                        icon::ROBOT,
                        format!("Reanudar {}", agent.name),
                        "última sesión",
                        cmd(UiCmd::OpenAgent(agent.id.clone(), true)),
                    );
                }
            }

            // Procesos administrados.
            for m in &ws.processes.list {
                let (id, name) = (&m.def.id, m.def.label());
                if m.is_active() {
                    add(
                        icon::STOP,
                        format!("Detener {name}"),
                        "proceso",
                        cmd(UiCmd::Stop(id.clone())),
                    );
                    add(
                        icon::ARROW_CLOCKWISE,
                        format!("Reiniciar {name}"),
                        "proceso",
                        cmd(UiCmd::Restart(id.clone())),
                    );
                } else {
                    add(
                        icon::PLAY,
                        format!("Iniciar {name}"),
                        "proceso",
                        cmd(UiCmd::Start(id.clone())),
                    );
                }
                add(
                    icon::SCROLL,
                    format!("Ver logs de {name}"),
                    "proceso",
                    cmd(UiCmd::ShowProcess(id.clone())),
                );
            }
            if !ws.processes.list.is_empty() {
                add(
                    icon::PLAY,
                    "Iniciar todos los procesos".into(),
                    "",
                    cmd(UiCmd::StartAll),
                );
                add(
                    icon::STOP,
                    "Detener todos los procesos".into(),
                    "",
                    cmd(UiCmd::StopAll),
                );
            }

            // Comandos guardados del proyecto.
            for c in &ws.project.config.commands {
                add(
                    icon::TERMINAL,
                    format!("Ejecutar {}", c.name),
                    &c.command,
                    cmd(UiCmd::RunCommand(c.clone())),
                );
            }

            // Guard.
            add(
                icon::SHIELD_CHECK,
                "Guard: mostrar u ocultar".into(),
                "⌘G",
                cmd(UiCmd::ToggleGuard),
            );
            for stage in Stage::ALL {
                add(
                    icon::SHIELD_CHECK,
                    format!("Guard: validar {}", stage.label().to_lowercase()),
                    "",
                    Run::Cmds(vec![UiCmd::GuardStage(stage), UiCmd::GuardRun]),
                );
            }
            add(
                icon::GIT_BRANCH,
                "Guard: instalar hooks de Git".into(),
                "",
                cmd(UiCmd::HooksInstall),
            );
            add(
                icon::GIT_BRANCH,
                "Guard: quitar hooks de Git".into(),
                "",
                cmd(UiCmd::HooksUninstall),
            );
            if !ws.project.path.join(".forge/rules.toml").exists() {
                add(
                    icon::FILE_PLUS,
                    "Guard: crear rules.toml".into(),
                    "",
                    cmd(UiCmd::CreateRules),
                );
            }

            // Memoria.
            add(
                icon::BRAIN,
                "Memoria: mostrar u ocultar".into(),
                "⌘⇧M",
                cmd(UiCmd::ToggleMemory),
            );
            add(
                icon::NOTE_PENCIL,
                "Memoria: nueva nota".into(),
                "",
                cmd(UiCmd::NewNote),
            );
            add(
                icon::LIGHTBULB,
                "Ideas: mostrar u ocultar".into(),
                "⌘⇧I",
                cmd(UiCmd::ToggleIdeas),
            );

            // Paneles.
            add(
                icon::TERMINAL_WINDOW,
                "Nueva terminal".into(),
                "⌘T",
                act(Action::Ws(WsAction::NewTerminal)),
            );
            add(
                icon::SQUARE_SPLIT_HORIZONTAL,
                "Dividir a la derecha".into(),
                "⌘D",
                act(Action::Ws(WsAction::Split(Dir::Row))),
            );
            add(
                icon::SQUARE_SPLIT_VERTICAL,
                "Dividir hacia abajo".into(),
                "⌘⇧D",
                act(Action::Ws(WsAction::Split(Dir::Column))),
            );
            add(
                icon::ARROWS_OUT,
                "Maximizar o restaurar panel".into(),
                "⌘↩",
                act(Action::Ws(WsAction::ToggleMaximize)),
            );
            add(
                icon::ARROW_RIGHT,
                "Panel siguiente".into(),
                "⌘]",
                act(Action::Ws(WsAction::Cycle(1))),
            );
            add(
                icon::ARROW_LEFT,
                "Panel anterior".into(),
                "⌘[",
                act(Action::Ws(WsAction::Cycle(-1))),
            );
            add(
                icon::X,
                "Cerrar panel".into(),
                "⌘W",
                act(Action::Ws(WsAction::Close)),
            );
            add(
                icon::LAYOUT,
                "Restablecer layout".into(),
                "",
                cmd(UiCmd::ResetLayout),
            );

            // Proyecto.
            add(
                icon::GEAR,
                "Editar configuración del proyecto".into(),
                "",
                cmd(UiCmd::EditConfig),
            );
            add(
                icon::ARROW_CLOCKWISE,
                "Recargar configuración del proyecto".into(),
                "",
                cmd(UiCmd::ReloadConfig),
            );
            add(
                icon::X_CIRCLE,
                "Cerrar proyecto".into(),
                "⌘⇧W",
                act(Action::CloseProject),
            );
            if let Some(i) = self.active {
                add(
                    icon::MOON,
                    "Dormir proyecto (libera memoria)".into(),
                    "",
                    cmd(UiCmd::Sleep(i)),
                );
            }
        }

        // Ideas: del proyecto activo o, en Inicio, la lista general.
        add(
            icon::LIGHTBULB,
            "Nueva idea".into(),
            "",
            cmd(UiCmd::NewIdea),
        );

        // Ventana.
        add(
            icon::FOLDER_PLUS,
            "Abrir carpeta…".into(),
            "⌘O",
            act(Action::OpenFolder),
        );
        add(icon::HOUSE, "Inicio".into(), "⌘⇧H", act(Action::Home));
        add(
            icon::SIDEBAR,
            "Mostrar u ocultar barra lateral".into(),
            "⌘B",
            act(Action::ToggleSidebar),
        );
        add(
            icon::MAGNIFYING_GLASS_PLUS,
            "Aumentar texto".into(),
            "⌘+",
            act(Action::FontBigger),
        );
        add(
            icon::MAGNIFYING_GLASS_MINUS,
            "Reducir texto".into(),
            "⌘-",
            act(Action::FontSmaller),
        );
        add(
            icon::TEXT_AA,
            "Tamaño de texto normal".into(),
            "⌘0",
            act(Action::FontReset),
        );
        out
    }

    /// Dibuja la paleta (si está abierta) y devuelve la acción elegida.
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context, full: Rect) -> Option<Run> {
        self.palette.as_ref()?;
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.palette = None;
            return None;
        }

        let query = self
            .palette
            .as_ref()
            .map(|p| p.query.clone())
            .unwrap_or_default();
        let mut matches: Vec<(i32, Entry)> = self
            .palette_entries()
            .into_iter()
            .filter_map(|e| fuzzy(&query, &e.label).map(|s| (s, e)))
            .collect();
        // Orden estable: a igual puntuación, el orden de la lista (lo más útil primero).
        matches.sort_by_key(|(s, _)| -s);
        matches.truncate(MAX_RESULTS);

        let palette = self.palette.as_mut()?;
        if down && !matches.is_empty() {
            palette.selected = (palette.selected + 1) % matches.len();
        }
        if up && !matches.is_empty() {
            palette.selected = (palette.selected + matches.len() - 1) % matches.len();
        }
        palette.selected = palette.selected.min(matches.len().saturating_sub(1));
        let mut chosen = enter.then_some(palette.selected);

        let width = (full.width() - 32.0).min(600.0);
        let shown = egui::Area::new(egui::Id::new("palette"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(full.center().x - width / 2.0, full.min.y + 90.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::SEPARATOR))
                    .corner_radius(12.0)
                    .inner_margin(8.0)
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 10],
                        blur: 30,
                        spread: 0,
                        color: Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 16.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(icon::MAGNIFYING_GLASS)
                                    .size(16.0)
                                    .color(theme::TEXT_3),
                            );
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut palette.query)
                                    .hint_text("Buscar acciones, proyectos, agentes, procesos…")
                                    .font(FontId::proportional(16.0))
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY),
                            );
                            edit.request_focus();
                            if edit.changed() {
                                palette.selected = 0;
                            }
                        });
                        ui.add_space(4.0);
                        ui.separator();
                        if matches.is_empty() {
                            ui.add_space(6.0);
                            ui.label(RichText::new("Sin resultados").color(theme::TEXT_3));
                            ui.add_space(6.0);
                        }
                        for (i, (_, entry)) in matches.iter().enumerate() {
                            let selected = i == palette.selected;
                            let color = if selected {
                                theme::ACCENT
                            } else {
                                theme::TEXT_2
                            };
                            let row = theme::row(
                                ui,
                                entry.icon,
                                color,
                                &entry.label,
                                &entry.hint,
                                selected,
                            );
                            if row.clicked() {
                                chosen = Some(i);
                            }
                            if row.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                                palette.selected = i;
                            }
                        }
                    });
            });

        // Clic fuera de la paleta: se cierra sin ejecutar nada.
        let outside = ctx.input(|i| {
            i.pointer.primary_clicked()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !shown.response.rect.contains(p))
        });
        if outside {
            self.palette = None;
            return None;
        }
        let i = chosen.filter(|&i| i < matches.len())?;
        self.palette = None;
        Some(matches.swap_remove(i).1.run)
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_prefers_word_starts_and_ignores_accents() {
        assert!(fuzzy("vc", "Guard: validar commit").is_some());
        assert!(fuzzy("xyz", "Guard: validar commit").is_none());
        assert!(fuzzy("vacunacion", "Abrir Vacunación").is_some());
        // Elige la mejor alineación: "dd" = Dividir … Derecha (dos inicios de palabra).
        assert_eq!(fuzzy("dd", "Dividir a la derecha"), Some(12));
        // Letras seguidas al inicio de palabra ganan a letras sueltas.
        assert!(fuzzy("gua", "Guard: validar") > fuzzy("gua", "Ir a gestión de usuarios"));
        assert_eq!(fuzzy("", "lo que sea"), Some(0));
    }
}
