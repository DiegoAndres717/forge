// Tablero vivo de agentes en paralelo: qué hace cada agente (y cada subagente) ahora, en qué
// worktree y rama, qué archivos tocó, y dónde se pisan. Los eventos llegan por los hooks de
// cada agente (`forge agent-activity`, el hook previo de Codex/Gemini/agy) y se guardan en la
// base de Forge con caducidad: lo terminado queda un rato con su resultado y luego se borra.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;

/// Sin eventos en este tiempo, un agente se da por muerto (cerró la terminal sin avisar).
pub const RUNNING_TTL: i64 = 3600;
/// Lo terminado queda este tiempo con su resultado.
pub const FINISHED_TTL: i64 = 600;
/// Tope de archivos por fila: el registro no crece sin límite.
const MAX_FILES: usize = 200;

/// Un evento de un hook (JSON por stdin de `forge agent-activity`).
#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
pub struct Event {
    /// `start` (arranca un subagente), `tool` (herramienta en uso) o `stop` (terminó).
    pub kind: String,
    /// Subagente (vacío: la conversación principal).
    #[serde(default)]
    pub agent_id: String,
    /// «explorer: find the login», para los subagentes.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    /// El archivo o el comando de la herramienta.
    #[serde(default)]
    pub target: Option<String>,
    /// Primera línea de la respuesta al terminar.
    #[serde(default)]
    pub result: Option<String>,
    /// El agente («Claude Code»); si falta, `FORGE_ORIGIN`.
    #[serde(default)]
    pub origin: Option<String>,
}

/// Lo que se sabe de un agente o subagente en marcha (o recién terminado).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Activity {
    /// Terminal de Forge (`FORGE_SESSION_ID`).
    pub session: String,
    pub agent_id: String,
    pub agent: String,
    pub name: Option<String>,
    /// Raíz del worktree donde trabaja.
    pub root: Option<PathBuf>,
    pub tool: Option<String>,
    pub target: Option<String>,
    /// Archivos que editó, relativos a `root`.
    pub files: Vec<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    pub result: Option<String>,
}

/// Qué hace ahora, para mostrarlo en cualquier idioma.
#[derive(Debug, Clone, PartialEq)]
pub enum Doing {
    Editing(String),
    Running(String),
    Reading(String),
    Tool(String),
    Idle,
}

/// Herramientas que escriben archivos (Claude Code, Codex, Gemini CLI, Antigravity).
pub fn is_edit(tool: &str) -> bool {
    matches!(
        tool,
        "Edit"
            | "Write"
            | "MultiEdit"
            | "NotebookEdit"
            | "apply_patch"
            | "write_file"
            | "replace"
            | "write_to_file"
            | "replace_file_content"
            | "multi_replace_file_content"
    )
}

fn is_shell(tool: &str) -> bool {
    matches!(
        tool,
        "Bash"
            | "exec_command"
            | "shell"
            | "local_shell"
            | "run_shell_command"
            | "run_command"
            | "PowerShell"
    )
}

/// ¿Cuánto cuenta este agente por sus hooks? Por nombre (`FORGE_ORIGIN`, el de la terminal).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Live {
    /// Claude Code (mod de Forge): subagentes, herramientas y archivos editados.
    Full,
    /// Codex, Gemini CLI y Antigravity: el hook previo de Guard ve comandos y lecturas, no
    /// las ediciones; los archivos cambiados salen de `git status` de su worktree.
    Commands,
    /// Sin hook que Forge use (OpenCode, Qwen Code, otros).
    None,
}

pub fn live_data(agent: &str) -> Live {
    let name = agent.to_lowercase();
    if name.contains("claude") {
        Live::Full
    } else if ["codex", "gemini", "antigravity", "agy"]
        .iter()
        .any(|n| name.contains(n))
    {
        Live::Commands
    } else {
        Live::None
    }
}

