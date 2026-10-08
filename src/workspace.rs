// Workspace de un proyecto: paneles (terminales y logs de procesos), layout, foco y procesos.
use forge_core::tr;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use eframe::egui::{self, Align2, Color32, FontId, Key, Rect, Sense, Vec2};

use crate::layout::{self, Dir, Node, PanelId, Toward};
use crate::processes::{Processes, Tone};
use crate::terminal::{Metrics, Terminal};
use forge_core::project::{LayoutSpec, Project, SavedCommand, SplitSpec, StartPolicy};

/// Estado restaurable de un workspace (se guarda como JSON en la fila del proyecto).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WorkspaceState {
    pub layout: Node,
    pub panels: HashMap<PanelId, PanelState>,
    pub focus: PanelId,
    /// Procesos administrados en marcha al guardar (se reinician al abrir).
    #[serde(default)]
    pub running: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PanelState {
    pub name: Option<String>,
    pub cwd: PathBuf,
    /// Comando con el que se abrió el panel (p. ej. `claude`, `npm run dev`).
    pub command: Option<String>,
    /// Si el panel muestra los logs de un proceso administrado, su id.
    #[serde(default)]
    pub process: Option<String>,
    /// Agente abierto en el panel (al restaurar se reanuda su última sesión).
    #[serde(default)]
    pub agent: Option<String>,
}

const HEADER: f32 = 30.0;

/// Acciones de teclado que afectan a los paneles del workspace activo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WsAction {
    Split(Dir),
    NewTerminal,
    Close,
    ToggleMaximize,
    Focus(Toward),
    Cycle(isize),
    /// ⌘F: buscar en la terminal enfocada.
    Find,
}

enum Content {
    Shell(Terminal),
    /// Logs de un proceso administrado (el proceso vive en `Workspace::processes`).
    Process(String),
}

struct Panel {
    content: Content,
    /// Nombre puesto por el usuario o por el layout.
    name: Option<String>,
    /// Comando con el que se abrió el shell (se vuelve a lanzar al restaurar).
    command: Option<String>,
    /// Agente del panel: al restaurar se reanuda su sesión si el agente lo permite.
    agent: Option<String>,
}

/// `/Users/x/dev` → `~/dev`
pub fn tilde(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").and_then(|h| path.strip_prefix(h).ok().map(PathBuf::from)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

pub fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Ok => crate::theme::GREEN,
        Tone::Busy => crate::theme::YELLOW,
        Tone::Bad => crate::theme::RED,
        Tone::Idle => crate::theme::TEXT_4,
    }
}

/// Estado del panel Guard (⌘G) de este proyecto.
pub struct GuardView {
    pub open: bool,
    pub stage: forge_core::guard::Stage,
    /// Última ejecución (en curso o terminada).
    pub run: Option<forge_core::guard::Handle>,
    /// Error al leer `.forge/rules.toml`.
    pub error: Option<String>,
    /// Formulario abierto para omitir una regla con motivo.
    pub allow: Option<AllowForm>,
    /// Resultado de instalar/quitar hooks.
    pub notice: Option<String>,
    /// Estado de los hooks (se relee cada pocos segundos: lanza git).
    pub hooks: Option<(Instant, forge_core::hooks::Status)>,
    /// La ejecución actual ya se guardó (evidencia + historial).
    pub saved: bool,
    /// El candidato cambió después de validar: la evidencia ya no corresponde.
    pub stale: bool,
    pub stale_checked: Option<Instant>,
    pub stale_rx: Option<std::sync::mpsc::Receiver<bool>>,
    /// Historial de validaciones (se recarga tras guardar).
    pub history: Option<Vec<(i64, forge_core::evidence::Report)>>,
    /// (gasto del mes, presupuesto) de modelos de pago.
    pub ai_spend: Option<(f64, f64)>,
    /// Formulario del pull request (borrador editable antes de publicarlo con `gh`).
    pub pr: Option<forge_core::pr::Draft>,
    /// Terminó una validación mientras el proyecto no estaba a la vista.
    pub unseen: bool,
}

pub struct AllowForm {
    pub rule: String,
    pub label: String,
    pub reason: String,
    pub scope: forge_core::guard::Scope,
}

impl Default for GuardView {
    fn default() -> Self {
        Self {
            open: false,
            stage: forge_core::guard::Stage::Commit,
            run: None,
            error: None,
            allow: None,
            notice: None,
            hooks: None,
            saved: false,
            stale: false,
            stale_checked: None,
            stale_rx: None,
            history: None,
            ai_spend: None,
            pr: None,
            unseen: false,
        }
    }
}

/// Salida mínima de un agente para considerar que trabajó (un redibujo de su barra de
/// estado o un spinner escribe mucho menos).
// ponytail: umbral fijo; si un agente da falsos avisos, medirlo por agente.
const AGENT_WORK_BYTES: u64 = 3_000;

/// Aviso de un proyecto que no está a la vista (punto en la barra lateral).
#[derive(Clone, Debug, PartialEq)]
pub enum Attention {
    /// Un agente trabajó y se quedó quieto: terminó o espera respuesta.
    Agent(String),
    /// Un proceso administrado falló.
    Failed(String),
    /// Guard bloqueó una validación.
    Blocked,
    /// Mensaje del propio agente ("Claude Code terminó", "Claude needs your permission…").
    Event(String),
}

/// Carpeta del historial de las terminales (solo la app real; en tests no se guarda).
pub static HISTORY_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Líneas de historial que se guardan por terminal.
const HISTORY_LINES: usize = 1000;

/// Datos del panel de Git (se leen en segundo plano).
pub type GitData = (
    forge_core::git::Status,
    Vec<String>,
    Vec<forge_core::git::Commit>,
);

/// Panel de Git (⌘⇧G).
#[derive(Default)]
pub struct GitView {
    pub open: bool,
    pub data: Option<GitData>,
    pub error: Option<String>,
    /// Mensaje del commit en preparación.
    pub message: String,
    /// Nombre de la rama nueva (formulario abierto).
    pub new_branch: Option<String>,
    /// Archivo cuyo descarte espera confirmación.
    pub confirm_discard: Option<forge_core::git::FileChange>,
    pub loading: Option<std::sync::mpsc::Receiver<Result<GitData, String>>>,
    pub loaded: Option<Instant>,
    pub dirty: bool,
}

