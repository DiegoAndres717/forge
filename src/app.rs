// Ventana principal: inicio con proyectos recientes, barra lateral y workspaces abiertos.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, FontId, Key, Modifiers, Rect, RichText, Sense, Vec2};

use crate::agents;
use crate::candidate;
use crate::guard::{self, Rules, Stage};
use crate::hooks;
use crate::layout::{Dir, Toward};
use crate::processes;
use crate::project::{self, AutoRestart, Project, SavedCommand};
use crate::store::{self, Store};
use crate::theme::{self, icon};
use crate::workspace::{AllowForm, NoteForm, Workspace, WsAction, tilde, tone_color};
use crate::{Settings, load_settings, metrics};

const SIDEBAR: f32 = 240.0;
const GUARD_WIDTH: f32 = 440.0;

fn level_icon(level: guard::Level) -> (&'static str, Color32) {
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
fn status_text(glyph: &str, color: Color32, label: &str, detail: &str) -> egui::text::LayoutJob {
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
fn check_status(
    c: &guard::CheckRun,
    level: Option<guard::Level>,
    exception: Option<&guard::Exception>,
) -> (&'static str, Color32, String) {
    let bad = match c.check.severity {
        guard::Severity::Blocking => (icon::X_CIRCLE, theme::RED),
        guard::Severity::Warning => (icon::WARNING_CIRCLE, theme::YELLOW),
    };
    let (glyph, color, status) = match &c.state {
        guard::CheckState::Waiting => (icon::CIRCLE, theme::TEXT_4, "en espera".to_string()),
        guard::CheckState::Running => {
            let secs = c.started.map_or(0, |s| s.elapsed().as_secs());
            (
                icon::CIRCLE_NOTCH,
                theme::ACCENT,
                format!("ejecutándose · {secs} s"),
            )
        }
        guard::CheckState::Passed => match c.reused {
            Some(at) => (
                icon::CHECK_CIRCLE,
                theme::GREEN,
                format!("evidencia del mismo candidato ({})", store::ago_precise(at)),
            ),
            None => (icon::CHECK_CIRCLE, theme::GREEN, seconds(c.duration)),
        },
        guard::CheckState::Failed(code) => (
            bad.0,
            bad.1,
            format!("código {code} · {}", seconds(c.duration)),
        ),
        guard::CheckState::TimedOut => (
            bad.0,
            bad.1,
            format!("tiempo agotado ({} s)", c.check.timeout_seconds),
        ),
        guard::CheckState::Cancelled => (icon::CIRCLE, theme::TEXT_4, "cancelado".to_string()),
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
fn segmented(ui: &mut egui::Ui, current: Stage, cmds: &mut Vec<UiCmd>) {
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

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

fn verdict_text(verdict: guard::Verdict, stage: Stage) -> (String, Color32) {
    let target = match stage {
        Stage::Commit => "commit",
        Stage::Push => "push",
        Stage::PullRequest => "pull request",
    };
    match verdict {
        guard::Verdict::Ready => (format!("listo para {target}"), theme::GREEN),
        guard::Verdict::Warnings => (
            format!("listo para {target} con advertencias"),
            theme::YELLOW,
        ),
        guard::Verdict::Blocked => ("bloqueado".into(), theme::RED),
        guard::Verdict::Running => ("revisando…".into(), theme::ACCENT),
        guard::Verdict::Pending => ("pendiente de revisión".into(), theme::TEXT_3),
        guard::Verdict::NothingToCheck => ("sin cambios".into(), theme::TEXT_3),
    }
}

/// Escribe el reporte en `.forge/reports/` (ignorado por Git) y devuelve el aviso.
fn export_report(project: &Path, report: &crate::evidence::Report) -> Result<String, String> {
    let dir = project.join(".forge/reports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n").map_err(|e| e.to_string())?;
    }
    let date: String = crate::evidence::utc(report.finished_at)
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
fn verdict_short(verdict: guard::Verdict) -> (String, Color32) {
    match verdict {
        guard::Verdict::Ready => ("Listo".into(), theme::GREEN),
        guard::Verdict::Warnings => ("Con avisos".into(), theme::YELLOW),
        guard::Verdict::Blocked => ("Bloqueado".into(), theme::RED),
        guard::Verdict::Running => ("Revisando…".into(), theme::ACCENT),
        guard::Verdict::Pending => ("Pendiente".into(), theme::TEXT_3),
        guard::Verdict::NothingToCheck => ("Sin cambios".into(), theme::TEXT_3),
    }
}

/// "2.1.293 (Claude Code)" → "2.1.293"; "codex-cli 0.159.2" → "0.159.2".
fn short_version(v: &str) -> String {
    v.split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or(v)
        .to_string()
}

/// 1468006400 → "1,4 GB"
fn human_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.1} GB").replace('.', ",")
    } else {
        format!("{} MB", bytes / 1_048_576)
    }
}

/// 3.2 s → "3,2 s"
fn seconds(d: Option<Duration>) -> String {
    format!("{:.1} s", d.unwrap_or_default().as_secs_f32()).replace('.', ",")
}

fn human_secs(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("{} h", secs / 3600),
    }
}
const SAVE_EVERY: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Ws(WsAction),
    FontBigger,
    FontSmaller,
    FontReset,
    Home,
    Switch(usize),
    OpenFolder,
    CloseProject,
    ToggleSidebar,
    ToggleGuard,
    OpenDefaultAgent,
    ToggleMemory,
}

/// Atajos globales (siempre con ⌘, nunca con Ctrl, para no robar teclas al shell).
pub fn shortcut(key: Key, m: Modifiers) -> Option<Action> {
    if !m.mac_cmd || m.ctrl {
        return None;
    }
    let digits = [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ];
    if let Some(n) = digits
        .iter()
        .position(|k| *k == key)
        .filter(|_| !m.shift && !m.alt)
    {
        return Some(Action::Switch(n));
    }
    Some(match (key, m.shift, m.alt) {
        (Key::D, true, false) => Action::Ws(WsAction::Split(Dir::Column)),
        (Key::D, false, false) => Action::Ws(WsAction::Split(Dir::Row)),
        (Key::T, false, false) => Action::Ws(WsAction::NewTerminal),
        (Key::W, true, false) => Action::CloseProject,
        (Key::W, false, false) => Action::Ws(WsAction::Close),
        (Key::Enter, false, false) => Action::Ws(WsAction::ToggleMaximize),
        // ⌘⌥flechas (como iTerm): ⌥flechas sola mueve por palabras en el shell.
        (Key::ArrowLeft, false, true) => Action::Ws(WsAction::Focus(Toward::Left)),
        (Key::ArrowRight, false, true) => Action::Ws(WsAction::Focus(Toward::Right)),
        (Key::ArrowUp, false, true) => Action::Ws(WsAction::Focus(Toward::Up)),
        (Key::ArrowDown, false, true) => Action::Ws(WsAction::Focus(Toward::Down)),
        (Key::OpenBracket, false, false) => Action::Ws(WsAction::Cycle(-1)),
        (Key::CloseBracket, false, false) => Action::Ws(WsAction::Cycle(1)),
        (Key::Plus | Key::Equals, _, false) => Action::FontBigger,
        (Key::Minus, false, false) => Action::FontSmaller,
        (Key::Num0, false, false) => Action::FontReset,
        (Key::O, false, false) => Action::OpenFolder,
        (Key::H, true, false) => Action::Home,
        (Key::B, false, false) => Action::ToggleSidebar,
        (Key::G, false, false) => Action::ToggleGuard,
        (Key::A, true, false) => Action::OpenDefaultAgent,
        (Key::M, true, false) => Action::ToggleMemory,
        _ => return None,
    })
}