impl Activity {
    pub fn doing(&self) -> Doing {
        if self.finished_at.is_some() {
            return Doing::Idle;
        }
        let (Some(tool), target) = (&self.tool, self.target.clone().unwrap_or_default()) else {
            return Doing::Idle;
        };
        let file = || {
            let rel = self
                .root
                .as_deref()
                .and_then(|r| Path::new(&target).strip_prefix(r).ok())
                .map(|p| p.to_string_lossy().replace('\\', "/"));
            rel.unwrap_or_else(|| target.clone())
        };
        if is_edit(tool) && !target.is_empty() {
            Doing::Editing(file())
        } else if is_shell(tool) && !target.is_empty() {
            Doing::Running(first_line(&target, 60))
        } else if matches!(
            tool.as_str(),
            "Read" | "Grep" | "Glob" | "read_file" | "view_file" | "grep_search"
        ) && !target.is_empty()
        {
            Doing::Reading(file())
        } else {
            Doing::Tool(tool.clone())
        }
    }

    /// «Claude Code», o «Claude Code ↳ explorer: find the login».
    pub fn label(&self) -> String {
        match (&self.name, self.agent_id.is_empty()) {
            (Some(name), false) => format!("{} ↳ {name}", self.agent),
            (None, false) => format!("{} ↳ subagent", self.agent),
            _ => self.agent.clone(),
        }
    }
}

/// La primera línea con texto, recortada a `max` caracteres.
pub fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max - 1).collect::<String>())
    } else {
        line.to_string()
    }
}

pub fn init(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_activity (
            project     TEXT NOT NULL,
            session     TEXT NOT NULL,
            agent_id    TEXT NOT NULL,
            agent       TEXT NOT NULL,
            name        TEXT,
            root        TEXT,
            tool        TEXT,
            target      TEXT,
            files       TEXT NOT NULL DEFAULT '[]',
            started_at  INTEGER NOT NULL,
            updated_at  INTEGER NOT NULL,
            finished_at INTEGER,
            result      TEXT,
            PRIMARY KEY (project, session, agent_id)
        );",
    )
}

/// Raíz del worktree que contiene `path` (un archivo o una carpeta, aunque aún no exista).
pub fn root_of(path: &Path) -> Option<PathBuf> {
    let mut dir = path;
    while !dir.is_dir() {
        dir = dir.parent()?;
    }
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !top.is_empty()).then(|| crate::canonical(Path::new(&top)))
}

/// `path` relativo a `root` con `/`; tal cual si no está dentro. El archivo puede no existir
/// aún (Write): se mira su carpeta real (en macOS /var es /private/var).
fn relative(root: Option<&Path>, path: &str) -> String {
    let path = Path::new(path);
    let real = path
        .parent()
        .zip(path.file_name())
        .map(|(dir, name)| crate::canonical(dir).join(name));
    root.and_then(|r| {
        [Some(path.to_path_buf()), real]
            .into_iter()
            .flatten()
            .find_map(|p| p.strip_prefix(r).ok().map(Path::to_path_buf))
    })
    .map(|p| p.to_string_lossy().replace('\\', "/"))
    .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"))
}

impl crate::store::Store {
    /// Borra lo terminado hace más de `FINISHED_TTL` y lo que lleva `RUNNING_TTL` sin eventos.
    pub fn prune_activity(&self, now: i64) {
        let _ = self.conn.execute(
            "DELETE FROM agent_activity WHERE (finished_at IS NOT NULL AND finished_at < ?1)
                OR updated_at < ?2",
            params![now - FINISHED_TTL, now - RUNNING_TTL],
        );
    }