/// Lista de ideas (de un proyecto, o la general en Inicio).
#[derive(Default)]
pub struct IdeasView {
    pub open: bool,
    pub items: Vec<forge_core::ideas::Idea>,
    /// Sin hacer (contador de la barra lateral).
    pub open_count: i64,
    pub show_done: bool,
    /// Campo "Nueva idea…".
    pub input: String,
    /// Dar el foco al campo (al abrir con ⌘K → "Nueva idea").
    pub focus: bool,
    /// Idea en edición: (id, título, nota).
    pub editing: Option<(i64, String, String)>,
    pub dirty: bool,
    /// Última lectura (los agentes escriben desde otros procesos: se relee cada poco).
    pub loaded: Option<Instant>,
}

/// Panel de memoria del proyecto (⌘⇧M).
#[derive(Default)]
pub struct MemoryView {
    pub open: bool,
    pub query: String,
    pub kind: Option<String>,
    pub results: Vec<forge_core::memory::Memory>,
    pub count: i64,
    /// Hay que volver a consultar (cambió la búsqueda o se guardó algo).
    pub dirty: bool,
    pub form: Option<NoteForm>,
}

#[derive(Default)]
pub struct NoteForm {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub tags: String,
}

/// Botones de la cabecera de un panel.
enum HeaderAction {
    Close,
    Maximize,
    Start(String),
    Stop(String),
    Restart(String),
}

pub struct Workspace {
    pub project: Project,
    pub processes: Processes,
    pub guard: GuardView,
    pub memory: MemoryView,
    pub ideas: IdeasView,
    pub git: GitView,
    panels: HashMap<PanelId, Panel>,
    layout: Node,
    focus: PanelId,
    maximized: Option<PanelId>,
    next_id: PanelId,
    renaming: Option<(PanelId, String)>,
    branch: (Option<String>, Option<Instant>),
    /// Último error (p. ej. no se pudo abrir un shell); la app lo muestra y lo limpia.
    pub error: Option<String>,
    /// Dormido: sin terminales ni procesos; guarda el estado para restaurarlo al despertar.
    dormant: Option<Option<WorkspaceState>>,
    /// Última vez que estuvo a la vista (para los avisos de actividad).
    seen: Instant,
    /// Procesos fallidos que el usuario ya vio.
    seen_failures: Vec<String>,
    /// Bytes que había escrito cada terminal la última vez que se vio el proyecto.
    seen_output: HashMap<PanelId, u64>,
    /// Bytes de cada terminal cuando se guardó su historial (para no reescribirlo igual).
    history_saved: HashMap<PanelId, u64>,
    /// Avisos ya notificados desde la última vez que se vio el proyecto (uno por aviso).
    pub notified: Vec<Attention>,
    /// Último aviso visto: se mantiene hasta entrar al proyecto (sin parpadeos si el
    /// agente redibuja su pantalla mientras espera).
    sticky: Option<Attention>,
}

impl Workspace {
    /// Abre el workspace: sesión guardada → layout predefinido → un panel en la raíz.
    /// Inicia los procesos `on-workspace-open` y los que estaban en marcha al cerrar.
    pub fn open(ctx: &egui::Context, project: Project, saved: Option<WorkspaceState>) -> Self {
        let mut ws = Self::new(project);
        ws.start(ctx, saved);
        ws
    }

    /// Workspace vacío (sin paneles ni procesos en marcha).
    fn new(project: Project) -> Self {
        let processes = Processes::new(
            &project.config.processes,
            project.root(),
            project.config.environment.clone(),
        );
        Self {
            project,
            processes,
            guard: GuardView::default(),
            memory: MemoryView {
                dirty: true,
                ..Default::default()
            },
            ideas: IdeasView {
                dirty: true,
                ..Default::default()
            },
            git: GitView {
                dirty: true,
                ..Default::default()
            },
            panels: HashMap::new(),
            layout: Node::Leaf(0),
            focus: 0,
            maximized: None,
            next_id: 0,
            renaming: None,
            branch: (None, None),
            error: None,
            dormant: None,
            seen: Instant::now(),
            seen_failures: Vec::new(),
            seen_output: HashMap::new(),
            history_saved: HashMap::new(),
            notified: Vec::new(),
            sticky: None,
        }
    }

    /// Proyecto abierto pero dormido: aparece en la lista y no lanza nada hasta despertarlo.
    pub fn asleep(project: Project, saved: Option<WorkspaceState>) -> Self {
        let mut ws = Self::new(project);
        ws.dormant = Some(saved);
        ws
    }

    pub fn is_dormant(&self) -> bool {
        self.dormant.is_some()
    }

    /// Despierta un proyecto dormido: restaura paneles y procesos como al abrirlo.
    pub fn wake(&mut self, ctx: &egui::Context) {
        if let Some(saved) = self.dormant.take() {
            self.next_id = 0;
            self.seen = Instant::now();
            self.start(ctx, saved);
        }
    }

    /// Cierra terminales y procesos (al soltarlos se matan) y conserva el layout.
    pub fn sleep(&mut self) {
        if self.is_dormant() {
            return;
        }
        let state = self.state();
        self.save_history();
        self.panels.clear();
        self.maximized = None;
        self.renaming = None;
        self.processes = Processes::new(
            &self.project.config.processes,
            self.project.root(),
            self.project.config.environment.clone(),
        );
        self.guard.run = None;
        self.dormant = Some(Some(state));
    }

    /// Archivo del historial de un panel: carpeta de Forge + proyecto + panel.
    fn history_file(&self, id: PanelId) -> Option<PathBuf> {
        // FNV-1a: nombre estable por proyecto sin dependencias.
        let hash = self
            .project
            .path
            .to_string_lossy()
            .bytes()
            .fold(0xcbf29ce484222325_u64, |h, b| {
                (h ^ b as u64).wrapping_mul(0x100000001b3)
            });
        HISTORY_DIR
            .get()
            .map(|d| d.join(format!("{hash:016x}-{id}.txt")))
    }