/// Acciones pedidas desde la UI (inicio y barra lateral); se aplican al final del frame.
enum UiCmd {
    /// Acción de paneles desde la barra de herramientas (como los atajos).
    Ws(WsAction),
    Home,
    Activate(usize),
    Close(usize),
    OpenFolder,
    Open(PathBuf),
    Forget(PathBuf),
    RunCommand(SavedCommand),
    CreateConfig,
    ResetLayout,
    ReloadConfig,
    EditConfig,
    Start(String),
    Stop(String),
    Restart(String),
    ShowProcess(String),
    StartAll,
    StopAll,
    ToggleGuard,
    ToggleMemory,
    MemorySave,
    MemoryDelete(i64),
    /// Abre un agente: (id, reanudar la última sesión).
    OpenAgent(String, bool),
    OpenDefaultAgent,
    DetectAgents,
    GuardRun,
    GuardCancel,
    GuardStage(Stage),
    GuardDiff,
    GuardCommit,
    CreateRules,
    /// Abre (o cierra con None) el formulario para omitir una regla: (regla, etiqueta).
    GuardAllowForm(Option<(String, String)>),
    GuardAllow,
    /// Exporta un reporte: el de la ejecución actual (None) o uno del historial.
    GuardExport(Option<i64>),
    HooksInstall,
    HooksUninstall,
}

pub struct App {
    workspaces: Vec<Workspace>,
    /// Workspace visible; `None` = pantalla de inicio.
    active: Option<usize>,
    store: Option<Store>,
    settings: Settings,
    sidebar: bool,
    error: Option<String>,
    title: String,
    last_save: Instant,
    picker: Option<Receiver<Option<PathBuf>>>,
    /// Agentes encontrados en el PATH del usuario (programa → ruta y versión).
    agents: Arc<Mutex<HashMap<String, agents::Detection>>>,
    detecting: Arc<AtomicBool>,
    /// Memoria (RSS) de los procesos del proyecto activo, refrescada en segundo plano.
    ram: Arc<Mutex<Option<u64>>>,
    ram_checked: Option<Instant>,
}

impl App {
    pub fn new(
        ctx: &egui::Context,
        settings: Settings,
        error: Option<String>,
        open: Option<PathBuf>,
    ) -> Self {
        let mut app = Self {
            workspaces: Vec::new(),
            active: None,
            store: None,
            settings,
            sidebar: true,
            error,
            title: String::new(),
            last_save: Instant::now(),
            picker: None,
            agents: Arc::default(),
            detecting: Arc::default(),
            ram: Arc::default(),
            ram_checked: None,
        };
        match Store::default_path()
            .ok_or_else(|| "no se encontró $HOME".to_string())
            .and_then(|p| Store::open(&p))
        {
            Ok(store) => app.store = Some(store),
            Err(e) => app.error = Some(format!("{e} (los workspaces no se guardarán)")),
        }

        // Restaura los proyectos que estaban abiertos al cerrar.
        let open_before = app.db(|s| s.open_projects()).unwrap_or_default();
        for path in open_before {
            if path.is_dir() {
                app.open_project(ctx, &path);
            } else {
                app.db(|s| s.set_closed(&path));
            }
        }
        let active = app.db(|s| s.setting("active")).flatten();
        app.active = active.and_then(|a| {
            app.workspaces
                .iter()
                .position(|w| w.project.path == Path::new(&a))
        });
        if let Some(path) = open {
            app.open_project(ctx, &path);
        }
        app.detect_agents(ctx, false);
        app
    }

    /// Ejecuta una operación sobre la base de datos y convierte el error en aviso.
    fn db<T>(&mut self, f: impl FnOnce(&Store) -> Result<T, String>) -> Option<T> {
        let result = f(self.store.as_ref()?);
        result.map_err(|e| self.error = Some(e)).ok()
    }

    fn open_project(&mut self, ctx: &egui::Context, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.workspaces.iter().position(|w| w.project.path == path) {
            self.active = Some(i);
            return;
        }
        if !path.is_dir() {
            self.error = Some(format!("{} no es una carpeta", path.display()));
            return;
        }
        // Configuración inválida: se abre igual con valores por defecto y se avisa.
        let project = Project::load(&path).unwrap_or_else(|e| {
            self.error = Some(e);
            Project {
                path: path.clone(),
                config: Default::default(),
                agents: crate::agents::builtins(),
            }
        });
        let saved = self.db(|s| s.workspace(&path)).flatten();
        let name = project.name();
        self.db(|s| s.touch(&path, &name));
        self.workspaces.push(Workspace::open(ctx, project, saved));
        self.active = Some(self.workspaces.len() - 1);
        self.save();
        self.detect_agents(ctx, false);
    }

