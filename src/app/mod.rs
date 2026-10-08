// Ventana principal: inicio con proyectos recientes, barra lateral y workspaces abiertos.
use forge_core::tr;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, FontId, Key, Modifiers, Rect, RichText, Sense, Vec2};

use crate::layout::{Dir, Toward};
use crate::processes;
use crate::theme::{self, icon};
use crate::workspace::{AllowForm, NoteForm, Workspace, WsAction, tilde, tone_color};
use crate::{Settings, load_settings, metrics};
use forge_core::agents;
use forge_core::candidate;
use forge_core::guard::{self, Rules, Stage};
use forge_core::hooks;
use forge_core::project::{self, AutoRestart, Project, SavedCommand};
use forge_core::store::{self, Store};

const SIDEBAR: f32 = 240.0;
const GUARD_WIDTH: f32 = 440.0;

mod approvals;
mod chrome;
mod guard_panel;
mod ideas_panel;
mod memory_panel;
mod notify;
mod palette;
mod settings;
mod sidebar;

use guard_panel::*;
use ideas_panel::IdeasTarget;

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
    ToggleIdeas,
    Palette,
    Settings,
    /// ⌃Tab: proyecto usado antes (repetido rápido, sigue retrocediendo).
    NextRecent,
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
        (Key::F, false, false) => Action::Ws(WsAction::Find),
        (Key::A, true, false) => Action::OpenDefaultAgent,
        (Key::M, true, false) => Action::ToggleMemory,
        (Key::I, true, false) => Action::ToggleIdeas,
        (Key::K, false, false) => Action::Palette,
        (Key::Comma, false, false) => Action::Settings,
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
    /// Abre en el editor un archivo de `.forge/` del proyecto activo.
    EditFile(&'static str),
    /// Abre la ventana de Ajustes.
    OpenSettings,
    /// Abre la paleta de comandos (⌘K).
    OpenPalette,
    /// Cambia el idioma de la interfaz (se guarda para la próxima vez).
    SetLang(forge_core::i18n::Lang),
    /// Duerme el proyecto: cierra sus terminales y procesos y conserva el layout.
    Sleep(usize),
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
    ToggleIdeas,
    /// Abre Ideas con el foco en "Nueva idea…" (la general si no hay proyecto activo).
    NewIdea,
    IdeaAdd(IdeasTarget),
    IdeaSet(IdeasTarget, i64, &'static str),
    /// Abre (Some) o cierra (None) la edición de una idea.
    IdeaEdit(IdeasTarget, Option<i64>),
    IdeaSave(IdeasTarget),
    IdeaDelete(IdeasTarget, i64),
    MemorySave,
    /// Abre la memoria con el formulario de nota nueva.
    NewNote,
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
    /// Abre (true) o cierra el formulario del pull request.
    PrForm(bool),
    PrCreate,
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
    /// RAM de cada proyecto despierto (terminales, agentes y procesos).
    ram: Arc<Mutex<HashMap<PathBuf, u64>>>,
    ram_checked: Option<Instant>,
    /// Paleta de comandos (⌘K) abierta.
    palette: Option<palette::Palette>,
    /// Solicitudes de comandos peligrosos pendientes (las vigila un hilo).
    approvals: Arc<Mutex<Vec<forge_core::danger::Request>>>,
    approvals_dir: Option<PathBuf>,
    /// Última solicitud por la que se trajo la ventana al frente.
    approval_seen: Option<String>,
    /// Ideas generales (sin proyecto), en Inicio.
    general: crate::workspace::IdeasView,
    /// Proyectos de más a menos recientemente usados (⌃Tab).
    mru: Vec<PathBuf>,
    /// ⌃Tab con Ctrl aún pulsado: (orden de proyectos al empezar, posición actual).
    /// Al soltar Ctrl se olvida: el siguiente ⌃Tab vuelve a alternar con el anterior.
    mru_cycle: Option<(Vec<PathBuf>, usize)>,
    /// Barra lateral con todos los proyectos (con muchos se compacta).
    show_all: bool,
    /// Notificaciones de macOS (desactivadas en tests).
    notifications: bool,
    /// Proyecto de la notificación en la que el usuario hizo clic.
    notification_click: Arc<Mutex<Option<PathBuf>>>,
    /// Ventana de Ajustes (⌘,) abierta.
    settings_open: bool,
    /// Notificaciones activadas en Ajustes.
    notifications_enabled: bool,
    /// Ancho de los botones de la derecha de la barra superior (fotograma anterior).
    toolbar_right: f32,
    /// Dónde acaban los semáforos de macOS (x); ahí empiezan los botones de la ventana.
    traffic_end: f32,
}

impl App {
    pub fn new(
        ctx: &egui::Context,
        settings: Settings,
        error: Option<String>,
        open: Option<PathBuf>,
    ) -> Self {
        let store = Store::default_path()
            .ok_or_else(|| tr!("no se encontró $HOME").to_string())
            .and_then(|p| Store::open(&p));
        let mut app = Self::with_store(ctx, settings, error, open, store);
        app.notifications = true; // solo la app real (los tests no notifican)
        app.notifications_enabled =
            app.db(|s| s.setting("notifications")).flatten().as_deref() != Some("off");
        app
    }

    /// Como `new`, con la base de datos indicada (los tests usan una en memoria).
    pub fn with_store(
        ctx: &egui::Context,
        settings: Settings,
        error: Option<String>,
        open: Option<PathBuf>,
        store: Result<Store, String>,
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
            palette: None,
            traffic_end: 72.0,
            toolbar_right: 0.0,
            settings_open: false,
            notifications_enabled: true,
            notifications: false,
            notification_click: Arc::default(),
            mru: Vec::new(),
            mru_cycle: None,
            show_all: false,
            general: crate::workspace::IdeasView {
                dirty: true,
                ..Default::default()
            },
            approvals: Arc::default(),
            approvals_dir: None,
            approval_seen: None,
        };
        if let Some(dir) = crate::terminal::SHELL_ENV
            .get()
            .and_then(|env| env.get("FORGE_APPROVALS"))
        {
            app.watch_approvals(ctx, PathBuf::from(dir));
        }
        match store {
            Ok(store) => app.store = Some(store),
            Err(e) => app.error = Some(tr!("{e} (los workspaces no se guardarán)", e = e)),
        }

        // Restaura los proyectos que estaban abiertos al cerrar.
        let open_before = app.db(|s| s.open_projects()).unwrap_or_default();
        for path in open_before {
            if path.is_dir() {
                // Dormidos: solo se despierta el activo (y los demás al entrar en ellos).
                app.open_project_as(ctx, &path, false);
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
        self.open_project_as(ctx, path, true);
    }

    /// Abre un proyecto; dormido (`awake = false`) solo aparece en la lista hasta usarlo.
    fn open_project_as(&mut self, ctx: &egui::Context, path: &Path, awake: bool) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.workspaces.iter().position(|w| w.project.path == path) {
            self.active = Some(i);
            return;
        }
        if !path.is_dir() {
            self.error = Some(tr!("{p0} no es una carpeta", p0 = path.display()));
            return;
        }
        // Configuración inválida: se abre igual con valores por defecto y se avisa.
        let project = Project::load(&path).unwrap_or_else(|e| {
            self.error = Some(e);
            Project {
                path: path.clone(),
                config: Default::default(),
                agents: forge_core::agents::builtins(),
            }
        });
        let saved = self.db(|s| s.workspace(&path)).flatten();
        let name = project.name();
        self.db(|s| s.touch(&path, &name));
        if !awake {
            self.workspaces.push(Workspace::asleep(project, saved));
            return;
        }
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
            .map(|w| (w.project.path.clone(), w.saved_state()))
            .collect();
        for (i, (path, state)) in states.iter().enumerate() {
            if let Some(state) = state {
                self.db(|s| s.save_workspace(path, state, i));
            }
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
    /// Aplica una acción de atajo (o elegida en la paleta).
    fn handle_action(
        &mut self,
        action: Action,
        cmds: &mut Vec<UiCmd>,
        ws_actions: &mut Vec<WsAction>,
    ) {
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
            Action::ToggleIdeas => cmds.push(UiCmd::ToggleIdeas),
            Action::Settings => self.settings_open = !self.settings_open,
            Action::NextRecent => {
                if self.mru.len() < 2 {
                    return;
                }
                let (order, pos) = self.mru_cycle.get_or_insert_with(|| (self.mru.clone(), 0));
                *pos = (*pos + 1) % order.len();
                let target = order[*pos].clone();
                if let Some(i) = self
                    .workspaces
                    .iter()
                    .position(|w| w.project.path == target)
                {
                    cmds.push(UiCmd::Activate(i));
                }
            }
            Action::Palette => {
                if self.palette.take().is_none() {
                    self.open_palette();
                }
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, cmd: UiCmd, area: Rect) {
        match cmd {
            UiCmd::Home => self.active = None,
            UiCmd::Activate(i) => self.active = Some(i),
            UiCmd::Close(i) => self.close_project(i),
            UiCmd::OpenSettings => self.settings_open = true,
            UiCmd::OpenPalette => self.open_palette(),
            UiCmd::EditFile(name) => {
                let Some(i) = self.active else { return };
                let file = self.workspaces[i].project.path.join(".forge").join(name);
                if !file.exists() {
                    let _ = std::fs::create_dir_all(file.parent().unwrap_or(&file));
                    let _ = std::fs::write(&file, "");
                }
                if let Err(e) = std::process::Command::new("open")
                    .arg("-t")
                    .arg(&file)
                    .spawn()
                {
                    self.error = Some(e.to_string());
                }
            }
            UiCmd::SetLang(lang) => {
                forge_core::i18n::set_lang(lang);
                self.db(|s| s.set_setting("language", lang.code()));
            }
            UiCmd::Sleep(i) => {
                self.save();
                if let Some(ws) = self.workspaces.get_mut(i) {
                    ws.sleep();
                }
                if self.active == Some(i) {
                    self.active = None;
                }
            }
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
            UiCmd::ToggleIdeas | UiCmd::NewIdea => match self.active {
                Some(i) => {
                    let ws = &mut self.workspaces[i];
                    let new = matches!(cmd, UiCmd::NewIdea);
                    ws.ideas.open = new || !ws.ideas.open;
                    ws.ideas.focus = new;
                    ws.ideas.dirty = true;
                    if ws.ideas.open {
                        ws.guard.open = false;
                        ws.memory.open = false;
                    }
                }
                None => self.general.focus = true,
            },
            UiCmd::IdeaAdd(_)
            | UiCmd::IdeaSet(..)
            | UiCmd::IdeaEdit(..)
            | UiCmd::IdeaSave(_)
            | UiCmd::IdeaDelete(..) => self.apply_idea(cmd),
            UiCmd::ToggleMemory | UiCmd::NewNote | UiCmd::MemorySave | UiCmd::MemoryDelete(_) => {
                let Some(i) = self.active else { return };
                let path = self.workspaces[i].project.path.clone();
                match cmd {
                    UiCmd::ToggleMemory => {
                        let ws = &mut self.workspaces[i];
                        ws.memory.open = !ws.memory.open;
                        ws.memory.dirty = true;
                        if ws.memory.open {
                            ws.guard.open = false;
                            ws.ideas.open = false;
                        }
                    }
                    UiCmd::NewNote => {
                        let ws = &mut self.workspaces[i];
                        ws.memory.open = true;
                        ws.memory.dirty = true;
                        ws.guard.open = false;
                        ws.ideas.open = false;
                        ws.memory.form = Some(NoteForm {
                            kind: "decision".into(),
                            ..Default::default()
                        });
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
                                tr!(
                                    "No hay ningún agente instalado (Claude Code, Codex, OpenCode…)"
                                )
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
            | UiCmd::HooksUninstall
            | UiCmd::PrForm(_)
            | UiCmd::PrCreate => {
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
                            ws.ideas.open = false;
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
                        ws.guard.pr = None; // el borrador era de la validación anterior
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
                                    routing: forge_core::router::RouterConfig::load(
                                        &ws.project.path,
                                    )
                                    .map_err(|e| {
                                        ws.guard.notice = Some(tr!(
                                            "✕ {e} (revisiones con IA desactivadas)",
                                            e = e
                                        ))
                                    })
                                    .ok(),
                                    month_spent,
                                    reviewers: forge_core::reviewers::load(&ws.project.path)
                                        .unwrap_or_else(|e| {
                                            ws.guard.notice = Some(format!("✕ {e}"));
                                            forge_core::reviewers::builtins()
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
                    UiCmd::PrForm(false) => ws.guard.pr = None,
                    UiCmd::PrForm(true) => {
                        let Some(run) = &ws.guard.run else { return };
                        let report = run.snapshot(|r| r.report());
                        let draft = Rules::load(&ws.project.path)
                            .map(Option::unwrap_or_default)
                            .and_then(|rules| {
                                forge_core::pr::draft(&ws.project.path, &rules, &report)
                            });
                        match draft {
                            Ok(d) => ws.guard.pr = Some(d),
                            Err(e) => self.error = Some(e),
                        }
                    }
                    UiCmd::PrCreate => {
                        let Some(draft) = ws.guard.pr.take() else {
                            return;
                        };
                        match forge_core::pr::command(&ws.project.path, &draft) {
                            Ok(command) => {
                                let command = SavedCommand {
                                    name: tr!("pull request").into(),
                                    command,
                                    working_directory: None,
                                };
                                ws.run_command(ctx, &command, area);
                            }
                            Err(e) => {
                                ws.guard.pr = Some(draft); // se conserva lo escrito
                                self.error = Some(e);
                            }
                        }
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
                    self.error = Some(tr!(
                        "no se pudo abrir {p0}: {e}",
                        p0 = file.display(),
                        e = e
                    ));
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
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
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
                } if *key == Key::Tab && modifiers.ctrl && !modifiers.mac_cmd => {
                    if *pressed {
                        actions.push(Action::NextRecent);
                    }
                    false
                }
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
        if !ctx.input(|i| i.modifiers.ctrl) {
            self.mru_cycle = None;
        }
        let mut ws_actions = Vec::new();
        for action in actions {
            self.handle_action(action, &mut cmds, &mut ws_actions);
        }

        let full = ui.max_rect();
        match self.palette_ui(&ctx, full) {
            Some(palette::Run::Action(action)) => {
                self.handle_action(action, &mut cmds, &mut ws_actions)
            }
            Some(palette::Run::Cmds(list)) => cmds.extend(list),
            None => {}
        }
        // Semáforos de macOS centrados en la barra superior (como Warp).
        #[cfg(target_os = "macos")]
        if let Some(end) = crate::traffic_lights::center(frame, theme::TOOLBAR_HEIGHT as f64) {
            self.traffic_end = end;
        }
        // Barra superior a todo lo ancho; debajo, la barra lateral y el contenido.
        let (toolbar, below) = full.split_top_bottom_at_y(full.min.y + theme::TOOLBAR_HEIGHT);
        let side_width = if self.sidebar { SIDEBAR } else { 0.0 };
        let (side, content) = below.split_left_right_at_x(below.min.x + side_width);
        ui.painter().rect_filled(content, 0.0, theme::BG);
        let (mut area, status) =
            content.split_top_bottom_at_y(content.max.y - theme::STATUS_HEIGHT);
        // Arrastre de la ventana desde la barra superior (sus botones tienen prioridad).
        self.window_drag(ui, toolbar);
        if self.sidebar {
            ui.painter().rect_filled(side, 0.0, theme::SIDEBAR);
            theme::hairline(ui, side, true);
            self.sidebar(ui, side, &mut cmds);
        }
        self.toolbar(ui, toolbar, &mut cmds);
        self.window_buttons(ui, &mut cmds);
        self.status_bar(ui, status);
        self.ram_tick(&ctx);
        // Panel Guard o Memoria a la derecha del workspace activo (uno a la vez).
        if let Some(i) = self.active {
            let ws = &mut self.workspaces[i];
            if ws.guard.open || ws.memory.open || ws.ideas.open {
                let (rest, drawer) = area.split_left_right_at_x(area.max.x - GUARD_WIDTH);
                if ws.guard.open {
                    Self::guard_ui(ws, ui, drawer, &mut cmds);
                } else if ws.memory.open {
                    Self::memory_ui(ws, ui, drawer, &mut cmds);
                } else {
                    Self::ideas_ui(ws, Some(i), ui, drawer, &mut cmds);
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
        self.ideas_tick();
        for ws in &mut self.workspaces {
            ws.refresh_attention();
        }
        self.notify_tick(&ctx);
        // Las ideas visibles se releen cada pocos segundos (los agentes escriben aparte).
        ctx.request_repaint_after(Duration::from_secs(2));

        match self.active {
            Some(i) => {
                let m = metrics(&ctx, &self.settings);
                let ws = &mut self.workspaces[i];
                ws.wake(&ctx);
                if ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
                    ws.mark_seen();
                }
                let path = ws.project.path.clone();
                if self.mru.first() != Some(&path) {
                    self.mru.retain(|p| p != &path);
                    self.mru.insert(0, path);
                }
                let ws = &mut self.workspaces[i];
                ws.ui(
                    ui,
                    area,
                    &m,
                    &ws_actions,
                    self.palette.is_none() && !self.settings_open,
                );
                if let Some(e) = ws.error.take() {
                    self.error = Some(e);
                }
                if ws.is_empty() && !ws.is_dormant() {
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
                tr!("Suelta la carpeta para abrirla como proyecto"),
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
        self.approvals_ui(&ctx);
        let mut from_settings = Vec::new();
        self.settings_ui(&ctx, &mut from_settings);
        for cmd in from_settings {
            self.apply(&ctx, cmd, area);
        }
        // Con la paleta abierta, el resto de la ventana se atenúa (la paleta va encima).
        if self.palette.is_some() {
            ui.painter()
                .rect_filled(full, 0.0, Color32::from_black_alpha(150));
        }
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

#[cfg(test)]
mod e2e;
#[cfg(test)]
mod preview;