    /// Guarda el historial de las terminales que escribieron algo desde la última vez.
    pub fn save_history(&mut self) {
        let Some(dir) = HISTORY_DIR.get() else { return };
        let _ = std::fs::create_dir_all(dir);
        let ids: Vec<PanelId> = self.panels.keys().copied().collect();
        for id in ids {
            let Some(Panel {
                content: Content::Shell(t),
                ..
            }) = self.panels.get(&id)
            else {
                continue;
            };
            let bytes = t.output_bytes();
            if self.history_saved.get(&id) == Some(&bytes) {
                continue;
            }
            if let (Some(text), Some(file)) = (t.history_text(HISTORY_LINES), self.history_file(id))
            {
                let _ = std::fs::write(file, text);
            }
            self.history_saved.insert(id, bytes);
        }
    }

    /// Estado para guardar (el de antes de dormir si está dormido).
    pub fn saved_state(&self) -> Option<WorkspaceState> {
        match &self.dormant {
            Some(saved) => saved.clone(),
            None => Some(self.state()),
        }
    }

    /// El usuario lo está viendo: se apagan sus avisos.
    pub fn mark_seen(&mut self) {
        self.seen = Instant::now();
        self.guard.unseen = false;
        self.notified.clear();
        self.sticky = None;
        self.seen_failures = self.failed_processes();
        self.seen_output = self
            .panels
            .iter()
            .filter_map(|(id, p)| match &p.content {
                Content::Shell(t) => Some((*id, t.output_bytes())),
                Content::Process(_) => None,
            })
            .collect();
    }

    fn failed_processes(&self) -> Vec<String> {
        self.processes
            .list
            .iter()
            .filter(|m| {
                matches!(m.status, crate::processes::Status::Failed(_))
                    || matches!(m.status, crate::processes::Status::Exited { code } if code != 0)
            })
            .map(|m| m.def.id.clone())
            .collect()
    }

    /// Aviso más importante desde la última vez que se vio el proyecto (se mantiene hasta
    /// verlo).
    pub fn attention(&self) -> Option<Attention> {
        self.current_attention().or_else(|| self.sticky.clone())
    }

    /// Aviso enviado por el propio agente (hook): se mantiene hasta ver el proyecto.
    pub fn agent_event(&mut self, text: String) {
        self.sticky = Some(Attention::Event(text));
    }

    /// Recuerda el aviso actual (se llama cada fotograma).
    pub fn refresh_attention(&mut self) {
        if let Some(a) = self.current_attention() {
            self.sticky = Some(a);
        }
    }

    fn current_attention(&self) -> Option<Attention> {
        if self.is_dormant() {
            return None;
        }
        if let Some(id) = self
            .failed_processes()
            .into_iter()
            .find(|id| !self.seen_failures.contains(id))
        {
            let name = self
                .processes
                .get(&id)
                .map_or(id.clone(), |m| m.def.label().to_string());
            return Some(Attention::Failed(name));
        }
        let blocked = self.guard.unseen
            && self.guard.run.as_ref().is_some_and(|r| {
                r.snapshot(|r| r.verdict()) == forge_core::guard::Verdict::Blocked
            });
        if blocked {
            return Some(Attention::Blocked);
        }
        // Agente que trabajó mientras no se miraba (escribió una respuesta, no solo un
        // redibujo de su pantalla) y lleva unos segundos quieto.
        self.panels.iter().find_map(|(id, p)| {
            let agent = p.agent.as_ref()?;
            let spec = self.project.agents.iter().find(|s| &s.id == agent);
            if spec.is_some_and(|s| s.sends_events()) {
                return None; // avisa él mismo (hooks), sin adivinar
            }
            let Content::Shell(t) = &p.content else {
                return None;
            };
            let last = t.last_output()?;
            let written = t.output_bytes() - self.seen_output.get(id).copied().unwrap_or(0);
            let worked = written > AGENT_WORK_BYTES && last > self.seen;
            (worked && last.elapsed() > Duration::from_secs(4)).then(|| {
                let spec = self.project.agents.iter().find(|s| &s.id == agent);
                Attention::Agent(spec.map_or(agent.clone(), |s| s.name.clone()))
            })
        })
    }

    /// Restaura la sesión guardada (o el layout por defecto) y arranca los procesos.
    fn start(&mut self, ctx: &egui::Context, saved: Option<WorkspaceState>) {
        let ws = self;
        let config = &ws.project.config.workspace;
        let (restore_panels, restore_processes) = (config.restore_panels, config.restore_processes);
        let mut start: Vec<String> = ws
            .project
            .config
            .processes
            .iter()
            .filter(|p| p.restart == StartPolicy::OnWorkspaceOpen)
            .map(|p| p.id.clone())
            .collect();
        match saved.filter(|_| restore_panels) {
            Some(state) => {
                if restore_processes {
                    start.extend(state.running.iter().cloned());
                }
                ws.restore(ctx, state);
            }
            None => ws.reset_layout(ctx),
        }
        if restore_processes {
            for id in start {
                ws.processes.start(ctx, &id);
            }
        }
    }

    /// Aplica una configuración recargada del disco.
    pub fn set_project(&mut self, project: Project) {
        self.processes
            .set_env(project.root(), project.config.environment.clone());
        self.processes.sync(&project.config.processes);
        self.project = project;
    }

    fn insert(&mut self, id: PanelId, panel: Panel) {
        self.panels.insert(id, panel);
        self.next_id = self.next_id.max(id + 1);
    }