    /// Detecta en segundo plano los agentes de todos los proyectos abiertos (solo los que
    /// falten, salvo `force`).
    fn detect_agents(&self, ctx: &egui::Context, force: bool) {
        let known = self.agents.lock().unwrap().clone();
        let mut programs: Vec<String> = agents::builtins()
            .iter()
            .chain(self.workspaces.iter().flat_map(|w| &w.project.agents))
            .map(|a| a.program().to_string())
            .filter(|p| force || !known.contains_key(p))
            .collect();
        programs.sort();
        programs.dedup();
        if programs.is_empty() || self.detecting.swap(true, Ordering::SeqCst) {
            return;
        }
        let (agents, detecting, ctx) = (self.agents.clone(), self.detecting.clone(), ctx.clone());
        std::thread::spawn(move || {
            let found = agents::detect(&programs);
            agents.lock().unwrap().extend(found);
            detecting.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    fn close_project(&mut self, i: usize) {
        if i >= self.workspaces.len() {
            return;
        }
        self.save(); // el layout queda guardado para la próxima apertura
        let ws = self.workspaces.remove(i); // Drop mata sus shells.
        self.db(|s| s.set_closed(&ws.project.path));
        self.active = match self.active {
            _ if self.workspaces.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i => Some(i.min(self.workspaces.len() - 1)),
            other => other,
        };
        self.save();
    }

    fn save(&mut self) {
        self.last_save = Instant::now();
        let states: Vec<_> = self
            .workspaces
            .iter()
            .map(|w| (w.project.path.clone(), w.state()))
            .collect();
        for (i, (path, state)) in states.iter().enumerate() {
            self.db(|s| s.save_workspace(path, state, i));
        }
        let active = self.active.map(|i| {
            self.workspaces[i]
                .project
                .path
                .to_string_lossy()
                .into_owned()
        });
        self.db(|s| s.set_setting("active", &active.unwrap_or_default()));
    }

    /// Selector nativo de carpetas en un hilo para no congelar los terminales.
    fn pick_folder(&mut self, ctx: &egui::Context) {
        if self.picker.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // ponytail: diálogo de macOS vía osascript; Windows (Fase 12) usará otro.
            let output = std::process::Command::new("osascript")
                .args([
                    "-e",
                    "POSIX path of (choose folder with prompt \"Elige la carpeta del proyecto\")",
                ])
                .output();
            let path = output
                .ok()
                .filter(|o| o.status.success())
                .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
            let _ = tx.send(path);
            ctx.request_repaint();
        });
        self.picker = Some(rx);
    }

    fn apply(&mut self, ctx: &egui::Context, cmd: UiCmd, area: Rect) {
        match cmd {
            UiCmd::Home => self.active = None,
            UiCmd::Activate(i) => self.active = Some(i),
            UiCmd::Close(i) => self.close_project(i),
            UiCmd::OpenFolder => self.pick_folder(ctx),
            UiCmd::Open(path) => self.open_project(ctx, &path),
            UiCmd::Forget(path) => {
                self.db(|s| s.remove(&path));
            }
            UiCmd::OpenAgent(id, resume) => {
                if let Some(ws) = self.active.map(|i| &mut self.workspaces[i]) {
                    ws.open_agent(ctx, &id, resume, area);
                }
            }
            UiCmd::Ws(action) => {
                if let Some(ws) = self.active.map(|i| &mut self.workspaces[i]) {
                    ws.run(ctx, action, area);
                }
            }
            UiCmd::DetectAgents => self.detect_agents(ctx, true),
            UiCmd::ToggleMemory | UiCmd::MemorySave | UiCmd::MemoryDelete(_) => {
                let Some(i) = self.active else { return };
                let path = self.workspaces[i].project.path.clone();
                match cmd {
                    UiCmd::ToggleMemory => {
                        let ws = &mut self.workspaces[i];
                        ws.memory.open = !ws.memory.open;
                        ws.memory.dirty = true;
                        if ws.memory.open {
                            ws.guard.open = false;
                        }
                    }
                    UiCmd::MemorySave => {
                        let Some(form) = self.workspaces[i].memory.form.take() else {
                            return;
                        };
                        if self
                            .db(|s| {
                                s.add_memory(
                                    &path,
                                    &form.kind,
                                    &form.title,
                                    &form.body,
                                    &form.tags,
                                    "usuario",
                                )
                            })
                            .is_none()
                        {
                            self.workspaces[i].memory.form = Some(form); // se conserva lo escrito
                        }
                        self.workspaces[i].memory.dirty = true;
                    }
                    UiCmd::MemoryDelete(id) => {
                        self.db(|s| s.delete_memory(&path, id));
                        self.workspaces[i].memory.dirty = true;
                    }
                    _ => {}
                }
            }
            UiCmd::OpenDefaultAgent => {
                let detected = self.agents.lock().unwrap().clone();
                if let Some(ws) = self.active.map(|i| &mut self.workspaces[i]) {
                    match agents::default_agent(&ws.project.agents, &detected).map(|a| a.id.clone())
                    {
                        Some(id) => ws.open_agent(ctx, &id, false, area),
                        None => {
                            self.error = Some(
                                "No hay ningún agente instalado (Claude Code, Codex, OpenCode…)"
                                    .into(),
                            )
                        }
                    }
                }
            }
            UiCmd::RunCommand(command) => {
                if let Some(ws) = self.active.map(|i| &mut self.workspaces[i]) {
                    ws.run_command(ctx, &command, area);
                }
            }
            UiCmd::CreateConfig | UiCmd::ResetLayout | UiCmd::ReloadConfig => {
                let Some(i) = self.active else { return };
                let ws = &mut self.workspaces[i];
                let result = match cmd {
                    UiCmd::CreateConfig => ws.project.write_template(),
                    UiCmd::ResetLayout => {
                        ws.reset_layout(ctx);
                        Ok(())
                    }
                    _ => Ok(()),
                };
                // Tras crear o recargar, se relee la configuración del disco.
                let reloaded = Project::load(&ws.project.path).map(|p| ws.set_project(p));
                if let Err(e) = result.and(reloaded) {
                    self.error = Some(e);
                }
            }
            UiCmd::ToggleGuard
            | UiCmd::GuardRun
            | UiCmd::GuardCancel
            | UiCmd::GuardStage(_)
            | UiCmd::GuardDiff
            | UiCmd::GuardCommit
            | UiCmd::CreateRules
            | UiCmd::GuardAllowForm(_)
            | UiCmd::GuardAllow
            | UiCmd::GuardExport(_)
            | UiCmd::HooksInstall
            | UiCmd::HooksUninstall => {
                let Some(i) = self.active else { return };
                let path = self.workspaces[i].project.path.clone();
                let exceptions = self.db(|s| s.exceptions(&path)).unwrap_or_default();
                let evidence = self.db(|s| s.evidence(&path)).unwrap_or_default();
                let month_spent = self.db(|s| s.month_spent(&path)).unwrap_or(0.0);
                let ws = &mut self.workspaces[i];
                match cmd {
                    UiCmd::ToggleGuard => {
                        ws.guard.open = !ws.guard.open;
                        if ws.guard.open {
                            ws.memory.open = false;
                        }
                    }
                    UiCmd::GuardStage(stage) => ws.guard.stage = stage,
                    UiCmd::GuardCancel => {
                        if let Some(run) = &ws.guard.run {
                            run.cancel();
                        }
                    }
                    UiCmd::GuardRun => {
                        ws.guard.open = true;
                        match Rules::load(&ws.project.path) {
                            Err(e) => ws.guard.error = Some(e),
                            Ok(rules) => {
                                ws.guard.error = None;
                                let repaint = ctx.clone();
                                let options = guard::RunOptions {
                                    env: ws.project.config.environment.clone(),
                                    exceptions,
                                    push_range: None,
                                    evidence,
                                    name: ws.project.name(),
                                    routing: crate::router::RouterConfig::load(&ws.project.path)
                                        .map_err(|e| {
                                            ws.guard.notice = Some(format!(
                                                "✕ {e} (revisiones con IA desactivadas)"
                                            ))
                                        })
                                        .ok(),
                                    month_spent,
                                    reviewers: crate::reviewers::load(&ws.project.path)
                                        .unwrap_or_else(|e| {
                                            ws.guard.notice = Some(format!("✕ {e}"));
                                            crate::reviewers::builtins()
                                        }),
                                };
                                ws.guard.saved = false;
                                ws.guard.stale = false;
                                ws.guard.stale_rx = None;
                                ws.guard.stale_checked = None;
                                ws.guard.run = Some(guard::run(
                                    &ws.project.path,
                                    &ws.project.root(),
                                    rules.unwrap_or_default(),
                                    ws.guard.stage,
                                    options,
                                    move || repaint.request_repaint(),
                                ));
                            }
                        }
                    }
                    UiCmd::GuardDiff | UiCmd::GuardCommit => {
                        let Some(run) = &ws.guard.run else { return };
                        let (diff, unstaged) =
                            run.snapshot(|r| (r.diff_command.clone(), r.unstaged));
                        let (name, command) = match cmd {
                            UiCmd::GuardDiff => ("diff", diff),
                            _ if unstaged => ("commit", "git add -A && git commit".to_string()),
                            _ => ("commit", "git commit".to_string()),
                        };
                        let command = SavedCommand {
                            name: name.into(),
                            command,
                            working_directory: None,
                        };
                        ws.run_command(ctx, &command, area);
                    }
                    UiCmd::GuardAllowForm(form) => {
                        ws.guard.allow = form.map(|(rule, label)| AllowForm {
                            rule,
                            label,
                            reason: String::new(),
                            scope: guard::Scope::Commit,
                        });
                    }
                    UiCmd::GuardAllow => {
                        let (Some(form), Some(run)) = (ws.guard.allow.take(), &ws.guard.run) else {
                            return;
                        };
                        let (stage, candidate, branch) =
                            run.snapshot(|r| (r.stage, r.candidate.clone(), r.branch.clone()));
                        let repo = guard::repo_root(&path).unwrap_or_else(|_| path.clone());
                        let exception = guard::Exception {
                            id: 0,
                            rule: form.rule,
                            reason: form.reason.trim().to_string(),
                            scope: form.scope,
                            stage,
                            candidate,
                            branch,
                            user: guard::local_user(&repo),
                            created_at: store::now(),
                            revoked: false,
                        };
                        if self.db(|s| s.add_exception(&path, &exception)).is_some() {
                            // Se vuelve a evaluar para aplicar la excepción.
                            self.apply(ctx, UiCmd::GuardRun, area);
                        }
                        return;
                    }
                    UiCmd::GuardExport(id) => {
                        let report = match id {
                            None => ws
                                .guard
                                .run
                                .as_ref()
                                .map(|r| r.snapshot(guard::GuardRun::report)),
                            Some(id) => ws.guard.history.as_ref().and_then(|h| {
                                h.iter().find(|(v, _)| *v == id).map(|(_, r)| r.clone())
                            }),
                        };
                        if let Some(report) = report {
                            ws.guard.notice = Some(
                                export_report(&path, &report).unwrap_or_else(|e| format!("✕ {e}")),
                            );
                        }
                    }
                    UiCmd::HooksInstall | UiCmd::HooksUninstall => {
                        let result = match cmd {
                            UiCmd::HooksInstall => std::env::current_exe()
                                .map_err(|e| e.to_string())
                                .and_then(|exe| {
                                    hooks::install(&path, &exe.canonicalize().unwrap_or(exe))
                                }),
                            _ => hooks::uninstall(&path),
                        };
                        ws.guard.hooks = None;
                        ws.guard.notice =
                            Some(result.map_or_else(|e| format!("✕ {e}"), |r| r.join("\n")));
                    }
                    _ => {
                        if let Err(e) = guard::write_template(&ws.project.path) {
                            self.error = Some(e);
                        }
                    }
                }
            }
            UiCmd::EditConfig => {
                let Some(i) = self.active else { return };
                let file = self.workspaces[i].project.path.join(".forge/project.toml");
                // Editor de texto por defecto de macOS.
                if let Err(e) = std::process::Command::new("open")
                    .arg("-t")
                    .arg(&file)
                    .spawn()
                {
                    self.error = Some(format!("no se pudo abrir {}: {e}", file.display()));
                }
            }
            UiCmd::Start(_)
            | UiCmd::Stop(_)
            | UiCmd::Restart(_)
            | UiCmd::ShowProcess(_)
            | UiCmd::StartAll
            | UiCmd::StopAll => {
                let Some(i) = self.active else { return };
                let ws = &mut self.workspaces[i];
                let ids: Vec<String> = ws.processes.list.iter().map(|m| m.def.id.clone()).collect();
                match cmd {
                    UiCmd::Start(id) => ws.processes.start(ctx, &id),
                    UiCmd::Stop(id) => ws.processes.stop(&id),
                    UiCmd::Restart(id) => ws.processes.restart(ctx, &id),
                    UiCmd::ShowProcess(id) => ws.show_process(ctx, &id, area),
                    UiCmd::StartAll => ids.iter().for_each(|id| ws.processes.start(ctx, id)),
                    _ => ids.iter().for_each(|id| ws.processes.stop(id)),
                }
            }
        }
        self.save();
    }

    /// Barra lateral (estilo Finder/Xcode): workspaces arriba y, del proyecto activo,
    /// agentes, procesos, comandos y accesos a Guard, Memoria y configuración.
    fn sidebar(&mut self, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
        let detected = self.agents.lock().unwrap().clone();
        let detecting = self.detecting.load(Ordering::Relaxed);
        // Hueco para los semáforos de la ventana.
        let body = Rect::from_min_max(
            rect.min + Vec2::new(10.0, theme::TOOLBAR_HEIGHT + 4.0),
            rect.max - Vec2::new(10.0, 8.0),
        );
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
        ui.spacing_mut().item_spacing.y = 1.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                theme::section(ui, "Workspaces");
                if theme::row(
                    ui,
                    icon::HOUSE,
                    theme::TEXT_2,
                    "Inicio",
                    "⌘⇧H",
                    self.active.is_none(),
                )
                .clicked()
                {
                    cmds.push(UiCmd::Home);
                }
                for (i, ws) in self.workspaces.iter().enumerate() {
                    let running = ws.processes.active_count();
                    let detail = match (running, i) {
                        (n, _) if n > 0 => format!("{n} en marcha"),
                        (_, i) if i < 9 => format!("⌘{}", i + 1),
                        _ => String::new(),
                    };
                    let selected = self.active == Some(i);
                    let color = if selected {
                        theme::ACCENT
                    } else {
                        theme::TEXT_2
                    };
                    let row = theme::row(
                        ui,
                        icon::FOLDER,
                        color,
                        &ws.project.name(),
                        &detail,
                        selected,
                    )
                    .on_hover_text(tilde(&ws.project.path));
                    if row.clicked() {
                        cmds.push(UiCmd::Activate(i));
                    }
                    row.context_menu(|ui| {
                        if ui.button(format!("{}  Cerrar proyecto", icon::X)).clicked() {
                            cmds.push(UiCmd::Close(i));
                        }
                    });
                }
                if theme::row(ui, icon::PLUS, theme::TEXT_3, "Abrir carpeta…", "⌘O", false)
                    .clicked()
                {
                    cmds.push(UiCmd::OpenFolder);
                }

                let Some(i) = self.active else { return };
                let ws = &mut self.workspaces[i];
                ui.add_space(12.0);
                Self::agents_ui(ws, &detected, detecting, ui, cmds);
                ui.add_space(12.0);
                Self::processes_ui(ws, ui, cmds);

                if !ws.project.config.commands.is_empty() {
                    ui.add_space(12.0);
                    theme::section(ui, "Comandos");
                    for command in &ws.project.config.commands {
                        let row =
                            theme::row(ui, icon::PLAY, theme::TEXT_3, &command.name, "", false)
                                .on_hover_text(&command.command);
                        if row.clicked() {
                            cmds.push(UiCmd::RunCommand(command.clone()));
                        }
                    }
                }

                ui.add_space(12.0);
                theme::section(ui, "Proyecto");
                let (guard_text, guard_color) = match &ws.guard.run {
                    None => ("sin ejecutar".to_string(), theme::TEXT_3),
                    Some(run) => run.snapshot(|r| verdict_short(r.verdict())),
                };
                if theme::row(
                    ui,
                    icon::SHIELD_CHECK,
                    guard_color,
                    "Guard",
                    &guard_text,
                    ws.guard.open,
                )
                .on_hover_text("⌘G")
                .clicked()
                {
                    cmds.push(UiCmd::ToggleGuard);
                }
                let notes = if ws.memory.count > 0 {
                    ws.memory.count.to_string()
                } else {
                    String::new()
                };
                if theme::row(
                    ui,
                    icon::BRAIN,
                    theme::PURPLE,
                    "Memoria",
                    &notes,
                    ws.memory.open,
                )
                .on_hover_text("⌘⇧M")
                .clicked()
                {
                    cmds.push(UiCmd::ToggleMemory);
                }
                let config = theme::row(ui, icon::GEAR, theme::TEXT_3, "Configuración", "", false);
                egui::Popup::menu(&config).show(|ui| {
                    ui.set_min_width(220.0);
                    if !ws.project.has_config() && ui.button("Crear .forge/project.toml").clicked()
                    {
                        cmds.push(UiCmd::CreateConfig);
                    }
                    if ws.project.has_config() {
                        if ui
                            .button(format!("{}  Editar project.toml", icon::NOTE_PENCIL))
                            .clicked()
                        {
                            cmds.push(UiCmd::EditConfig);
                        }
                        if ui
                            .button(format!("{}  Recargar configuración", icon::ARROW_CLOCKWISE))
                            .clicked()
                        {
                            cmds.push(UiCmd::ReloadConfig);
                        }
                    }
                    if ws.project.default_layout().is_some()
                        && ui.button("Restablecer layout").clicked()
                    {
                        cmds.push(UiCmd::ResetLayout);
                    }
                });
            });
    }

    /// Sección AGENTES: instalados (con versión) y configurados; clic abre, clic derecho reanuda.
    fn agents_ui(
        ws: &Workspace,
        detected: &HashMap<String, agents::Detection>,
        detecting: bool,
        ui: &mut egui::Ui,
        cmds: &mut Vec<UiCmd>,
    ) {
        ui.horizontal(|ui| {
            theme::section(ui, "Agentes");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if detecting {
                    ui.spinner();
                } else if theme::icon_button_sized(
                    ui,
                    icon::ARROW_CLOCKWISE,
                    "Volver a detectar agentes",
                    20.0,
                    theme::TEXT_4,
                )
                .clicked()
                {
                    cmds.push(UiCmd::DetectAgents);
                }
            });
        });
        let default = agents::default_agent(&ws.project.agents, detected).map(|a| a.id.clone());
        let mut missing = Vec::new();
        for agent in ws.project.agents.iter().filter(|a| a.enabled) {
            let found = detected.get(agent.program());
            let installed = found.is_some_and(|d| d.path.is_some());
            if !installed && !agent.configured {
                if found.is_some() {
                    missing.push(agent);
                }
                continue;
            }
            let star = if default.as_deref() == Some(agent.id.as_str()) {
                "  ★"
            } else {
                ""
            };
            let version = found
                .and_then(|d| d.version.as_deref())
                .map(short_version)
                .unwrap_or_default();
            let color = if installed {
                theme::ORANGE
            } else {
                theme::TEXT_4
            };
            let tip = match found.and_then(|d| d.path.as_ref()) {
                Some(path) => format!(
                    "{}\n{}\nClic: abrir · clic derecho: reanudar y más",
                    agent.command,
                    path.display()
                ),
                None => format!("{} no está instalado", agent.program()),
            };
            let row = theme::row(
                ui,
                icon::ROBOT,
                color,
                &format!("{}{star}", agent.name),
                &version,
                false,
            )
            .on_hover_text(tip);
            if row.clicked() && installed {
                cmds.push(UiCmd::OpenAgent(agent.id.clone(), false));
            }
            row.context_menu(|ui| {
                ui.set_min_width(240.0);
                if ui
                    .add_enabled(
                        installed,
                        egui::Button::new(format!("{}  Abrir", icon::TERMINAL_WINDOW)),
                    )
                    .clicked()
                {
                    cmds.push(UiCmd::OpenAgent(agent.id.clone(), false));
                }
                if let Some(resume) = &agent.resume {
                    let button = egui::Button::new(format!(
                        "{}  Reanudar última sesión",
                        icon::CLOCK_COUNTER_CLOCKWISE
                    ));
                    if ui
                        .add_enabled(installed, button)
                        .on_hover_text(resume)
                        .clicked()
                    {
                        cmds.push(UiCmd::OpenAgent(agent.id.clone(), true));
                    }
                }
                if ui
                    .button(format!("{}  Copiar comando", icon::COPY))
                    .clicked()
                {
                    ui.ctx().copy_text(agent.command.clone());
                }
                ui.separator();
                for cap in agent.capabilities() {
                    ui.label(
                        RichText::new(format!("• {cap}"))
                            .small()
                            .color(theme::TEXT_3),
                    );
                }
                if !installed
                    && let Some(hint) = &agent.install_hint {
                        ui.label(
                            RichText::new(format!("Instalar: {hint}"))
                                .small()
                                .monospace(),
                        );
                    }
            });
        }
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|a| a.name.as_str()).collect();
            let hints: Vec<String> = missing
                .iter()
                .filter_map(|a| a.install_hint.as_ref().map(|h| format!("{}: {h}", a.name)))
                .collect();
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("   No instalados: {}", names.join(", ")))
                    .size(11.0)
                    .color(theme::TEXT_4),
            )
            .on_hover_text(hints.join("\n"));
        }
    }

    /// Sección PROCESOS: estado, puerto y controles (aparecen al pasar el ratón).
    fn processes_ui(ws: &Workspace, ui: &mut egui::Ui, cmds: &mut Vec<UiCmd>) {
        ui.horizontal(|ui| {
            theme::section(ui, "Procesos");
            if !ws.processes.list.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button_sized(
                        ui,
                        icon::STOP,
                        "Detener todos",
                        20.0,
                        theme::TEXT_4,
                    )
                    .clicked()
                    {
                        cmds.push(UiCmd::StopAll);
                    }
                    if theme::icon_button_sized(
                        ui,
                        icon::PLAY,
                        "Iniciar todos",
                        20.0,
                        theme::TEXT_4,
                    )
                    .clicked()
                    {
                        cmds.push(UiCmd::StartAll);
                    }
                });
            }
        });
        if ws.processes.list.is_empty() {
            ui.label(
                RichText::new("   Define [[processes]] en .forge/project.toml")
                    .size(11.0)
                    .color(theme::TEXT_4),
            );
        }
        for m in &ws.processes.list {
            let id = m.def.id.clone();
            let (status, tone) = m.describe();
            let detail = match m.ports.first() {
                Some(port) => format!(":{port}"),
                None => status.clone(),
            };
            let row = theme::row(ui, "●", tone_color(tone), m.def.label(), &detail, false)
                .on_hover_text(format!(
                    "{}\n{status}\nClic: ver logs · clic derecho: más opciones",
                    m.def.command
                ));
            if row.clicked() {
                cmds.push(UiCmd::ShowProcess(id.clone()));
            }
            // Controles al pasar el ratón, sobre el lado derecho de la fila.
            if ui.rect_contains_pointer(row.rect) {
                let size = 22.0;
                let mut x = row.rect.max.x - 2.0;
                let mut button = |glyph: &str, tip: &str, cmd: UiCmd, ui: &mut egui::Ui| {
                    x -= size;
                    let r = Rect::from_min_size(
                        egui::pos2(x, row.rect.center().y - size / 2.0),
                        Vec2::splat(size),
                    );
                    ui.painter().rect_filled(r, 6.0, theme::SURFACE);
                    let resp = ui
                        .interact(r, egui::Id::new(("proc-btn", &id, glyph)), Sense::click())
                        .on_hover_text(tip);
                    theme::paint_icon_button(ui, r, glyph, &resp, theme::TEXT_2);
                    if resp.clicked() {
                        cmds.push(cmd);
                    }
                };
                button(
                    icon::ARROW_CLOCKWISE,
                    "Reiniciar",
                    UiCmd::Restart(id.clone()),
                    ui,
                );
                if m.is_active() {
                    button(icon::STOP, "Detener (Ctrl+C)", UiCmd::Stop(id.clone()), ui);
                } else {
                    button(icon::PLAY, "Iniciar", UiCmd::Start(id.clone()), ui);
                }
            }
            row.context_menu(|ui| {
                ui.set_min_width(240.0);
                if ui
                    .button(format!("{}  Ver logs", icon::TERMINAL_WINDOW))
                    .clicked()
                {
                    cmds.push(UiCmd::ShowProcess(id.clone()));
                }
                if ui
                    .button(format!("{}  Copiar comando", icon::COPY))
                    .clicked()
                {
                    ui.ctx().copy_text(m.def.command.clone());
                }
                ui.menu_button(
                    format!("{}  Variables de entorno", icon::LIST_CHECKS),
                    |ui| {
                        let env = ws.processes.environment(&m.def);
                        if env.is_empty() {
                            ui.label("Sin variables propias");
                        }
                        let mut keys: Vec<_> = env.keys().collect();
                        keys.sort();
                        for key in keys {
                            ui.label(
                                RichText::new(format!("{key}={}", processes::mask(key, &env[key])))
                                    .monospace(),
                            );
                        }
                    },
                );
                ui.separator();
                let restart = match m.def.auto_restart {
                    AutoRestart::Never => "no",
                    AutoRestart::OnFailure => "si falla",
                    AutoRestart::Always => "siempre",
                };
                ui.label(
                    RichText::new(format!("Estado: {status}"))
                        .small()
                        .color(theme::TEXT_3),
                );
                ui.label(
                    RichText::new(format!("Reinicio automático: {restart}"))
                        .small()
                        .color(theme::TEXT_3),
                );
                // Puerto/URL solo como texto (sin abrir navegador).
                if let Some(url) = m.urls().first() {
                    ui.label(RichText::new(url).small().monospace().color(theme::TEXT_3));
                }
            });
        }
    }

    /// Inicio: proyectos recientes como lista agrupada, con abrir y arrastrar carpeta.
    fn home(&mut self, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
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
    fn toolbar(&mut self, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
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
    fn status_bar(&self, ui: &egui::Ui, rect: Rect) {
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
    fn window_drag(&self, ui: &egui::Ui, rect: Rect) {
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
    fn ram_tick(&mut self, ctx: &egui::Context) {
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

    /// Panel de memoria: búsqueda, filtro por tipo, notas nuevas y estado de la conexión con agentes.
    fn memory_ui(ws: &mut Workspace, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
        use crate::memory::{KINDS, kind_label};
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
                RichText::new("Memoria")
                    .size(15.0)
                    .color(theme::TEXT)
                    .strong(),
            );
            ui.label(
                RichText::new(format!("{} · {} notas", ws.project.name(), ws.memory.count))
                    .size(13.0)
                    .color(theme::TEXT_3),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::X, "Cerrar (⌘⇧M)").clicked() {
                    cmds.push(UiCmd::ToggleMemory);
                }
            });
        });

        // Conexión con los agentes.
        let memory = &ws.project.config.memory;
        let server = ws.mcp_server();
        let (sharing, color) = match &server {
            Some(_) if memory.provider == crate::project::MemoryProvider::Mcp => (
                format!(
                    "Agentes conectados a la memoria externa: {}",
                    memory.command.as_deref().unwrap_or_default()
                ),
                tone_color(processes::Tone::Ok),
            ),
            Some(_) => (
                "Compartida con Claude Code y Codex al abrirlos desde Forge (MCP).".to_string(),
                tone_color(processes::Tone::Ok),
            ),
            None => (
                "No se comparte con los agentes ([memory] share_with_agents = false).".to_string(),
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
                .link(RichText::new(format!("{}  Copiar comando MCP", icon::COPY)).size(12.0))
                .on_hover_text(format!(
                    "Para OpenCode u otros clientes (opencode mcp add):\n{command}"
                ))
                .clicked()
            {
                ui.ctx().copy_text(command);
            }
        }

        ui.horizontal(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut ws.memory.query)
                    .hint_text(format!(
                        "{}  Buscar (sin tildes también)",
                        icon::MAGNIFYING_GLASS
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
                .map_or("Todos", kind_label)
                .to_string();
            egui::ComboBox::from_id_salt("memory-kind")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(ws.memory.kind.is_none(), "Todos")
                        .clicked()
                    {
                        ws.memory.kind = None;
                        ws.memory.dirty = true;
                    }
                    for (id, label) in KINDS {
                        if ui
                            .selectable_label(ws.memory.kind.as_deref() == Some(id), label)
                            .clicked()
                        {
                            ws.memory.kind = Some(id.to_string());
                            ws.memory.dirty = true;
                        }
                    }
                });
        });
        if ws.memory.form.is_none()
            && theme::secondary(&mut ui, format!("{}  Nueva nota", icon::PLUS)).clicked()
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
                                ui.selectable_value(&mut form.kind, id.to_string(), label);
                            }
                        });
                    ui.add(
                        egui::TextEdit::singleline(&mut form.title)
                            .hint_text("Título")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut form.body)
                            .hint_text("Qué y por qué (p. ej. causa del error y cómo se resolvió)")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut form.tags)
                            .hint_text("Etiquetas, separadas por comas")
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !form.title.trim().is_empty(),
                                egui::Button::new("Guardar"),
                            )
                            .clicked()
                        {
                            cmds.push(UiCmd::MemorySave);
                        }
                        cancel = ui.button("Cancelar").clicked();
                    });
                });
            if cancel {
                ws.memory.form = None;
            }
        }
        ui.separator();

        if ws.memory.results.is_empty() {
            let text = if ws.memory.query.trim().is_empty() {
                "Todavía no hay notas. Guarda decisiones, errores resueltos o convenciones; los agentes también pueden hacerlo."
            } else {
                "Sin resultados."
            };
            ui.label(RichText::new(text).color(Color32::from_gray(0x90)));
        }
        egui::ScrollArea::vertical()
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
                                if ui.small_button("Borrar").clicked() {
                                    cmds.push(UiCmd::MemoryDelete(m.id));
                                }
                            });
                        });
                }
            });
    }

    /// Guarda las ejecuciones terminadas (evidencia + historial) y vigila si el candidato
    /// cambia después de validar.
    fn guard_tick(&mut self, ctx: &egui::Context) {
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
                crate::router::RouterConfig::load(&path).map_or(0.0, |c| c.monthly_budget_usd);
            self.workspaces[i].guard.history = Some(history);
            self.workspaces[i].guard.ai_spend = Some((spent, budget));
        }
    }

    /// Panel Project Guard: resumen y acciones arriba, controles en una tarjeta debajo,
    /// y hooks, consumo e historial al final.
    fn guard_ui(ws: &mut Workspace, ui: &mut egui::Ui, rect: Rect, cmds: &mut Vec<UiCmd>) {
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
                if theme::icon_button(ui, icon::X, "Cerrar (⌘G)").clicked() {
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
                ui.label(RichText::new("Comprueba si el proyecto está listo con reglas deterministas: tamaño del diff, archivos prohibidos, secretos y tus checks.").color(theme::TEXT_2));
                ui.add_space(4.0);
                if theme::primary(ui, format!("{}  Ejecutar", icon::PLAY)).clicked() {
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
                        RichText::new(format!(
                            "{done} de {total} controles · {} · hace {}",
                            state.stage.label(),
                            human_secs(ago)
                        ))
                        .size(12.0)
                        .color(theme::TEXT_3),
                    );
                });
            });
            if ws.guard.stage != state.stage {
                ui.label(
                    RichText::new(format!(
                        "Resultado de {}: pulsa Ejecutar para {}.",
                        state.stage.label(),
                        ws.guard.stage.label()
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
                        "Preparar todo y crear commit"
                    } else {
                        "Crear commit"
                    };
                    if theme::primary(ui, format!("{}  {label}", icon::GIT_BRANCH)).clicked() {
                        cmds.push(UiCmd::GuardCommit);
                    }
                }
                if running {
                    if theme::secondary(ui, format!("{}  Cancelar", icon::STOP)).clicked() {
                        cmds.push(UiCmd::GuardCancel);
                    }
                } else if can_commit {
                    if theme::secondary(ui, format!("{}  Repetir", icon::ARROW_CLOCKWISE)).clicked()
                    {
                        cmds.push(UiCmd::GuardRun);
                    }
                } else if theme::primary(ui, format!("{}  Ejecutar", icon::PLAY)).clicked() {
                    cmds.push(UiCmd::GuardRun);
                }
                if theme::secondary(ui, format!("{}  Ver diff", icon::GIT_DIFF)).clicked() {
                    cmds.push(UiCmd::GuardDiff);
                }
                if state.finished()
                    && state.frozen.is_some()
                    && theme::secondary(ui, format!("{}  Exportar", icon::EXPORT)).clicked()
                {
                    cmds.push(UiCmd::GuardExport(None));
                }
            });
            if let Some(c) = &state.frozen {
                let place = if c.isolated {
                    " · checks en copia aislada"
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
                .on_hover_text(format!("Árbol {}\nDiff {}", c.tree, c.diff_hash));
            }
        });

        if ws.guard.stale {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x3d, 0x33, 0x12))
                .stroke(egui::Stroke::new(1.0, theme::YELLOW.gamma_multiply(0.5)))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(&mut ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("{}  El proyecto cambió después de la validación", icon::WARNING_CIRCLE)).color(theme::YELLOW).strong());
                    ui.label(RichText::new("La evidencia ya no corresponde al estado actual. Ejecuta la revisión de nuevo.").size(12.0).color(theme::TEXT_2));
                });
        }
        if !ws.project.path.join(".forge/rules.toml").exists() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Sin .forge/rules.toml: solo se analiza el diff.")
                        .size(12.0)
                        .color(theme::TEXT_3),
                );
                if ui
                    .link(RichText::new("Crear rules.toml").size(12.0))
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
                ui.label(RichText::new(format!("Omitir: {}", form.label)).strong());
                ui.label(
                    RichText::new("Queda registrada con tu usuario, la fecha y el diff actual.")
                        .size(12.0)
                        .color(theme::TEXT_3),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut form.reason)
                        .hint_text("Motivo (obligatorio), p. ej. «archivos generados por Drizzle»")
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
                                RichText::new("Registrar excepción").color(Color32::WHITE),
                            )
                            .fill(theme::ACCENT),
                        )
                        .clicked()
                    {
                        cmds.push(UiCmd::GuardAllow);
                    }
                    if theme::secondary(ui, "Cancelar").clicked() {
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
                        ui.label(RichText::new("Analizando el diff…").color(theme::TEXT_3));
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
                                .link(RichText::new("Omitir con motivo…").size(11.5))
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
                                .link(RichText::new("Omitir con motivo…").size(11.5))
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
                        (icon::CHECK_CIRCLE, theme::GREEN, "Hooks de Git instalados")
                    } else {
                        (icon::CIRCLE, theme::TEXT_4, "Hooks de Git no instalados")
                    };
                    ui.label(
                        RichText::new(format!("{glyph}  {text}"))
                            .size(12.0)
                            .color(color),
                    )
                    .on_hover_text(
                        "pre-commit y pre-push ejecutan el Guard también fuera de Forge",
                    );
                    let action = if installed { "Quitar" } else { "Instalar" };
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
                        RichText::new(format!("Hooks: {e}"))
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
                RichText::new(format!("Historial ({})", history.len()))
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
                                if ui.link(RichText::new("Exportar").size(12.0)).clicked() {
                                    cmds.push(UiCmd::GuardExport(Some(*id)));
                                }
                            });
                        }
                    });
            });
        }
    }

    /// Aviso de error flotante (abajo a la derecha); clic para cerrarlo.
    fn error_banner(&mut self, ui: &egui::Ui, area: Rect) {
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut cmds = Vec::new();

        if let Some(picked) = self.picker.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.picker = None;
            if let Some(path) = picked {
                cmds.push(UiCmd::Open(path));
            }
        }
        // Carpetas arrastradas a la ventana.
        let (dropped, hovering) =
            ctx.input(|i| (i.raw.dropped_files.clone(), !i.raw.hovered_files.is_empty()));
        for file in dropped {
            let path = file.path();
            cmds.push(UiCmd::Open(if path.is_dir() {
                path.to_path_buf()
            } else {
                path.parent().unwrap_or(path).to_path_buf()
            }));
        }

        // Los atajos se retiran de la cola para que no lleguen al shell.
        let actions = ctx.input_mut(|i| {
            let mut actions = Vec::new();
            i.events.retain(|event| match event {
                egui::Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } => match shortcut(*key, *modifiers) {
                    Some(action) => {
                        if *pressed {
                            actions.push(action);
                        }
                        false
                    }
                    None => true,
                },
                _ => true,
            });
            actions
        });
        let mut ws_actions = Vec::new();
        for action in actions {
            let font_size = &mut self.settings.font_size;
            match action {
                Action::Ws(a) => ws_actions.push(a),
                Action::FontBigger => *font_size = (*font_size + 1.0).min(48.0),
                Action::FontSmaller => *font_size = (*font_size - 1.0).max(8.0),
                Action::FontReset => *font_size = load_settings().0.font_size,
                Action::Home => cmds.push(UiCmd::Home),
                Action::Switch(i) if i < self.workspaces.len() => cmds.push(UiCmd::Activate(i)),
                Action::Switch(_) => {}
                Action::OpenFolder => cmds.push(UiCmd::OpenFolder),
                Action::CloseProject => {
                    if let Some(i) = self.active {
                        cmds.push(UiCmd::Close(i));
                    }
                }
                Action::ToggleSidebar => self.sidebar = !self.sidebar,
                Action::ToggleGuard => cmds.push(UiCmd::ToggleGuard),
                Action::OpenDefaultAgent => cmds.push(UiCmd::OpenDefaultAgent),
                Action::ToggleMemory => cmds.push(UiCmd::ToggleMemory),
            }
        }

        let full = ui.max_rect();
        // Fondo opaco de toda la ventana (barra lateral, contenido y barras).
        let side_width = if self.sidebar { SIDEBAR } else { 0.0 };
        let (side, content) = full.split_left_right_at_x(full.min.x + side_width);
        ui.painter().rect_filled(content, 0.0, theme::BG);
        let (toolbar, below) = content.split_top_bottom_at_y(content.min.y + theme::TOOLBAR_HEIGHT);
        let (mut area, status) = below.split_top_bottom_at_y(below.max.y - theme::STATUS_HEIGHT);
        // Arrastre de la ventana: barra de herramientas y franja superior de la barra lateral
        // (se registra antes que sus botones, que tienen prioridad).
        self.window_drag(ui, toolbar);
        if self.sidebar {
            ui.painter().rect_filled(side, 0.0, theme::SIDEBAR);
            theme::hairline(ui, side, true);
            let strip =
                Rect::from_min_size(side.min, Vec2::new(side.width(), theme::TOOLBAR_HEIGHT));
            ui.interact(
                strip,
                egui::Id::new("sidebar-drag"),
                Sense::click_and_drag(),
            )
            .drag_started()
            .then(|| {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            });
            self.sidebar(ui, side, &mut cmds);
        }
        self.toolbar(ui, toolbar, &mut cmds);
        self.status_bar(ui, status);
        self.ram_tick(&ctx);
        // Panel Guard o Memoria a la derecha del workspace activo (uno a la vez).
        if let Some(i) = self.active {
            let ws = &mut self.workspaces[i];
            if ws.guard.open || ws.memory.open {
                let (rest, drawer) = area.split_left_right_at_x(area.max.x - GUARD_WIDTH);
                if ws.guard.open {
                    Self::guard_ui(ws, ui, drawer, &mut cmds);
                } else {
                    Self::memory_ui(ws, ui, drawer, &mut cmds);
                }
                area = rest;
            }
        }

        // Los procesos de todos los proyectos avanzan aunque no estén a la vista
        // (reinicio automático, paradas, puertos y health checks).
        let mut busy = false;
        for ws in &mut self.workspaces {
            busy |= ws.processes.tick(&ctx);
        }
        if busy {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        self.guard_tick(&ctx);

        match self.active {
            Some(i) => {
                let m = metrics(&ctx, &self.settings);
                let ws = &mut self.workspaces[i];
                ws.ui(ui, area, &m, &ws_actions, true);
                if let Some(e) = ws.error.take() {
                    self.error = Some(e);
                }
                if ws.is_empty() {
                    cmds.push(UiCmd::Close(i));
                }
            }
            None => self.home(ui, area, &mut cmds),
        }

        if hovering {
            ui.painter()
                .rect_filled(full, 0.0, Color32::from_black_alpha(160));
            ui.painter().text(
                full.center(),
                egui::Align2::CENTER_CENTER,
                "Suelta la carpeta para abrirla como proyecto",
                FontId::proportional(20.0),
                Color32::WHITE,
            );
        }

        for cmd in cmds {
            self.apply(&ctx, cmd, area);
        }

        let title = self
            .active
            .map_or_else(|| "Forge".into(), |i| self.workspaces[i].title());
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        if self.last_save.elapsed() > SAVE_EVERY {
            self.save();
        }
        self.error_banner(ui, area);
        // Guardado periódico aunque no haya eventos.
        ctx.request_repaint_after(SAVE_EVERY);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts() {
        let cmd = Modifiers::MAC_CMD | Modifiers::COMMAND;
        assert_eq!(
            shortcut(Key::D, cmd),
            Some(Action::Ws(WsAction::Split(Dir::Row)))
        );
        assert_eq!(
            shortcut(Key::D, cmd | Modifiers::SHIFT),
            Some(Action::Ws(WsAction::Split(Dir::Column)))
        );
        assert_eq!(
            shortcut(Key::W, cmd | Modifiers::SHIFT),
            Some(Action::CloseProject)
        );
        assert_eq!(shortcut(Key::Num2, cmd), Some(Action::Switch(1)));
        assert_eq!(
            shortcut(Key::ArrowLeft, cmd | Modifiers::ALT),
            Some(Action::Ws(WsAction::Focus(Toward::Left)))
        );
        // ⌘← (inicio de línea) y Ctrl+D (EOF) siguen siendo del shell.
        assert_eq!(shortcut(Key::ArrowLeft, cmd), None);
        assert_eq!(shortcut(Key::D, Modifiers::CTRL), None);
    }
}

