// Diálogo «Nuevo agente» (a la manera del «Create worktree» de Orca): proyecto, dónde (en el
// proyecto o en un worktree nuevo), desde qué (una tarea, una rama o un nombre), qué agente y,
// en «Avanzado», reanudar, la cuenta y todas las acciones de siempre de cada agente. ⌘↵ crea,
// Esc o ✕ cierran. «Crear más» lo deja abierto para lanzar otro.
use super::*;
use forge_core::worktrees;

/// De qué sale el worktree nuevo.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Source {
    /// Una tarea: nombra la rama y es el primer mensaje del agente.
    #[default]
    Task,
    /// Una rama que ya existe.
    Branch,
    /// El nombre de una rama nueva.
    Name,
}

#[derive(Default)]
pub(super) struct NewAgent {
    /// El proyecto para el que se rellenó: al cambiar de proyecto, lo suyo se vuelve a leer.
    project: PathBuf,
    /// En un worktree nuevo (si no, en la carpeta del proyecto).
    pub(super) worktree: bool,
    pub(super) source: Source,
    pub(super) task: String,
    pub(super) name: String,
    pub(super) branch: String,
    /// El agente elegido (`None`: el de siempre).
    pub(super) agent: Option<String>,
    advanced: bool,
    /// Reanudar su última sesión en el proyecto (o ir a su terminal si ya está abierta).
    resume: bool,
    /// Tras crear, seguir con el diálogo abierto.
    more: bool,
    /// Ramas locales del proyecto (se leen al abrir la pestaña «Rama»).
    branches: Option<Vec<String>>,
    error: Option<String>,
}

/// Nombre de rama a partir de una tarea: minúsculas, sin acentos, palabras unidas con guiones
/// (las seis primeras).
pub(super) fn slug(task: &str) -> String {
    let plain: String = task
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'Á' => 'a',
            'é' | 'è' | 'ë' | 'É' => 'e',
            'í' | 'ì' | 'ï' | 'Í' => 'i',
            'ó' | 'ò' | 'ö' | 'Ó' => 'o',
            'ú' | 'ù' | 'ü' | 'Ú' => 'u',
            'ñ' | 'Ñ' => 'n',
            c if c.is_ascii_alphanumeric() => c.to_ascii_lowercase(),
            _ => ' ',
        })
        .collect();
    plain
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join("-")
}

/// Ramas locales de `repo` que no están ya abiertas en otro worktree (git no deja abrirlas
/// dos veces).
fn local_branches(repo: &Path) -> Vec<String> {
    let busy: Vec<String> = worktrees::list_basic(repo)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| w.branch)
        .collect();
    let all: Vec<String> = std::process::Command::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    all.into_iter().filter(|b| !busy.contains(b)).collect()
}

/// Etiqueta pequeña sobre un campo.
fn field(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).size(11.5).color(theme::TEXT_3));
}

/// Pestañas en una fila; devuelve la elegida.
fn tabs<T: Copy + PartialEq>(ui: &mut egui::Ui, current: T, options: &[(T, &str)]) -> T {
    let mut chosen = current;
    ui.horizontal(|ui| {
        for (value, label) in options {
            if ui.selectable_label(current == *value, *label).clicked() {
                chosen = *value;
            }
        }
    });
    chosen
}

impl App {
    /// El «+» de la barra lateral: abre el diálogo para el proyecto activo.
    pub(super) fn open_new_agent(&mut self) {
        if self.active.is_some() {
            self.new_agent = Some(NewAgent::default());
        }
    }