    fn spawn_panel(&mut self, ctx: &egui::Context, id: PanelId, state: PanelState) -> bool {
        if let Some(process) = state.process {
            if self.processes.get(&process).is_none() {
                return false; // el proceso ya no está definido
            }
            let panel = Panel {
                content: Content::Process(process),
                name: state.name,
                command: None,
                agent: None,
            };
            self.insert(id, panel);
            return true;
        }
        let cwd = if state.cwd.is_dir() {
            state.cwd
        } else {
            self.project.root()
        };
        // Un panel de agente lleva las variables del agente y guarda su comando de apertura.
        let spec = state
            .agent
            .as_ref()
            .and_then(|a| self.project.agents.iter().find(|s| &s.id == a))
            .cloned();
        let mut env = self.project.config.environment.clone();
        env.insert(
            "FORGE_PROJECT".into(),
            self.project.path.to_string_lossy().into_owned(),
        );
        if let Some(spec) = &spec {
            env.extend(spec.environment.clone());
            env.insert("FORGE_ORIGIN".into(), spec.name.clone());
        }
        // Agentes que lo permiten: se les conecta la memoria del proyecto por MCP.
        let typed = match (&spec, &state.command, self.mcp_server()) {
            (Some(spec), Some(command), Some(server)) => Some(spec.with_mcp(command, &server)),
            _ => state.command.clone(),
        };
        // Y que el agente avise él mismo al terminar o al necesitar al usuario.
        let forge = std::env::current_exe()
            .ok()
            .map(|e| e.canonicalize().unwrap_or(e));
        let typed = match (&spec, typed, forge) {
            (Some(spec), Some(command), Some(forge)) => {
                let has_mod = crate::claude_plugin::PLUGIN_DIR.get().is_some();
                Some(spec.with_events(&command, &forge.to_string_lossy(), has_mod))
            }
            (_, typed, _) => typed,
        };
        match Terminal::spawn(ctx, &cwd, typed.as_deref(), &env) {
            Ok(terminal) => {
                let command = spec.as_ref().map(|s| s.command.clone()).or(state.command);
                let panel = Panel {
                    content: Content::Shell(terminal),
                    name: state.name,
                    command,
                    agent: state.agent,
                };
                self.insert(id, panel);
                true
            }
            Err(e) => {
                self.error = Some(tr!(
                    "no se pudo abrir la terminal en {p0}: {e}",
                    p0 = cwd.display(),
                    e = e
                ));
                false
            }
        }
    }

    /// Servidor MCP de memoria para los agentes, si el proyecto la comparte.
    pub fn mcp_server(&self) -> Option<forge_core::agents::McpServer> {
        use forge_core::project::MemoryProvider;
        let memory = &self.project.config.memory;
        if !memory.enabled || !memory.share_with_agents {
            return None;
        }
        match memory.provider {
            MemoryProvider::Local => {
                let exe = std::env::current_exe().ok()?;
                let exe = exe.canonicalize().unwrap_or(exe);
                Some(forge_core::agents::McpServer {
                    name: "forge".into(),
                    command: exe.to_string_lossy().into_owned(),
                    args: vec![
                        "mcp".into(),
                        "--project".into(),
                        self.project.path.to_string_lossy().into_owned(),
                    ],
                })
            }
            // ponytail: el comando externo se separa por espacios (sin comillas).
            MemoryProvider::Mcp => {
                let mut words = memory
                    .command
                    .as_deref()?
                    .split_whitespace()
                    .map(String::from);
                Some(forge_core::agents::McpServer {
                    name: "memory".into(),
                    command: words.next()?,
                    args: words.collect(),
                })
            }
        }
    }

    /// Abre un agente en un panel nuevo en la raíz del proyecto (reanudando si se pide y se puede).
    pub fn open_agent(&mut self, ctx: &egui::Context, id: &str, resume: bool, area: Rect) {
        let Some(spec) = self.project.agents.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let command = match (&spec.resume, resume) {
            (Some(r), true) => r.clone(),
            _ => spec.command.clone(),
        };
        let state = PanelState {
            name: Some(spec.name.clone()),
            cwd: self.project.root(),
            command: Some(command),
            process: None,
            agent: Some(spec.id.clone()),
        };
        let dir = self.focused_rect(area).map_or(Dir::Row, layout::auto_dir);
        self.add_panel(ctx, dir, state);
    }

    fn root_panel(&self) -> PanelState {
        PanelState {
            name: None,
            cwd: self.project.root(),
            command: None,
            process: None,
            agent: None,
        }
    }

    /// Si no quedó ningún panel, abre uno en la raíz del proyecto.
    fn ensure_panel(&mut self, ctx: &egui::Context) {
        if self.panels.is_empty() {
            let id = self.next_id;
            self.layout = Node::Leaf(id);
            self.focus = id;
            let state = self.root_panel();
            self.spawn_panel(ctx, id, state);
        }
    }

    fn restore(&mut self, ctx: &egui::Context, mut state: WorkspaceState) {
        let run_commands = self.project.config.workspace.restore_processes;
        self.layout = state.layout;
        for id in self.layout.ids() {
            let mut panel = state
                .panels
                .remove(&id)
                .unwrap_or_else(|| self.root_panel());
            // Agentes: se reanuda su última sesión si lo permiten; si no, se abren de nuevo.
            let spec = panel
                .agent
                .as_ref()
                .and_then(|a| self.project.agents.iter().find(|s| &s.id == a));
            if let Some(spec) = spec {
                panel.command = Some(spec.resume.clone().unwrap_or_else(|| spec.command.clone()));
            }
            if !run_commands {
                panel.command = None;
            }
            if !self.spawn_panel(ctx, id, panel) {
                self.layout.remove(id);
                continue;
            }
            // Lo que mostraba la terminal antes de cerrar Forge.
            let saved = self
                .history_file(id)
                .and_then(|f| std::fs::read_to_string(f).ok());
            if let (
                Some(text),
                Some(Panel {
                    content: Content::Shell(t),
                    ..
                }),
            ) = (saved.filter(|t| !t.is_empty()), self.panels.get_mut(&id))
            {
                t.prefill(&text, tr!("sesión anterior"));
            }
        }
        self.focus = if self.panels.contains_key(&state.focus) {
            state.focus
        } else {
            self.layout.ids()[0]
        };
        self.ensure_panel(ctx);
    }

    /// Cierra todos los paneles y aplica el layout por defecto del proyecto.
    /// Los procesos administrados siguen en marcha.
    pub fn reset_layout(&mut self, ctx: &egui::Context) {
        self.panels.clear(); // Drop mata los shells.
        self.maximized = None;
        self.renaming = None;
        if let Some(spec) = self.project.default_layout().cloned()
            && let Some(node) = self.build(ctx, &spec)
        {
            self.layout = node;
            self.focus = self.layout.ids()[0];
        }
        self.ensure_panel(ctx);
    }