/// Capturas de la interfaz sin pantalla (para revisar el diseño):
/// `FORGE_PREVIEW_DIR=/ruta cargo test --release ui_preview -- --ignored --nocapture`
#[cfg(test)]
mod preview {
    use super::*;
    use egui_kittest::Harness;

    fn save(harness: &mut Harness<'_, App>, dir: &Path, name: &str) {
        let image = harness.render().expect("render");
        image
            .save(dir.join(format!("{name}.png")))
            .expect("guardar captura");
    }

    fn settle(harness: &mut Harness<'_, App>) {
        for _ in 0..6 {
            harness.run_steps(3);
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Icono de la app (PNG 1024 con transparencia), para `scripts/install-app.sh`:
    /// `FORGE_ICON_OUT=assets/icon.png cargo test --release app_icon -- --ignored`
    #[test]
    #[ignore]
    fn app_icon() {
        let out = PathBuf::from(std::env::var("FORGE_ICON_OUT").expect("FORGE_ICON_OUT"));
        const SIZE: f32 = 1024.0;
        // Rejilla de iconos de macOS: 824 px de contenido con radio ~185 dentro de 1024.
        let tile = Rect::from_center_size(egui::pos2(SIZE / 2.0, SIZE / 2.0), Vec2::splat(824.0));
        const RADIUS: f32 = 185.0;
        let mut harness = Harness::builder().with_size([SIZE, SIZE]).with_pixels_per_point(1.0).wgpu().build_ui(move |ui| {
            let p = ui.painter();
            p.rect_filled(ui.max_rect(), 0.0, Color32::BLACK);
            p.rect_filled(tile, RADIUS, Color32::from_rgb(0x1e, 0x1e, 0x21));
            // Degradado vertical real (malla con color por vértice); lo que sale de la forma se recorta después.
            let mut mesh = egui::Mesh::default();
            let (top, bottom) = (Color32::from_white_alpha(20), Color32::TRANSPARENT);
            mesh.colored_vertex(tile.left_top(), top);
            mesh.colored_vertex(tile.right_top(), top);
            mesh.colored_vertex(tile.right_bottom(), bottom);
            mesh.colored_vertex(tile.left_bottom(), bottom);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            p.add(mesh);
            p.rect_stroke(tile, RADIUS, egui::Stroke::new(3.0, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
            // Resplandor cálido radial detrás de la llama.
            for i in 0..60 {
                let r = 330.0 * (1.0 - i as f32 / 60.0);
                p.circle_filled(egui::pos2(SIZE / 2.0, 470.0), r, Color32::from_rgba_unmultiplied(0xff, 0x8a, 0x00, 2));
            }
            p.text(egui::pos2(SIZE / 2.0, 455.0), egui::Align2::CENTER_CENTER, icon::FLAME, FontId::proportional(430.0), theme::ORANGE);
            p.text(egui::pos2(SIZE / 2.0, 735.0), egui::Align2::CENTER_CENTER, ">_", FontId::monospace(120.0), Color32::from_rgb(0xe5, 0xe5, 0xea));
        });
        crate::install_fonts(&harness.ctx, &Settings::default()).unwrap();
        harness.run_steps(3);
        let mut image = harness.render().expect("render");
        // Fuera del cuadrado redondeado: transparente (con borde suavizado).
        for (x, y, px) in image.enumerate_pixels_mut() {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = (tile.min.x + RADIUS - cx).max(cx - (tile.max.x - RADIUS)).max(0.0);
            let dy = (tile.min.y + RADIUS - cy).max(cy - (tile.max.y - RADIUS)).max(0.0);
            let inside = (RADIUS - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let inside = if cx < tile.min.x || cx > tile.max.x || cy < tile.min.y || cy > tile.max.y { 0.0 } else { inside };
            px.0[3] = (px.0[3] as f32 * inside) as u8;
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        image.save(&out).expect("guardar icono");
    }

    #[test]
    #[ignore]
    fn ui_preview() {
        let out = PathBuf::from(std::env::var("FORGE_PREVIEW_DIR").expect("FORGE_PREVIEW_DIR"));
        std::fs::create_dir_all(&out).unwrap();
        let base = std::env::temp_dir().join(format!("forge-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Bovinapp/.forge")).unwrap();
        // Ruta canónica (/private/var…): la que guarda la app al abrir el proyecto.
        let project = base.join("Bovinapp").canonicalize().unwrap();
        std::fs::create_dir_all(project.join("src/vacunas")).unwrap();
        let sh = |c: &str| {
            assert!(
                std::process::Command::new("sh")
                    .args(["-c", c])
                    .current_dir(&project)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        sh(
            "git init -q -b main && git config user.email t@t && git config user.name t && echo '# Bovinapp' > README.md && git add . && git commit -qm inicio",
        );
        std::fs::write(
            project.join(".forge/project.toml"),
            "[project]\nname = \"Bovinapp\"\n\
             [[commands]]\nname = \"Tests\"\ncommand = \"npm test\"\n\
             [[processes]]\nid = \"frontend\"\nname = \"Frontend\"\ncommand = \"sleep 600\"\nrestart = \"on-workspace-open\"\n\
             [[processes]]\nid = \"api\"\nname = \"API\"\ncommand = \"sleep 600\"\n",
        )
        .unwrap();
        std::fs::write(project.join(".forge/rules.toml"), "[commit]\nrequire_tests = true\n[[checks]]\nid = \"tests\"\nname = \"Tests\"\ncommand = \"sleep 0.3\"\n").unwrap();
        std::fs::write(
            project.join("src/vacunas/lotes.ts"),
            "export const lote = 1;\n",
        )
        .unwrap();
        sh("git add src");
        // SAFETY: test aislado; la base de datos de la vista previa no es la del usuario.
        unsafe { std::env::set_var("FORGE_DB", base.join("forge.db")) };
        {
            let store = Store::open(&base.join("forge.db")).unwrap();
            for (name, ago) in [("ReparAppi", 86_400), ("wabio", 5 * 86_400)] {
                let p = base.join(name);
                std::fs::create_dir_all(&p).unwrap();
                store.touch(&p, name).unwrap();
                store.conn.execute("UPDATE projects SET last_opened = last_opened - ?1, is_open = 0 WHERE path = ?2", rusqlite::params![ago, p.to_string_lossy()]).unwrap();
            }
            store
                .add_memory(
                    &project,
                    "decision",
                    "Vacunación por lotes",
                    "Se registran por lote para no duplicar animales.",
                    "vacunas",
                    "claude-code",
                )
                .unwrap();
            store
                .add_memory(
                    &project,
                    "error",
                    "Migración con índice duplicado",
                    "drizzle-kit falló; se renombró el índice.",
                    "",
                    "usuario",
                )
                .unwrap();
        }

        let project_arg = project.clone();
        let mut harness = Harness::builder()
            .with_size([1440.0, 880.0])
            .with_pixels_per_point(2.0)
            .wgpu()
            .build_eframe(move |cc| {
                crate::install_fonts(&cc.egui_ctx, &Settings::default()).unwrap();
                App::new(&cc.egui_ctx, Settings::default(), None, Some(project_arg))
            });
        settle(&mut harness);
        // Dos paneles más: división a la derecha y abajo.
        let ctx = harness.ctx.clone();
        harness.state_mut().workspaces[0].show_process(
            &ctx,
            "frontend",
            Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0)),
        );
        settle(&mut harness);
        save(&mut harness, &out, "1-workspace");

        harness.state_mut().workspaces[0].guard.open = true;
        harness
            .state_mut()
            .apply(&ctx, UiCmd::GuardRun, Rect::NOTHING);
        for _ in 0..12 {
            settle(&mut harness);
            if harness.state().workspaces[0]
                .guard
                .run
                .as_ref()
                .is_some_and(|r| r.snapshot(guard::GuardRun::finished))
            {
                break;
            }
        }
        settle(&mut harness);
        save(&mut harness, &out, "2-guard");

        harness.state_mut().workspaces[0].guard.open = false;
        harness.state_mut().workspaces[0].memory.open = true;
        harness.state_mut().workspaces[0].memory.dirty = true;
        settle(&mut harness);
        save(&mut harness, &out, "3-memoria");

        harness.state_mut().active = None;
        settle(&mut harness);
        save(&mut harness, &out, "4-inicio");
    }
}