    pub(super) fn new_agent_ui(&mut self, ctx: &egui::Context, cmds: &mut Vec<UiCmd>) {
        let (Some(i), Some(mut d)) = (self.active, self.new_agent.take()) else {
            return;
        };
        let detected = self.agents.lock().unwrap().clone();
        let detecting = self.detecting.load(Ordering::Relaxed);
        let preferred = self.preferred_agent();
        let names: Vec<String> = self.workspaces.iter().map(|w| w.project.name()).collect();
        let ws = &mut self.workspaces[i];
        if d.project != ws.project.path {
            d.project = ws.project.path.clone();
            d.branches = None;
            d.branch.clear();
            d.error = None;
        }
        // Los instalados (o configurados a mano), en el orden de Ajustes.
        let installed: Vec<agents::AgentSpec> = ws
            .project
            .agents
            .iter()
            .filter(|a| {
                a.enabled
                    && (a.configured || detected.get(a.program()).is_some_and(|f| f.path.is_some()))
            })
            .cloned()
            .collect();
        if d.agent
            .as_ref()
            .is_none_or(|id| !installed.iter().any(|a| &a.id == id))
        {
            d.agent = agents::default_agent_with(&installed, &detected, &preferred)
                .map(|a| a.id.clone())
                .or_else(|| installed.first().map(|a| a.id.clone()));
        }
        let spec = installed
            .iter()
            .find(|a| Some(&a.id) == d.agent.as_ref())
            .cloned();
        let mut close = false;
        let mut create = false;
        let modal = egui::Modal::new(egui::Id::new("new-agent"))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::SEPARATOR))
                    .corner_radius(14.0)
                    .inner_margin(20.0),
            )
            .show(ctx, |ui| {
                let width = (ctx.content_rect().width() - 80.0).clamp(280.0, 480.0);
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr!("Nuevo agente")).size(17.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, icon::X, tr!("Cerrar")).clicked() {
                            close = true;
                        }
                    });
                });
                field(ui, tr!("Proyecto"));
                egui::ComboBox::from_id_salt("new-agent-project")
                    .width(ui.available_width())
                    .selected_text(&names[i])
                    .show_ui(ui, |ui| {
                        for (k, name) in names.iter().enumerate() {
                            if ui.selectable_label(k == i, name).clicked() && k != i {
                                cmds.push(UiCmd::Activate(k));
                            }
                        }
                    });
                field(ui, tr!("Dónde"));
                d.worktree = tabs(
                    ui,
                    d.worktree,
                    &[
                        (false, tr!("En el proyecto")),
                        (true, tr!("Nuevo worktree")),
                    ],
                );
                if d.worktree {
                    field(ui, tr!("Crear desde"));
                    d.source = tabs(
                        ui,
                        d.source,
                        &[
                            (Source::Task, tr!("Tarea")),
                            (Source::Branch, tr!("Rama")),
                            (Source::Name, tr!("Nombre")),
                        ],
                    );
                    match d.source {
                        Source::Task => {
                            ui.add(
                                egui::TextEdit::multiline(&mut d.task)
                                    .hint_text(tr!("Describe la tarea: nombra la rama y es el primer mensaje del agente"))
                                    .desired_rows(2)
                                    .desired_width(f32::INFINITY),
                            );
                            let branch = slug(&d.task);
                            if !branch.is_empty() {
                                ui.label(
                                    RichText::new(tr!("Rama: {b}", b = branch))
                                        .size(11.0)
                                        .color(theme::TEXT_3),
                                );
                            }
                        }
                        Source::Branch => {
                            let branches = d
                                .branches
                                .get_or_insert_with(|| local_branches(&ws.project.path));
                            let shown = if d.branch.is_empty() {
                                tr!("Elige una rama").to_string()
                            } else {
                                d.branch.clone()
                            };
                            egui::ComboBox::from_id_salt("new-agent-branch")
                                .width(ui.available_width())
                                .selected_text(shown)
                                .show_ui(ui, |ui| {
                                    for b in branches.iter() {
                                        ui.selectable_value(&mut d.branch, b.clone(), b);
                                    }
                                });
                        }
                        Source::Name => {
                            ui.add(
                                egui::TextEdit::singleline(&mut d.name)
                                    .hint_text("feat/…")
                                    .desired_width(f32::INFINITY),
                            );
                        }
                    }
                } else {
                    field(ui, tr!("Tarea (opcional)"));
                    // Al reanudar sigue la conversación de antes: no hay primer mensaje.
                    let hint = if d.resume {
                        tr!("Reanuda la última sesión: sin mensaje nuevo")
                    } else {
                        tr!("Primer mensaje para el agente")
                    };
                    ui.add_enabled(
                        !d.resume,
                        egui::TextEdit::multiline(&mut d.task)
                            .hint_text(hint)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                }
                field(ui, tr!("Agente"));
                ui.horizontal(|ui| {
                    let (at, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    if let Some(spec) = &spec {
                        theme::agent_icon(ui, spec.program(), &spec.name, at, false);
                    }
                    let label = |a: &agents::AgentSpec| match ws.account_for(a.program(), None) {
                        Some(acc) if ws.has_accounts(a.program()) => {
                            format!("{} · {}", a.name, acc.name)
                        }
                        _ => a.name.clone(),
                    };
                    let shown = spec
                        .as_ref()
                        .map(label)
                        .unwrap_or_else(|| tr!("Ningún agente instalado").to_string());
                    egui::ComboBox::from_id_salt("new-agent-agent")
                        .width(ui.available_width())
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for a in &installed {
                                if ui
                                    .selectable_label(d.agent.as_ref() == Some(&a.id), label(a))
                                    .clicked()
                                {
                                    d.agent = Some(a.id.clone());
                                }
                            }
                        });
                });
                ui.add_space(6.0);
                let caret = if d.advanced {
                    icon::CARET_DOWN
                } else {
                    icon::CARET_RIGHT
                };
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!("{} {caret}", tr!("Avanzado")))
                                .size(12.0)
                                .color(theme::TEXT_2),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    d.advanced = !d.advanced;
                }
                if d.advanced {
                    if let Some(spec) = &spec
                        && !d.worktree
                    {
                        ui.checkbox(&mut d.resume, tr!("Reanudar la última sesión"))
                            .on_hover_text(tr!(
                                "Si ya está abierto, va a su terminal; si no, sigue su última conversación en este proyecto"
                            ));
                        if forge_core::accounts::supported(spec.program()) {
                            let current = ws.account_for(spec.program(), None).map(|a| a.id);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(tr!("Cuenta")).color(theme::TEXT_3));
                                for a in ws.accounts_of(spec.program()) {
                                    if ui.radio(current == Some(a.id), &a.name).clicked() {
                                        cmds.push(UiCmd::UseAccount(a.program.clone(), a.id));
                                    }
                                }
                            });
                        }
                    }
                    // Todas las acciones de siempre de cada agente (clic: a su terminal o
                    // reanudar; +: sesión nueva; clic derecho: más), y volver a detectarlos.
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(tr!("Agentes instalados"))
                                .size(11.5)
                                .color(theme::TEXT_3),
                        );
                        if detecting {
                            ui.add(egui::Spinner::new().size(12.0));
                        } else if theme::icon_button(
                            ui,
                            icon::ARROW_CLOCKWISE,
                            tr!("Volver a detectar agentes"),
                        )
                        .clicked()
                        {
                            cmds.push(UiCmd::DetectAgents);
                        }
                    });
                    // Lo que se pida desde la lista pasa detrás del diálogo: se cierra para verlo.
                    let before = cmds.len();
                    App::agents_ui(ws, &detected, &preferred, ui, cmds);
                    if cmds[before..]
                        .iter()
                        .any(|c| !matches!(c, UiCmd::DetectAgents | UiCmd::UseAccount(..)))
                    {
                        close = true;
                    }
                    ui.label(
                        RichText::new(tr!(
                            "La lista completa, sus cuentas, modelo y esfuerzo están en Ajustes → Agentes."
                        ))
                        .size(11.0)
                        .color(theme::TEXT_3),
                    );
                }
                if let Some(e) = &d.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).size(12.0).color(theme::RED));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut d.more, tr!("Crear más"))
                        .on_hover_text(tr!("Tras crear, el diálogo sigue abierto para lanzar otro"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = format!(
                            "{}  {}",
                            tr!("Crear y abrir agente"),
                            forge_core::i18n::keys("⌘↩")
                        );
                        if ui
                            .add_enabled(spec.is_some(), theme::primary_button(label))
                            .clicked()
                        {
                            create = true;
                        }
                    });
                });
            });
        // Esc cierra; un clic fuera no (no se pierde lo escrito).
        if modal.should_close() && !modal.backdrop_response.clicked() {
            close = true;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) && spec.is_some() {
            create = true;
        }
        if close {
            return;
        }
        self.new_agent = Some(d);
        if create {
            cmds.push(UiCmd::NewAgentCreate);
        }
    }

    /// Crea lo pedido en el diálogo: el worktree si hace falta y el agente en una terminal.
    pub(super) fn create_new_agent(&mut self, ctx: &egui::Context, area: Rect) {
        let (Some(i), Some(d)) = (self.active, self.new_agent.as_ref()) else {
            return;
        };
        let root = self.worktrees.root.clone().or_else(worktrees::root);
        let ws = &mut self.workspaces[i];
        let result = create(ws, d, root, ctx, area);
        let Some(d) = self.new_agent.as_mut() else {
            return;
        };
        match result {
            Err(e) => d.error = Some(e),
            Ok(()) if d.more => {
                // Listo para el siguiente: lo escrito se vacía, lo elegido se queda.
                d.task.clear();
                d.name.clear();
                d.error = None;
                d.branches = None;
            }
            Ok(()) => self.new_agent = None,
        }
    }
}