    fn build(&mut self, ctx: &egui::Context, spec: &LayoutSpec) -> Option<Node> {
        match spec {
            LayoutSpec::Panel {
                name,
                command,
                working_directory,
            } => {
                let id = self.next_id;
                let cwd = working_directory
                    .as_ref()
                    .map_or_else(|| self.project.root(), |d| self.project.root().join(d));
                let state = PanelState {
                    name: name.clone(),
                    cwd,
                    command: command.clone(),
                    process: None,
                    agent: None,
                };
                self.spawn_panel(ctx, id, state).then_some(Node::Leaf(id))
            }
            LayoutSpec::Split {
                split,
                ratio,
                first,
                second,
            } => {
                let dir = match split {
                    SplitSpec::Row => Dir::Row,
                    SplitSpec::Column => Dir::Column,
                };
                match (self.build(ctx, first), self.build(ctx, second)) {
                    (Some(a), Some(b)) => Some(Node::Split {
                        dir,
                        ratio: ratio.clamp(0.1, 0.9),
                        first: Box::new(a),
                        second: Box::new(b),
                    }),
                    (a, b) => a.or(b),
                }
            }
        }
    }

    /// Estado para guardar: layout, paneles, directorio actual de cada shell y procesos activos.
    pub fn state(&self) -> WorkspaceState {
        let panels = self
            .panels
            .iter()
            .map(|(id, p)| {
                let state = match &p.content {
                    Content::Shell(t) => PanelState {
                        name: p.name.clone(),
                        cwd: t.cwd().unwrap_or_else(|| self.project.root()),
                        command: p.command.clone(),
                        process: None,
                        agent: p.agent.clone(),
                    },
                    Content::Process(process) => PanelState {
                        name: p.name.clone(),
                        cwd: self.project.root(),
                        command: None,
                        process: Some(process.clone()),
                        agent: None,
                    },
                };
                (*id, state)
            })
            .collect();
        WorkspaceState {
            layout: self.layout.clone(),
            panels,
            focus: self.focus,
            running: self.processes.active_ids(),
        }
    }

    /// Procesos raíz del proyecto (shells de los paneles y procesos administrados).
    pub fn pids(&self) -> Vec<u32> {
        let shells = self.panels.values().filter_map(|p| match &p.content {
            Content::Shell(t) => t.pid(),
            Content::Process(_) => None,
        });
        let managed = self
            .processes
            .list
            .iter()
            .filter_map(|m| m.terminal.as_ref()?.pid());
        shells.chain(managed).collect()
    }

    /// Ninguna terminal escribió en `quiet` (tests: esperar a que arranquen los shells).
    #[cfg(test)]
    pub fn is_quiet(&self, quiet: Duration) -> bool {
        self.panels.values().all(|p| match &p.content {
            Content::Shell(t) => t.last_output().is_some_and(|at| at.elapsed() > quiet),
            Content::Process(_) => true,
        })
    }

    #[cfg(test)]
    pub fn panel_count(&self) -> usize {
        self.panels.len()
    }