    /// Guarda un evento de `session` (y su subagente, si lo hay). `root`: el worktree donde
    /// ocurre (para que los archivos queden relativos a él).
    pub fn record_activity(
        &self,
        project: &Path,
        session: &str,
        e: &Event,
        root: Option<&Path>,
        now: i64,
    ) -> Result<(), String> {
        self.prune_activity(now);
        let key = self.project_key(project);
        let before: Option<(String, Option<i64>, i64)> = self
            .conn
            .query_row(
                "SELECT files, finished_at, started_at FROM agent_activity
                 WHERE project = ?1 AND session = ?2 AND agent_id = ?3",
                params![key, session, e.agent_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let mut files: Vec<String> = before
            .as_ref()
            .and_then(|(f, _, _)| serde_json::from_str(f).ok())
            .unwrap_or_default();
        let tool = e.tool.as_deref().unwrap_or_default();
        if let Some(target) = e.target.as_deref().filter(|_| is_edit(tool)) {
            let rel = relative(root, target);
            if !files.contains(&rel) && files.len() < MAX_FILES {
                files.push(rel);
            }
        }
        // Un arranque, o volver a trabajar tras terminar, empieza la cuenta de nuevo.
        let restart = e.kind == "start" || before.as_ref().is_none_or(|(_, f, _)| f.is_some());
        let started = match &before {
            Some((_, _, s)) if !restart => *s,
            _ => now,
        };
        let finished = (e.kind == "stop").then_some(now);
        let agent = e
            .origin
            .clone()
            .or_else(|| std::env::var("FORGE_ORIGIN").ok())
            .unwrap_or_else(|| "agent".into());
        let files = serde_json::to_string(&files).unwrap_or_else(|_| "[]".into());
        self.conn
            .execute(
                "INSERT INTO agent_activity (project, session, agent_id, agent, name, root, tool, target,
                    files, started_at, updated_at, finished_at, result)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(project, session, agent_id) DO UPDATE SET
                    agent = excluded.agent,
                    name = COALESCE(excluded.name, name),
                    root = COALESCE(excluded.root, root),
                    tool = CASE WHEN ?14 = 'stop' THEN tool ELSE excluded.tool END,
                    target = CASE WHEN ?14 = 'stop' THEN target ELSE excluded.target END,
                    files = excluded.files,
                    started_at = excluded.started_at,
                    updated_at = excluded.updated_at,
                    finished_at = excluded.finished_at,
                    result = excluded.result",
                params![
                    key,
                    session,
                    e.agent_id,
                    agent,
                    e.name,
                    root.map(|r| r.to_string_lossy().into_owned()),
                    e.tool,
                    e.target.as_deref().map(|t| first_line(t, 200)),
                    files,
                    started,
                    now,
                    finished,
                    e.result.as_deref().map(|r| first_line(r, 140)),
                    e.kind,
                ],
            )
            .map(drop)
            .map_err(|e| e.to_string())
    }

    /// Los agentes del proyecto (todos sus worktrees): por terminal, la conversación principal
    /// y después sus subagentes, del que empezó antes al último.
    pub fn agent_activity(&self, project: &Path, now: i64) -> Vec<Activity> {
        self.prune_activity(now);
        let key = self.project_key(project);
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT session, agent_id, agent, name, root, tool, target, files, started_at,
                    updated_at, finished_at, result
             FROM agent_activity WHERE project = ?1
             ORDER BY session, agent_id <> '', started_at, agent_id",
        ) else {
            return Vec::new();
        };
        stmt.query_map([key], |r| {
            Ok(Activity {
                session: r.get(0)?,
                agent_id: r.get(1)?,
                agent: r.get(2)?,
                name: r.get(3)?,
                root: r.get::<_, Option<String>>(4)?.map(PathBuf::from),
                tool: r.get(5)?,
                target: r.get(6)?,
                files: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
                started_at: r.get(8)?,
                updated_at: r.get(9)?,
                finished_at: r.get(10)?,
                result: r.get(11)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }
}

/// `forge agent-activity`: guarda un evento (JSON por stdin). Nunca falla: un error aquí no
/// debe interrumpir al agente. Fuera de una terminal de Forge no hace nada.
pub fn record_main(project: &Path, payload: &str) {
    let (Some(session), Ok(event)) = (
        crate::store::current_session_id(),
        serde_json::from_str::<Event>(payload),
    ) else {
        return;
    };
    record_event(project, &session, &event, None);
}

/// Guarda `event` en la base de Forge, con el worktree de su archivo o de `cwd` (la carpeta
/// actual si no se sabe).
pub fn record_event(project: &Path, session: &str, event: &Event, cwd: Option<&Path>) {
    let target = event
        .target
        .as_deref()
        .filter(|t| event.tool.as_deref().is_some_and(is_edit) && Path::new(t).is_absolute());
    let cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    let root = target
        .and_then(|t| root_of(Path::new(t)))
        .or_else(|| cwd.and_then(|d| root_of(&d)));
    // La clave del proyecto es la del repositorio entero (un proyecto sin remoto va por ruta).
    let repo = crate::worktrees::main_root(project).unwrap_or_else(|_| project.to_path_buf());
    if let Some(store) =
        crate::store::Store::default_path().and_then(|p| crate::store::Store::open(&p).ok())
    {
        let _ = store.record_activity(&repo, session, event, root.as_deref(), crate::store::now());
    }
}

/// La herramienta y su archivo o comando en la llamada de un hook previo de otro agente
/// (Codex: `tool_name`/`tool_input`; Gemini: igual; agy: `toolCall.name`/`toolCall.args`).
pub fn tool_event(call: &serde_json::Value) -> Option<Event> {
    let tool = call["tool_name"]
        .as_str()
        .or_else(|| call["toolCall"]["name"].as_str())?;
    let args = call
        .get("tool_input")
        .or_else(|| call.pointer("/toolCall/args"));
    let pick = |keys: &[&str]| {
        keys.iter().find_map(|k| match args?.get(*k)? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Array(words) => Some(
                words
                    .iter()
                    .filter_map(|w| w.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
    };
    let target = pick(&["command", "CommandLine", "cmd"]).or_else(|| {
        pick(&[
            "file_path",
            "absolute_path",
            "AbsolutePath",
            "TargetFile",
            "path",
        ])
    });
    Some(Event {
        kind: "tool".into(),
        tool: Some(tool.to_string()),
        target,
        ..Default::default()
    })
}

/// Un worktree del repositorio con sus archivos cambiados (`git status`).
#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub changed: Vec<String>,
    /// «plan #194, phase 3 "Live board"» si su rama es la de una fase.
    pub phase: Option<String>,
}

/// Un archivo que dos o más tocan a la vez.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlap {
    pub file: String,
    /// Quiénes: agentes (su `label`) o «cambios sin commit en <worktree>».
    pub who: Vec<String>,
}

/// Quién reclama cada archivo: cada agente por lo que editó, y cada worktree por sus
/// cambios sin commit que ningún agente suyo explica. Solape: dos o más.
pub fn overlaps(agents: &[Activity], trees: &[Tree]) -> Vec<Overlap> {
    let mut claims: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for a in agents {
        for f in &a.files {
            claims.entry(f.clone()).or_default().insert(who(a));
        }
    }
    for t in trees {
        for f in &t.changed {
            let explained = agents
                .iter()
                .any(|a| a.root.as_deref() == Some(t.path.as_path()) && a.files.contains(f));
            if !explained {
                claims
                    .entry(f.clone())
                    .or_default()
                    .insert(format!("uncommitted changes in {}", tree_label(t)));
            }
        }
    }
    claims
        .into_iter()
        .filter(|(_, w)| w.len() > 1)
        .map(|(file, who)| Overlap {
            file,
            who: who.into_iter().collect(),
        })
        .collect()
}

/// Un agente, distinguible aunque haya dos iguales: «Codex (terminal 5)».
fn who(a: &Activity) -> String {
    let panel = a.session.rsplit(':').next().unwrap_or(&a.session);
    format!("{} (terminal {panel})", a.label())
}

fn tree_label(t: &Tree) -> String {
    t.branch
        .clone()
        .unwrap_or_else(|| t.path.display().to_string())
}

/// Los worktrees vivos del repositorio, con sus cambios y la fase de su rama.
pub fn trees(repo: &Path, plans: &[crate::ideas::Idea]) -> Vec<Tree> {
    crate::worktrees::list_basic(repo)
        .unwrap_or_default()
        .into_iter()
        .filter(|w| !w.prunable && w.path.is_dir())
        .map(|w| {
            let changed = crate::git::status(&w.path)
                .map(|s| {
                    s.files
                        .into_iter()
                        .map(|f| f.path)
                        .filter(|p| !p.ends_with('/'))
                        .collect()
                })
                .unwrap_or_default();
            let phase = w.branch.as_deref().and_then(|b| phase_of(plans, b));
            Tree {
                path: crate::canonical(&w.path),
                branch: w.branch,
                changed,
                phase,
            }
        })
        .collect()
}

fn phase_of(plans: &[crate::ideas::Idea], branch: &str) -> Option<String> {
    plans.iter().find_map(|idea| {
        let (n, phase) = idea
            .phases
            .iter()
            .enumerate()
            .find(|(_, p)| p.branch.as_deref() == Some(branch))?;
        Some(format!(
            "plan #{}, phase {} \"{}\"",
            idea.id,
            n + 1,
            phase.title
        ))
    })
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Lo que cambió en la rama principal (`origin/main` o `main`) desde que `root` se separó
/// de ella, entre `files`: (archivo, commit de la principal que lo cambió por última vez).
pub fn main_changes(root: &Path, files: &[String]) -> (String, Vec<(String, String)>) {
    let Some(base) = crate::git::main_branch(root) else {
        return (String::new(), Vec::new());
    };
    let (Some(tip), Some(fork)) = (
        git(root, &["rev-parse", &base]),
        git(root, &["merge-base", "HEAD", &base]),
    ) else {
        return (base, Vec::new());
    };
    if tip == fork || files.is_empty() {
        return (base, Vec::new());
    }
    let range = format!("{fork}..{tip}");
    let changed: BTreeSet<String> = git(root, &["diff", "--name-only", &fork, &tip])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let hits = files
        .iter()
        .filter(|f| changed.contains(*f))
        .filter_map(|f| {
            let commit = git(root, &["log", "-1", "--format=%H", &range, "--", f])?;
            Some((f.clone(), commit))
        })
        .collect();
    (base, hits)
}

/// Lo que este agente toca en `root`: sus cambios sin commit y lo que editó su sesión.
fn touching(
    store: &crate::store::Store,
    project: &Path,
    session: &str,
    root: &Path,
) -> Vec<String> {
    let mut files: BTreeSet<String> = crate::git::status(root)
        .map(|s| s.files.into_iter().map(|f| f.path).collect())
        .unwrap_or_default();
    let repo = crate::worktrees::main_root(project).unwrap_or_else(|_| project.to_path_buf());
    for a in store.agent_activity(&repo, crate::store::now()) {
        if a.session == session && a.root.as_deref() == Some(root) {
            files.extend(a.files);
        }
    }
    files.into_iter().filter(|f| !f.ends_with('/')).collect()
}

/// El aviso de un hook cuando la rama principal cambió archivos que este agente toca: una
/// sola vez por archivo y commit (lo ya avisado se guarda en la base). `None`: nada nuevo.
pub fn main_check(
    store: &crate::store::Store,
    project: &Path,
    session: &str,
    root: &Path,
) -> Option<String> {
    let files = touching(store, project, session, root);
    let (base, hits) = main_changes(root, &files);
    let key = format!("main-warned:{session}:{}", root.display());
    let mut warned: BTreeSet<String> = store
        .setting(&key)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    let new: Vec<&(String, String)> = hits
        .iter()
        .filter(|(f, c)| !warned.contains(&format!("{f}@{c}")))
        .collect();
    if new.is_empty() {
        return None;
    }
    warned.extend(new.iter().map(|(f, c)| format!("{f}@{c}")));
    let _ = store.set_setting(
        &key,
        &serde_json::to_string(&warned).unwrap_or_else(|_| "[]".into()),
    );
    let list: Vec<String> = new
        .iter()
        .map(|(f, c)| format!("{f} ({})", &c[..c.len().min(8)]))
        .collect();
    Some(format!(
        "Forge: {base} changed files you are touching since your branch left it: {}. Merge or rebase onto {base} before you go on, or you will be resolving conflicts later.",
        list.join(", ")
    ))
}

/// Pide `git fetch` en segundo plano si hace más de `every` segundos del último, para que
/// `origin/main` no se quede viejo. Solo con un remoto `origin`; nunca pregunta credenciales.
pub fn fetch_if_stale(root: &Path, every: i64) {
    let Some(git_dir) = git(root, &["rev-parse", "--git-common-dir"]) else {
        return;
    };
    if git(root, &["remote", "get-url", "origin"]).is_none() {
        return;
    }
    let git_dir = root.join(git_dir);
    let age = std::fs::metadata(git_dir.join("FETCH_HEAD"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map_or(i64::MAX, |d| d.as_secs() as i64);
    if age < every {
        return;
    }
    // Sin esperar: el hook no se queda colgado de la red.
    let _ = std::process::Command::new("git")
        .args(["fetch", "--quiet", "--no-tags", "origin"])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn duration(secs: i64) -> String {
    match secs {
        ..60 => format!("{}s", secs.max(0)),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

fn doing_en(a: &Activity) -> String {
    match a.doing() {
        Doing::Editing(f) => format!("editing {f}"),
        Doing::Running(c) => format!("running `{c}`"),
        Doing::Reading(f) => format!("reading {f}"),
        Doing::Tool(t) => format!("using {t}"),
        Doing::Idle => "idle".into(),
    }
}

/// Sesión de una terminal de agente abierta en Forge (`agent_sessions`).
#[derive(Debug, Clone)]
pub struct Terminal {
    pub session: String,
    pub agent: String,
}

/// El texto de `work_status` (para agentes, en inglés): worktrees con su rama, fase y
/// cambios; cada agente con lo que hace; los solapes; y si la principal cambió lo que toca
/// la sesión `me`.
pub fn work_status(
    store: &crate::store::Store,
    project: &Path,
    me: Option<&str>,
    now: i64,
) -> String {
    let repo = crate::worktrees::main_root(project).unwrap_or_else(|_| project.to_path_buf());
    let plans = store.list_ideas(Some(&repo), false).unwrap_or_default();
    let trees = trees(&repo, &plans);
    let agents = store.agent_activity(&repo, now);
    let terminals: Vec<Terminal> = store
        .active_agent_sessions(&repo)
        .unwrap_or_default()
        .into_iter()
        .map(|s| Terminal {
            session: s.id,
            agent: s.agent,
        })
        .collect();
    render(&trees, &agents, &terminals, me, now)
}

/// Cuántos archivos se listan por worktree o agente antes de «+N more».
const LIST: usize = 12;

fn files_line(files: &[String]) -> String {
    let mut s = files
        .iter()
        .take(LIST)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if files.len() > LIST {
        s.push_str(&format!(" (+{} more)", files.len() - LIST));
    }
    s
}

/// `work_status` a partir de lo ya leído (separado para probarlo sin Git).
pub fn render(
    trees: &[Tree],
    agents: &[Activity],
    terminals: &[Terminal],
    me: Option<&str>,
    now: i64,
) -> String {
    let mut out = vec!["Work status (live, from Forge hooks and git):".to_string()];
    out.push("Worktrees:".into());
    for t in trees {
        let mut line = format!("- {} · branch {}", t.path.display(), tree_label(t));
        if let Some(p) = &t.phase {
            line.push_str(&format!(" · {p}"));
        }
        match t.changed.len() {
            0 => line.push_str(" · no uncommitted changes"),
            n => line.push_str(&format!(" · {n} changed: {}", files_line(&t.changed))),
        }
        out.push(line);
    }
    out.push("Agents:".into());
    let tree_of = |a: &Activity| {
        a.root
            .as_deref()
            .and_then(|r| trees.iter().find(|t| t.path == r))
    };
    for a in agents {
        let mine = me == Some(a.session.as_str());
        let mut line = format!("- {}{}", who(a), if mine { " [you]" } else { "" });
        match a.finished_at {
            Some(at) => {
                line.push_str(&format!(" · finished {} ago", duration(now - at)));
                if let Some(r) = &a.result {
                    line.push_str(&format!(": {r}"));
                }
            }
            None => line.push_str(&format!(
                " · {} · for {}",
                doing_en(a),
                duration(now - a.started_at)
            )),
        }
        if let Some(t) = tree_of(a) {
            line.push_str(&format!(" · branch {}", tree_label(t)));
            if let Some(p) = &t.phase {
                line.push_str(&format!(" · {p}"));
            }
        } else if let Some(r) = &a.root {
            line.push_str(&format!(" · in {}", r.display()));
        }
        if !a.files.is_empty() {
            line.push_str(&format!(" · edited: {}", files_line(&a.files)));
        }
        if a.agent_id.is_empty() && live_data(&a.agent) == Live::Commands {
            line.push_str(" · live data: commands and reads only (its hooks do not report edits; see its worktree's changes)");
        }
        out.push(line);
    }
    for t in terminals {
        if agents.iter().any(|a| a.session == t.session) {
            continue;
        }
        let panel = t.session.rsplit(':').next().unwrap_or(&t.session);
        let why = match live_data(&t.agent) {
            Live::None => {
                "no live data for this agent (it has no hook Forge can use); its edits show up only as its worktree's uncommitted changes"
            }
            _ => "no activity reported yet",
        };
        out.push(format!("- {} (terminal {panel}) · {why}", t.agent));
    }
    if agents.is_empty() && terminals.is_empty() {
        out.push("- none running".into());
    }
    let found = overlaps(agents, trees);
    if found.is_empty() {
        out.push("Overlaps: none".into());
    } else {
        out.push("Overlaps (the same file touched by more than one at a time; coordinate before editing it):".into());
        for o in found {
            out.push(format!("- {}: {}", o.file, o.who.join(" and ")));
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn ev(kind: &str, agent_id: &str, tool: Option<&str>, target: Option<&str>) -> Event {
        Event {
            kind: kind.into(),
            agent_id: agent_id.into(),
            tool: tool.map(str::to_string),
            target: target.map(str::to_string),
            origin: Some("Claude Code".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_subagent_shows_up_with_its_task_and_expires_after_finishing() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/no/such/project");
        let start = Event {
            name: Some("explorer: find the login".into()),
            ..ev("start", "a1", None, None)
        };
        store.record_activity(p, "1:2", &start, None, 1000).unwrap();
        let tool = ev("tool", "a1", Some("Bash"), Some("cargo test -p x"));
        store.record_activity(p, "1:2", &tool, None, 1010).unwrap();
        let list = store.agent_activity(p, 1020);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label(), "Claude Code ↳ explorer: find the login");
        assert_eq!(list[0].doing(), Doing::Running("cargo test -p x".into()));
        assert_eq!(list[0].started_at, 1000);

        let stop = Event {
            result: Some("Found it in src/login.rs\nmore".into()),
            ..ev("stop", "a1", None, None)
        };
        store.record_activity(p, "1:2", &stop, None, 1100).unwrap();
        let list = store.agent_activity(p, 1200);
        assert_eq!(list[0].finished_at, Some(1100));
        assert_eq!(list[0].result.as_deref(), Some("Found it in src/login.rs"));
        // Un rato después de terminar, ya no está.
        assert!(store.agent_activity(p, 1100 + FINISHED_TTL + 1).is_empty());
    }

    #[test]
    fn a_running_agent_without_events_expires_and_files_are_capped() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/no/such/project");
        let root = Path::new("/r");
        for i in 0..(MAX_FILES + 20) {
            let file = format!("/r/src/f{i}.rs");
            let e = ev("tool", "", Some("Edit"), Some(&file));
            store
                .record_activity(p, "1:2", &e, Some(root), 1000)
                .unwrap();
        }
        let list = store.agent_activity(p, 1000);
        assert_eq!(list[0].files.len(), MAX_FILES);
        assert_eq!(list[0].files[0], "src/f0.rs");
        assert!(store.agent_activity(p, 1000 + RUNNING_TTL + 1).is_empty());
    }

    #[test]
    fn two_agents_editing_the_same_file_overlap() {
        let a = Activity {
            session: "1:2".into(),
            agent: "Claude Code".into(),
            root: Some("/r/a".into()),
            files: vec!["src/x.rs".into()],
            ..Default::default()
        };
        let b = Activity {
            session: "1:3".into(),
            agent: "Claude Code".into(),
            root: Some("/r/b".into()),
            files: vec!["src/x.rs".into(), "src/y.rs".into()],
            ..Default::default()
        };
        // El worktree de `a` con x.rs cambiado no es un tercero: lo explica `a`.
        let trees = [Tree {
            path: "/r/a".into(),
            changed: vec!["src/x.rs".into()],
            ..Default::default()
        }];
        let found = overlaps(&[a.clone(), b.clone()], &trees);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].file, "src/x.rs");
        assert_eq!(found[0].who.len(), 2);
        // Solo uno: sin solape.
        assert!(overlaps(&[b], &[]).is_empty());
    }

    #[test]
    fn an_agent_without_hooks_is_shown_honestly() {
        let text = render(
            &[],
            &[],
            &[Terminal {
                session: "1:7".into(),
                agent: "OpenCode".into(),
            }],
            None,
            0,
        );
        assert!(text.contains("OpenCode (terminal 7)"), "{text}");
        assert!(text.contains("no live data for this agent"), "{text}");
        assert_eq!(live_data("Claude Code"), Live::Full);
        assert_eq!(live_data("Codex"), Live::Commands);
        assert_eq!(live_data("Qwen Code"), Live::None);
    }

    /// Repositorio Git real en una carpeta temporal (la prueba la borra al final).
    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-act-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        sh(
            &dir,
            "git init -q -b main && git config user.email t@t && git config user.name t \
             && git config core.autocrlf false && mkdir src && echo v0 > src/x.rs \
             && echo v0 > src/y.rs && git add . && git commit -qm inicio",
        );
        crate::canonical(&dir)
    }

    fn sh(dir: &Path, c: &str) {
        let ok = std::process::Command::new("sh")
            .args(["-c", c])
            .current_dir(dir)
            .env(crate::worktrees::ALLOW, "1")
            .status()
            .unwrap()
            .success();
        assert!(ok, "{c}");
    }

    #[test]
    fn two_terminals_editing_the_same_file_in_two_worktrees_overlap_in_work_status() {
        let dir = repo("overlap");
        let wts = dir.join("wts");
        let b = crate::worktrees::create(&dir, "feat/b", None, &wts, Default::default()).unwrap();
        let b = crate::canonical(&b);
        let store = Store::in_memory().unwrap();
        let edit = |session: &str, root: &Path, file: &str| {
            std::fs::write(root.join(file), "mine\n").unwrap();
            let target = root.join(file).to_string_lossy().into_owned();
            let e = ev("tool", "", Some("Edit"), Some(&target));
            store
                .record_activity(&dir, session, &e, Some(root), crate::store::now())
                .unwrap();
        };
        edit("1:2", &dir, "src/x.rs");
        edit("1:3", &b, "src/x.rs");
        edit("1:3", &b, "src/y.rs");
        let text = work_status(&store, &dir, Some("1:2"), crate::store::now());
        assert!(text.contains("Overlaps (the same file"), "{text}");
        assert!(
            text.contains("- src/x.rs: Claude Code (terminal 2) and Claude Code (terminal 3)"),
            "{text}"
        );
        assert!(
            !text.contains("- src/y.rs:"),
            "y.rs lo toca uno solo: {text}"
        );
        assert!(
            text.contains("[you]") && text.contains("branch feat/b"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_changing_a_file_this_agent_touches_warns_once_per_change() {
        let dir = repo("main-moved");
        sh(&dir, "git switch -qc feat/a && echo a > src/x.rs");
        let store = Store::in_memory().unwrap();
        // La principal aún no se movió: nada.
        assert_eq!(main_check(&store, &dir, "1:2", &dir), None);
        sh(
            &dir,
            "git stash -q && git switch -q main && echo m > src/x.rs && echo m > src/z.rs \
             && git commit -qam 'main x' && git add . && git commit -qm z -q; \
             git switch -q feat/a && git stash pop -q",
        );
        let first = main_check(&store, &dir, "1:2", &dir).expect("avisa");
        assert!(
            first.contains("src/x.rs") && first.contains("main"),
            "{first}"
        );
        assert!(!first.contains("src/z.rs"), "z.rs no lo toca: {first}");
        // Una sola vez por cambio.
        assert_eq!(main_check(&store, &dir, "1:2", &dir), None);
        // Otro cambio de la principal en el mismo archivo: otro aviso.
        sh(
            &dir,
            "git stash -q && git switch -q main && echo m2 > src/x.rs && git commit -qam 'main x2' \
             && git switch -q feat/a && git stash pop -q",
        );
        assert!(main_check(&store, &dir, "1:2", &dir).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hook_calls_of_other_agents_become_tool_events() {
        let codex =
            serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "cargo build"}});
        let e = tool_event(&codex).unwrap();
        assert_eq!(
            (e.tool.as_deref(), e.target.as_deref()),
            (Some("Bash"), Some("cargo build"))
        );
        let agy = serde_json::json!({"toolCall": {"name": "view_file", "args": {"AbsolutePath": "/r/a.rs"}}});
        let e = tool_event(&agy).unwrap();
        assert_eq!(e.target.as_deref(), Some("/r/a.rs"));
        assert!(tool_event(&serde_json::json!({})).is_none());
    }
}