fn create(
    ws: &mut Workspace,
    d: &NewAgent,
    root: Option<PathBuf>,
    ctx: &egui::Context,
    area: Rect,
) -> Result<(), String> {
    let id = d
        .agent
        .clone()
        .ok_or_else(|| tr!("Elige un agente").to_string())?;
    let spec = ws
        .project
        .agents
        .iter()
        .find(|a| a.id == id)
        .cloned()
        .ok_or_else(|| tr!("No encuentro el agente {id} en este proyecto.", id = id))?;
    let task = d.task.trim();
    let prompt = match (d.worktree, d.source) {
        (true, Source::Task) | (false, _) => task,
        _ => "",
    };
    // Antes de crear nada: un agente sin mensaje inicial no recibiría la tarea.
    if !prompt.is_empty() && spec.with_prompt(&spec.command, "x").is_none() {
        return Err(crate::workspace::no_prompt(&spec.name));
    }
    if !d.worktree {
        match (d.resume, prompt.is_empty()) {
            (true, _) => ws.agent_click(ctx, &id, false, area),
            (false, true) => ws.agent_click(ctx, &id, true, area),
            (false, false) => {
                let cwd = ws.project.root();
                ws.launch_agent_in(ctx, &id, prompt, cwd, spec.name.clone(), area)?;
            }
        }
        return Ok(());
    }
    let branch = match d.source {
        Source::Task => slug(task),
        Source::Branch => d.branch.clone(),
        Source::Name => d.name.trim().to_string(),
    };
    if branch.is_empty() {
        return Err(match d.source {
            Source::Task => tr!("Describe la tarea para nombrar la rama"),
            Source::Branch => tr!("Elige una rama"),
            Source::Name => tr!("Escribe el nombre de la rama"),
        }
        .to_string());
    }
    // Una rama ya abierta (en el proyecto o en otro worktree): git no la abre dos veces, y
    // reutilizar su worktree mezclaría dos agentes en la misma carpeta.
    if let Some(open) = worktrees::list_basic(&ws.project.path)
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch.as_str()))
    {
        return Err(tr!(
            "La rama {b} ya está abierta en {p}",
            b = branch,
            p = tilde(&open.path)
        ));
    }
    let root = root.ok_or_else(|| tr!("no se encontró $HOME").to_string())?;
    let who = worktrees::Record {
        origin: Some(spec.name.clone()),
        // La terminal que se abre ahora: al cerrarla, la limpieza lo propone.
        session: Some(ws.next_session_id()),
        ..Default::default()
    };
    let wt = worktrees::create(&ws.project.path, &branch, None, &root, who)?;
    let name = format!("{} · {branch}", spec.name);
    ws.launch_agent_in(ctx, &id, prompt, wt, name, area)
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn a_task_names_its_branch() {
        assert_eq!(
            slug("Añadir el botón de exportación"),
            "anadir-el-boton-de-exportacion"
        );
        assert_eq!(
            slug("Fix: login timeout (#12) on the slow path today"),
            "fix-login-timeout-12-on-the"
        );
        assert_eq!(slug("  ¿?  "), "");
    }
}