    /// Rama Git (se relee cada 2 s).
    pub fn branch(&mut self) -> Option<String> {
        if self
            .branch
            .1
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.branch = (
                forge_core::project::git_branch(&self.project.path),
                Some(Instant::now()),
            );
        }
        self.branch.0.clone()
    }

    fn label(&self, id: PanelId) -> String {
        let Some(p) = self.panels.get(&id) else {
            return String::new();
        };
        match &p.content {
            Content::Process(process) => {
                let def = self
                    .processes
                    .get(process)
                    .map(|m| m.def.label().to_string());
                p.name.clone().or(def).unwrap_or_else(|| process.clone())
            }
            Content::Shell(t) => p
                .name
                .clone()
                .or_else(|| t.title.clone())
                .unwrap_or_else(|| t.cwd().map_or_else(|| "terminal".into(), |c| tilde(&c))),
        }
    }

    pub fn title(&self) -> String {
        format!("{} — {}", self.project.name(), self.label(self.focus))
    }

    fn focused_rect(&self, area: Rect) -> Option<Rect> {
        self.rects(area)
            .into_iter()
            .find(|(id, _)| *id == self.focus)
            .map(|(_, r)| r)
    }

    /// Abre un comando guardado en un panel nuevo junto al que tiene el foco.
    pub fn run_command(&mut self, ctx: &egui::Context, command: &SavedCommand, area: Rect) {
        let cwd = command
            .working_directory
            .as_ref()
            .map_or_else(|| self.project.root(), |d| self.project.root().join(d));
        let state = PanelState {
            name: Some(command.name.clone()),
            cwd,
            command: Some(command.command.clone()),
            process: None,
            agent: None,
        };
        let dir = self.focused_rect(area).map_or(Dir::Row, layout::auto_dir);
        self.add_panel(ctx, dir, state);
    }

    /// Muestra los logs del proceso: enfoca su panel si ya existe o abre uno nuevo.
    pub fn show_process(&mut self, ctx: &egui::Context, process: &str, area: Rect) {
        let existing = self
            .panels
            .iter()
            .find(|(_, p)| matches!(&p.content, Content::Process(id) if id == process));
        if let Some((id, _)) = existing {
            self.focus = *id;
            if self.maximized.is_some() {
                self.maximized = Some(*id);
            }
            return;
        }
        let state = PanelState {
            name: None,
            cwd: self.project.root(),
            command: None,
            process: Some(process.to_string()),
            agent: None,
        };
        let dir = self.focused_rect(area).map_or(Dir::Row, layout::auto_dir);
        self.add_panel(ctx, dir, state);
    }

    fn add_panel(&mut self, ctx: &egui::Context, dir: Dir, state: PanelState) {
        let id = self.next_id;
        if self.spawn_panel(ctx, id, state) {
            if self.panels.len() == 1 {
                self.layout = Node::Leaf(id);
            } else {
                self.layout.split(self.focus, dir, id);
            }
            self.focus = id;
            self.maximized = None;
        }
    }

    /// Terminal nueva en el directorio actual del shell con foco.
    fn split(&mut self, ctx: &egui::Context, dir: Dir) {
        let cwd = match self.panels.get(&self.focus).map(|p| &p.content) {
            Some(Content::Shell(t)) => t.cwd(),
            _ => None,
        };
        let cwd = cwd.unwrap_or_else(|| self.project.root());
        self.add_panel(
            ctx,
            dir,
            PanelState {
                name: None,
                cwd,
                command: None,
                process: None,
                agent: None,
            },
        );
    }

    fn close(&mut self, id: PanelId) {
        if let Some(file) = self.history_file(id) {
            let _ = std::fs::remove_file(file);
        }
        self.layout.remove(id);
        self.panels.remove(&id); // Drop mata el shell; un proceso administrado sigue en marcha.
        if self.maximized == Some(id) {
            self.maximized = None;
        }
        if self.renaming.as_ref().is_some_and(|(r, _)| *r == id) {
            self.renaming = None;
        }
        if self.focus == id {
            self.focus = self.layout.ids()[0];
        }
    }

    /// Sin paneles: la app cierra el workspace.
    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    fn rects(&self, area: Rect) -> Vec<(PanelId, Rect)> {
        let mut rects = Vec::new();
        match self.maximized {
            Some(id) => rects.push((id, area)),
            None if self.panels.is_empty() => {}
            None => self.layout.rects(area, &mut rects),
        }
        rects
    }

    pub fn run(&mut self, ctx: &egui::Context, action: WsAction, area: Rect) {
        let rects = self.rects(area);
        match action {
            WsAction::Split(dir) => self.split(ctx, dir),
            WsAction::NewTerminal => {
                let rect = rects
                    .iter()
                    .find(|(id, _)| *id == self.focus)
                    .map(|(_, r)| *r);
                self.split(ctx, rect.map_or(Dir::Row, layout::auto_dir));
            }
            WsAction::Close => self.close(self.focus),
            WsAction::Find => {
                let focus = self.focus;
                let terminal = match self.panels.get_mut(&focus).map(|p| &mut p.content) {
                    Some(Content::Shell(t)) => Some(t),
                    Some(Content::Process(id)) => {
                        let id = id.clone();
                        self.processes
                            .get_mut(&id)
                            .and_then(|m| m.terminal.as_mut())
                    }
                    None => None,
                };
                if let Some(t) = terminal {
                    t.open_search();
                }
            }
            WsAction::ToggleMaximize => {
                self.maximized = match self.maximized {
                    None if self.panels.len() > 1 => Some(self.focus),
                    _ => None,
                };
            }
            WsAction::Focus(toward) => {
                if let Some(id) = layout::neighbor(&rects, self.focus, toward) {
                    self.focus = id;
                }
            }
            WsAction::Cycle(step) => {
                let ids = self.layout.ids();
                let i = ids.iter().position(|id| *id == self.focus).unwrap_or(0) as isize;
                self.focus = ids[(i + step).rem_euclid(ids.len() as isize) as usize];
                if self.maximized.is_some() {
                    self.maximized = Some(self.focus);
                }
            }
        }
    }

    /// Procesa acciones y dibuja los paneles en `area`.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        m: &Metrics,
        actions: &[WsAction],
        active: bool,
    ) {
        let ctx = ui.ctx().clone();

        // Shells que terminaron (exit, Ctrl+D): se cierra su panel. Los logs de procesos se quedan.
        let exited: Vec<PanelId> = self
            .panels
            .iter()
            .filter(|(_, p)| matches!(&p.content, Content::Shell(t) if t.exited()))
            .map(|(id, _)| *id)
            .collect();
        for id in exited {
            self.close(id);
        }
        for action in actions {
            if !self.panels.is_empty() {
                self.run(&ctx, *action, area);
            }
        }

        let key = self.project.path.clone();
        let mut header_actions = Vec::new();
        for (id, rect) in self.rects(area) {
            let (header, body) = rect.split_top_bottom_at_y(rect.min.y + HEADER);
            header_actions.extend(self.header(ui, id, header).into_iter().map(|a| (id, a)));
            let focused = active && id == self.focus && self.renaming.is_none();
            let terminal_id = egui::Id::new(("terminal", key.as_path(), id));
            let Some(panel) = self.panels.get_mut(&id) else {
                continue;
            };
            let terminal = match &mut panel.content {
                Content::Shell(t) => Some(t),
                Content::Process(process) => {
                    let process = process.clone();
                    match self
                        .processes
                        .get_mut(&process)
                        .and_then(|m| m.terminal.as_mut())
                    {
                        Some(t) => Some(t),
                        None => {
                            let painter = ui.painter_at(body);
                            painter.rect_filled(
                                body,
                                0.0,
                                crate::terminal::color(alacritty_background()),
                            );
                            painter.text(
                                body.center(),
                                Align2::CENTER_CENTER,
                                tr!("Proceso detenido. Pulsa ▶ en la cabecera para iniciarlo."),
                                FontId::proportional(14.0),
                                Color32::from_gray(0x90),
                            );
                            let response = ui.interact(body, terminal_id, Sense::click());
                            if response.clicked() {
                                self.focus = id;
                            }
                            None
                        }
                    }
                }
            };
            if let Some(terminal) = terminal {
                // Margen interior: el texto no toca los bordes del panel.
                ui.painter().rect_filled(body, 0.0, crate::theme::BG);
                let inner = Rect::from_min_max(
                    body.min + Vec2::new(10.0, 6.0),
                    body.max - Vec2::new(6.0, 4.0),
                );
                let response = terminal.ui(ui, inner, terminal_id, focused, m);
                if response.is_pointer_button_down_on() {
                    self.focus = id;
                }
            }
        }
        if self.maximized.is_none() && !self.panels.is_empty() {
            self.layout
                .dividers(ui, area, egui::Id::new(("layout", key.as_path())));
        }
        for (id, action) in header_actions {
            match action {
                HeaderAction::Close => self.close(id),
                HeaderAction::Maximize => {
                    self.focus = id;
                    self.maximized = if self.maximized.is_some() {
                        None
                    } else {
                        Some(id)
                    };
                }
                HeaderAction::Start(p) => self.processes.start(&ctx, &p),
                HeaderAction::Stop(p) => self.processes.stop(&p),
                HeaderAction::Restart(p) => self.processes.restart(&ctx, &p),
            }
        }
    }

    /// Cabecera del panel (estilo pestaña de macOS): icono según el tipo, título, detalle
    /// en gris y botones. Doble clic en el título para renombrar.
    fn header(&mut self, ui: &mut egui::Ui, id: PanelId, rect: Rect) -> Vec<HeaderAction> {
        use crate::theme::{self, icon};
        let mut actions = Vec::new();
        let focused = id == self.focus;
        let branch = self.branch();
        let key = self.project.path.clone();
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            0.0,
            if focused {
                theme::SURFACE
            } else {
                Color32::from_rgb(0x21, 0x21, 0x23)
            },
        );
        painter.hline(
            rect.x_range(),
            rect.max.y - 0.5,
            egui::Stroke::new(1.0, theme::SEPARATOR),
        );

        // Botones de derecha a izquierda.
        let size = 24.0;
        let mut right = rect.max.x - 4.0;
        let mut button = |glyph: &str, hint: &str, salt: &str| {
            let r = Rect::from_center_size(
                egui::pos2(right - size / 2.0, rect.center().y),
                Vec2::splat(size),
            );
            right -= size + 2.0;
            let response = ui
                .interact(r, egui::Id::new((salt, &key, id)), Sense::click())
                .on_hover_text(hint);
            let color = if focused {
                theme::TEXT_3
            } else {
                theme::TEXT_4
            };
            theme::paint_icon_button(ui, r, glyph, &response, color);
            response.clicked()
        };
        if button(icon::X, tr!("Cerrar panel (⌘W)"), "close") {
            actions.push(HeaderAction::Close);
        }
        if self.panels.len() > 1 {
            let (glyph, hint) = if self.maximized == Some(id) {
                (icon::ARROWS_IN, "Restaurar (⌘Enter)")
            } else {
                (icon::ARROWS_OUT, "Maximizar (⌘Enter)")
            };
            if button(glyph, hint, "maximize") {
                actions.push(HeaderAction::Maximize);
            }
        }
        let panel = self.panels.get(&id);
        let process = match panel.map(|p| &p.content) {
            Some(Content::Process(p)) => self.processes.get(p),
            _ => None,
        };
        if let Some(m) = process {
            let pid = m.def.id.clone();
            if button(icon::ARROW_CLOCKWISE, tr!("Reiniciar"), "restart") {
                actions.push(HeaderAction::Restart(pid.clone()));
            }
            if m.is_active() {
                if button(icon::STOP, tr!("Detener (Ctrl+C)"), "stop") {
                    actions.push(HeaderAction::Stop(pid));
                }
            } else if button(icon::PLAY, tr!("Iniciar"), "start") {
                actions.push(HeaderAction::Start(pid));
            }
        }

        // Icono: punto de estado (proceso), robot (agente) o terminal.
        let icon_pos = egui::pos2(rect.min.x + 18.0, rect.center().y);
        let accent = if focused {
            theme::ACCENT
        } else {
            theme::TEXT_4
        };
        match (process, panel.and_then(|p| p.agent.as_ref())) {
            (Some(m), _) => {
                painter.circle_filled(icon_pos, 4.0, tone_color(m.describe().1));
            }
            (None, Some(agent))
                if self
                    .project
                    .agents
                    .iter()
                    .find(|s| &s.id == agent)
                    .is_some_and(|s| matches!(s.program(), "claude" | "codex" | "opencode")) =>
            {
                let program = self
                    .project
                    .agents
                    .iter()
                    .find(|s| &s.id == agent)
                    .map(|s| s.program().to_string())
                    .unwrap_or_default();
                theme::agent_logo(
                    ui,
                    &program,
                    Rect::from_center_size(icon_pos, Vec2::splat(16.0)),
                    !focused,
                );
            }
            (None, Some(_)) => {
                painter.text(
                    icon_pos,
                    Align2::CENTER_CENTER,
                    icon::ROBOT,
                    FontId::proportional(14.0),
                    if focused {
                        theme::ORANGE
                    } else {
                        theme::TEXT_4
                    },
                );
            }
            _ => {
                painter.text(
                    icon_pos,
                    Align2::CENTER_CENTER,
                    icon::TERMINAL_WINDOW,
                    FontId::proportional(14.0),
                    accent,
                );
            }
        };
        // Rama de Git a la derecha, junto a los botones (se omite si no cabe).
        if let Some(branch) = branch {
            let text = format!("{}  {branch}", icon::GIT_BRANCH);
            let galley = painter.layout_no_wrap(text, FontId::proportional(11.5), theme::TEXT_4);
            let max = (right - rect.min.x - 32.0) * 0.45;
            if max > 60.0 {
                let width = galley.size().x.min(max);
                let at = Rect::from_min_max(
                    egui::pos2(right - 8.0 - width, rect.min.y),
                    egui::pos2(right - 8.0, rect.max.y),
                );
                painter.with_clip_rect(at).galley(
                    egui::pos2(at.min.x, rect.center().y - galley.size().y / 2.0),
                    galley,
                    theme::TEXT_4,
                );
                ui.interact(at, egui::Id::new(("branch", &key, id)), Sense::hover())
                    .on_hover_text(&branch);
                right = at.min.x - 6.0;
            }
        }
        let label_rect = Rect::from_min_max(
            egui::pos2(rect.min.x + 32.0, rect.min.y),
            egui::pos2(right - 4.0, rect.max.y),
        );

        if let Some((_, buf)) = self.renaming.as_mut().filter(|(r, _)| *r == id) {
            let edit = ui.put(
                label_rect.shrink2(Vec2::new(0.0, 4.0)),
                egui::TextEdit::singleline(buf).font(FontId::proportional(13.0)),
            );
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            if edit.lost_focus() {
                let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
                let name = buf.trim().to_string();
                if !cancelled && let Some(panel) = self.panels.get_mut(&id) {
                    panel.name = (!name.is_empty()).then_some(name);
                }
                self.renaming = None;
            }
        } else {
            let title = self.label(id);
            // Detalle en gris: comando, estado del proceso, puerto/URL (solo texto).
            let mut detail = Vec::new();
            if let Some(command) = panel
                .and_then(|p| p.command.as_ref())
                .filter(|c| **c != title)
            {
                detail.push(command.clone());
            }
            if let Some(m) = process {
                detail.push(m.def.command.clone());
                detail.push(m.describe().0);
                if let Some(url) = m.urls().first() {
                    detail.push(url.clone());
                }
            }
            if self.maximized == Some(id) {
                detail.push(tr!("maximizado · {p0} paneles", p0 = self.panels.len()));
            }
            let mut job = egui::text::LayoutJob::default();
            let title_color = if focused { theme::TEXT } else { theme::TEXT_3 };
            job.append(
                &title,
                0.0,
                egui::TextFormat::simple(FontId::proportional(13.0), title_color),
            );
            if !detail.is_empty() {
                let text = format!("   {}", detail.join("  ·  "));
                job.append(
                    &text,
                    0.0,
                    egui::TextFormat::simple(FontId::proportional(12.0), theme::TEXT_4),
                );
            }
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            painter.with_clip_rect(label_rect).galley(
                egui::pos2(label_rect.min.x, rect.center().y - galley.size().y / 2.0),
                galley,
                theme::TEXT,
            );
            let response = ui.interact(
                label_rect,
                egui::Id::new(("header", &key, id)),
                Sense::click(),
            );
            if response.double_clicked() {
                self.renaming = Some((id, title));
            } else if response.clicked() {
                self.focus = id;
            }
        }
        actions
    }
}

fn alacritty_background() -> alacritty_terminal::vte::ansi::Color {
    alacritty_terminal::vte::ansi::Color::Named(
        alacritty_terminal::vte::ansi::NamedColor::Background,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::store::Store;

    fn wait_for(mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < deadline, "tiempo de espera agotado");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Criterio de las Fases 3 y 4: cerrar y reabrir recupera layout, nombres, comandos,
    /// carpetas, paneles de logs y procesos en marcha.
    /// El historial guardado de un panel reaparece al restaurar y se borra al cerrarlo.
    #[test]
    fn panel_history_is_restored_and_removed_on_close() {
        let base = std::env::temp_dir().join(format!("forge-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("proyecto/.forge")).unwrap();
        let dir = base.join("proyecto").canonicalize().unwrap();
        // Única prueba que fija la carpeta (OnceLock global del proceso de tests).
        let _ = HISTORY_DIR.set(base.join("scrollback"));
        let ctx = egui::Context::default();
        let ws = Workspace::open(&ctx, Project::load(&dir).unwrap(), None);
        let state = ws.state();
        let id = ws.layout.ids()[0];
        let file = ws.history_file(id).unwrap();
        drop(ws);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "npm run build\nlinea guardada").unwrap();

        let mut ws = Workspace::open(&ctx, Project::load(&dir).unwrap(), Some(state));
        let Some(Panel {
            content: Content::Shell(t),
            ..
        }) = ws.panels.get(&id)
        else {
            panic!("sin terminal");
        };
        let text = t.history_text(100).unwrap();
        assert!(
            text.contains("linea guardada") && text.contains("sesión anterior"),
            "{text}"
        );
        ws.close(id);
        assert!(!file.exists(), "cerrar el panel borra su historial");
    }

    #[test]
    fn layout_survives_restart() {
        let dir = std::env::temp_dir().join(format!("forge-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        std::fs::create_dir_all(dir.join("web")).unwrap();
        let dir = dir.canonicalize().unwrap();
        std::fs::write(
            dir.join(".forge/project.toml"),
            "[workspace]\ndefault_layout = \"dev\"\n\
             [layouts.dev]\nsplit = \"row\"\nratio = 0.3\n\
             first = { name = \"Agente\" }\n\
             second = { name = \"Web\", command = \"cd web\" }\n\
             [[processes]]\nid = \"sleeper\"\ncommand = \"sleep 60\"\n",
        )
        .unwrap();

        let ctx = egui::Context::default();
        let area = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let store = Store::in_memory().unwrap();
        let mut ws = Workspace::open(&ctx, Project::load(&dir).unwrap(), None);
        assert_eq!(ws.panel_count(), 2);
        ws.processes.start(&ctx, "sleeper");
        ws.show_process(&ctx, "sleeper", area);
        assert_eq!(ws.panel_count(), 3);
        wait_for(|| ws.state().panels.values().any(|p| p.cwd == dir.join("web")));
        let before = ws.state();
        assert_eq!(before.running, vec!["sleeper".to_string()]);
        store.touch(&dir, "test").unwrap();
        store.save_workspace(&dir, &before, 0).unwrap();
        drop(ws); // cierra la app: mata shells y procesos

        let saved = store.workspace(&dir).unwrap();
        let ws = Workspace::open(&ctx, Project::load(&dir).unwrap(), saved);
        let after = ws.state();
        assert_eq!(after.layout, before.layout);
        assert_eq!(after.focus, before.focus);
        assert_eq!(after.running, vec!["sleeper".to_string()]);
        let web = after
            .panels
            .values()
            .find(|p| p.name.as_deref() == Some("Web"))
            .unwrap();
        assert_eq!(web.cwd, dir.join("web"));
        assert_eq!(web.command.as_deref(), Some("cd web"));
        assert!(
            after
                .panels
                .values()
                .any(|p| p.name.as_deref() == Some("Agente") && p.cwd == dir)
        );
        assert!(
            after
                .panels
                .values()
                .any(|p| p.process.as_deref() == Some("sleeper"))
        );
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    /// Fase 8: un panel de agente guarda su identidad y al restaurar se reanuda la sesión.
    #[test]
    fn agent_panel_resumes_on_restore() {
        let dir = std::env::temp_dir().join(format!("forge-agent-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let dir = dir.canonicalize().unwrap();
        std::fs::write(
            dir.join(".forge/agents.toml"),
            "[[agents]]\nid = \"falso\"\nname = \"Agente falso\"\ncommand = \"true\"\nresume_command = \"cd sub\"\n",
        )
        .unwrap();
        let ctx = egui::Context::default();
        let area = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0));

        let mut ws = Workspace::open(&ctx, Project::load(&dir).unwrap(), None);
        ws.open_agent(&ctx, "falso", false, area);
        let state = ws.state();
        let agent = state
            .panels
            .values()
            .find(|p| p.agent.as_deref() == Some("falso"))
            .unwrap();
        assert_eq!(
            (agent.name.as_deref(), agent.command.as_deref()),
            (Some("Agente falso"), Some("true"))
        );
        assert_eq!(agent.cwd, dir);
        drop(ws);

        // Al reabrir se ejecuta el comando de reanudar (aquí `cd sub`) en vez del de abrir.
        let ws = Workspace::open(&ctx, Project::load(&dir).unwrap(), Some(state));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let restored = ws.state();
            let agent = restored
                .panels
                .values()
                .find(|p| p.agent.as_deref() == Some("falso"))
                .unwrap();
            assert_eq!(
                agent.command.as_deref(),
                Some("true"),
                "se guarda el comando de abrir"
            );
            if agent.cwd == dir.join("sub") {
                break;
            }
            assert!(Instant::now() < deadline, "no se reanudó: {:?}", agent.cwd);
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
