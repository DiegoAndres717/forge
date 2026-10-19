// Persistencia local (SQLite): proyectos recientes, workspaces abiertos y su layout.
use crate::tr;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::evidence::{Evidence, Report};
use crate::guard::{Exception, Scope, Stage};
use crate::router::Usage;

pub struct ProjectRow {
    pub path: PathBuf,
    pub name: String,
    pub last_opened: i64,
}

/// Sesión activa de un agente en un proyecto y rama (para detectar colisiones).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActiveAgentSession {
    pub id: String,
    pub project_key: String,
    pub branch: String,
    pub agent: String,
    pub pid: Option<u32>,
    pub updated_at: i64,
}

pub struct Store {
    pub conn: Connection,
    /// Claves de proyecto ya resueltas (ver `project_key`).
    keys: Mutex<HashMap<PathBuf, String>>,
    /// Carpeta de las cuentas de agentes (junto a la base; `None` en memoria).
    pub accounts_root: Option<PathBuf>,
    /// $HOME, donde están las carpetas principales de los agentes.
    pub home: Option<PathBuf>,
}

type Result<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    tr!("base de datos: {e}", e = e)
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// macOS: `~/Library/Application Support/Forge`. Linux: `$XDG_DATA_HOME/forge`
/// (o `~/.local/share/forge`). Windows: `%LOCALAPPDATA%\\Forge`, no la carpeta Roaming:
/// el historial de las terminales no debe copiarse al servidor en cada inicio de sesión.
fn db_path(
    home: &Path,
    xdg_data_home: Option<std::ffi::OsString>,
    local_appdata: Option<std::ffi::OsString>,
    os: &str,
) -> PathBuf {
    if os == "macos" {
        return home.join("Library/Application Support/Forge/forge.db");
    }
    if os == "windows" {
        return local_appdata
            .filter(|d| !d.is_empty())
            .map_or_else(|| home.join("AppData").join("Local"), PathBuf::from)
            .join("Forge")
            .join("forge.db");
    }
    xdg_data_home
        .filter(|d| !d.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from)
        .join("forge/forge.db")
}

/// Claves `\\?\…` que guardaba Forge 0.7.1 en Windows → la ruta normal.
fn strip_verbatim_keys(conn: &Connection) {
    // Igual que `crate::strip_verbatim`, en SQL; OR IGNORE si ya existe la clave normal.
    let plain = |col: &str| {
        format!(
            r"CASE WHEN {col} LIKE '\\?\UNC\%' THEN '\\' || substr({col}, 9)
             WHEN {col} LIKE '\\?\%' THEN substr({col}, 5) ELSE {col} END"
        )
    };
    let columns = [
        ("projects", "path"),
        ("exceptions", "project"),
        ("evidence", "project"),
        ("usage", "project"),
        ("validations", "project"),
        ("agent_usage", "project"),
        ("ideas", "project"),
        ("memories", "project"),
        ("agent_sessions", "project_key"),
    ];
    for (table, col) in columns {
        let _ = conn.execute(
            &format!(
                r"UPDATE OR IGNORE {table} SET {col} = {} WHERE {col} LIKE '\\?\%'",
                plain(col)
            ),
            [],
        );
    }
    let _ = conn.execute(
        &format!(
            r"UPDATE settings SET value = {} WHERE key = 'active' AND value LIKE '\\?\%'",
            plain("value")
        ),
        [],
    );
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Store {
    /// `~/Library/Application Support/Forge/forge.db` (en Linux, ver `db_path`).
    /// `FORGE_DB` permite otra base (tests, varias instalaciones).
    pub fn default_path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("FORGE_DB") {
            return Some(PathBuf::from(path));
        }
        let home = crate::home_dir()?;
        Some(db_path(
            &home,
            std::env::var_os("XDG_DATA_HOME"),
            std::env::var_os("LOCALAPPDATA"),
            std::env::consts::OS,
        ))
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        let mut store = Self::init(Connection::open(path).map_err(err)?)?;
        strip_verbatim_keys(&store.conn);
        store.accounts_root = path.parent().map(|d| d.join("accounts"));
        Ok(store)
    }

    /// Base en memoria (tests).
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(err)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS projects (
                path        TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                last_opened INTEGER NOT NULL,
                is_open     INTEGER NOT NULL DEFAULT 0,
                position    INTEGER NOT NULL DEFAULT 0,
                workspace   TEXT
            );
            CREATE TABLE IF NOT EXISTS identities (
                path TEXT PRIMARY KEY,
                key  TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS exceptions (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                project    TEXT NOT NULL,
                rule       TEXT NOT NULL,
                reason     TEXT NOT NULL,
                scope      TEXT NOT NULL,
                stage      TEXT NOT NULL,
                candidate  TEXT NOT NULL,
                branch     TEXT NOT NULL,
                user       TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                revoked    INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS evidence (
                project     TEXT NOT NULL,
                tree        TEXT NOT NULL,
                check_id    TEXT NOT NULL,
                command     TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                output      TEXT NOT NULL,
                created_at  INTEGER NOT NULL,
                PRIMARY KEY (project, tree, check_id, command)
            );
            CREATE TABLE IF NOT EXISTS usage (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                project       TEXT NOT NULL,
                task          TEXT NOT NULL,
                provider      TEXT NOT NULL,
                model         TEXT NOT NULL,
                input_tokens  INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cost_usd      REAL NOT NULL,
                local         INTEGER NOT NULL,
                created_at    INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS validations (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                project    TEXT NOT NULL,
                stage      TEXT NOT NULL,
                candidate  TEXT NOT NULL,
                verdict    TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                report     TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS agent_sessions (
                id          TEXT PRIMARY KEY,
                project_key TEXT NOT NULL,
                branch      TEXT NOT NULL,
                agent       TEXT NOT NULL,
                pid         INTEGER,
                updated_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_agent_sessions_branch ON agent_sessions (project_key, branch);",
        )
        .map_err(err)?;
        crate::memory::init(&conn).map_err(err)?;
        crate::ideas::init(&conn).map_err(err)?;
        crate::events::init(&conn).map_err(err)?;
        Ok(Self {
            conn,
            keys: Mutex::default(),
            accounts_root: None,
            home: crate::home_dir(),
        })
    }

    /// Clave de un proyecto para su memoria y sus planes: el remoto de Git (y la subcarpeta),
    /// así siguen ahí al mover la carpeta, en otro clon o en un worktree. Sin remoto, la ruta.
    /// La primera vez pasa a la clave nueva lo que se guardó con la ruta.
    pub fn project_key(&self, project: &Path) -> String {
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(key) = keys.get(project) {
            return key.clone();
        }
        let path = path_key(project);
        let key = crate::git::remote_identity(project)
            .map_or_else(|| path.clone(), |id| format!("git:{id}"));
        // Lo guardado con la ruta, o con el remoto que tenía esta carpeta (repo renombrado o
        // movido), pasa a la clave de ahora. Del remoto viejo, solo si la clave nueva también
        // es un remoto (git puede fallar un momento) y ninguna otra carpeta lo sigue usando
        // (otro clon, o el original de un fork).
        let before: Option<String> = self
            .conn
            .query_row("SELECT key FROM identities WHERE path = ?1", [&path], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
            .filter(|old| {
                key.starts_with("git:")
                    && !self
                        .conn
                        .query_row(
                            "SELECT 1 FROM identities WHERE key = ?1 AND path != ?2",
                            params![old, path],
                            |_| Ok(()),
                        )
                        .optional()
                        .is_ok_and(|other| other.is_some())
            });
        for old in [Some(path.clone()), before].into_iter().flatten() {
            if old != key {
                for table in ["memories", "ideas"] {
                    let _ = self.conn.execute(
                        &format!("UPDATE {table} SET project = ?1 WHERE project = ?2"),
                        params![key, old],
                    );
                }
                let _ = self.conn.execute(
                    "UPDATE agent_sessions SET project_key = ?1 WHERE project_key = ?2",
                    params![key, old],
                );
            }
        }
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO identities (path, key) VALUES (?1, ?2)",
            params![path, key],
        );
        keys.insert(project.to_path_buf(), key.clone());
        key
    }

    /// Registra la apertura de un proyecto (lo crea si es nuevo).
    pub fn touch(&self, path: &Path, name: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO projects (path, name, last_opened, is_open) VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(path) DO UPDATE SET name = ?2, last_opened = ?3, is_open = 1",
                params![path_key(path), name, now()],
            )
            .map(drop)
            .map_err(err)
    }

    /// Guarda el estado restaurable del workspace (lo define la app) como JSON.
    pub fn save_workspace<T: Serialize>(
        &self,
        path: &Path,
        state: &T,
        position: usize,
    ) -> Result<()> {
        let json = serde_json::to_string(state).map_err(err)?;
        self.conn
            .execute(
                "UPDATE projects SET workspace = ?2, position = ?3, is_open = 1 WHERE path = ?1",
                params![path_key(path), json, position as i64],
            )
            .map(drop)
            .map_err(err)
    }

    /// Orden de la barra lateral: cada proyecto en la posición de su índice.
    pub fn reorder_projects(&self, paths: &[PathBuf]) -> Result<()> {
        for (i, path) in paths.iter().enumerate() {
            self.conn
                .execute(
                    "UPDATE projects SET position = ?2 WHERE path = ?1",
                    params![path_key(path), i as i64],
                )
                .map_err(err)?;
        }
        Ok(())
    }

    /// Marca el proyecto como cerrado; conserva su último layout para la próxima vez.
    pub fn set_closed(&self, path: &Path) -> Result<()> {
        self.conn
            .execute(
                "UPDATE projects SET is_open = 0 WHERE path = ?1",
                params![path_key(path)],
            )
            .map(drop)
            .map_err(err)
    }

    pub fn remove(&self, path: &Path) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM projects WHERE path = ?1",
                params![path_key(path)],
            )
            .map(drop)
            .map_err(err)
    }

    pub fn workspace<T: serde::de::DeserializeOwned>(&self, path: &Path) -> Result<Option<T>> {
        let json: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT workspace FROM projects WHERE path = ?1",
                params![path_key(path)],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        // Un JSON de una versión anterior que ya no encaja se ignora: se abre el layout por defecto.
        Ok(json.flatten().and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// Proyectos recientes, del más reciente al más antiguo.
    pub fn recent(&self) -> Result<Vec<ProjectRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, name, last_opened FROM projects ORDER BY last_opened DESC")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ProjectRow {
                    path: PathBuf::from(r.get::<_, String>(0)?),
                    name: r.get(1)?,
                    last_opened: r.get(2)?,
                })
            })
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)
    }

    /// Proyectos que estaban abiertos al cerrar la app, en su orden.
    pub fn open_projects(&self) -> Result<Vec<PathBuf>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM projects WHERE is_open = 1 ORDER BY position")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0).map(PathBuf::from))
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = ?2",
                params![key, value],
            )
            .map(drop)
            .map_err(err)
    }
}

impl Store {
    pub fn add_exception(&self, project: &Path, e: &Exception) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO exceptions (project, rule, reason, scope, stage, candidate, branch, user, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    path_key(project),
                    e.rule,
                    e.reason,
                    e.scope.as_str(),
                    e.stage.as_str(),
                    e.candidate,
                    e.branch,
                    e.user,
                    e.created_at
                ],
            )
            .map_err(err)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Historial de excepciones del proyecto (incluidas las revocadas), la más reciente primero.
    pub fn exceptions(&self, project: &Path) -> Result<Vec<Exception>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, rule, reason, scope, stage, candidate, branch, user, created_at, revoked
                 FROM exceptions WHERE project = ?1 ORDER BY id DESC",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![path_key(project)], |r| {
                Ok(Exception {
                    id: r.get(0)?,
                    rule: r.get(1)?,
                    reason: r.get(2)?,
                    scope: Scope::parse(&r.get::<_, String>(3)?).unwrap_or(Scope::Commit),
                    stage: Stage::parse(&r.get::<_, String>(4)?).unwrap_or(Stage::Commit),
                    candidate: r.get(5)?,
                    branch: r.get(6)?,
                    user: r.get(7)?,
                    created_at: r.get(8)?,
                    revoked: r.get(9)?,
                })
            })
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)
    }

    /// Desactiva una excepción (queda en el historial). Devuelve si existía.
    pub fn revoke_exception(&self, project: &Path, id: i64) -> Result<bool> {
        self.conn
            .execute(
                "UPDATE exceptions SET revoked = 1 WHERE id = ?1 AND project = ?2",
                params![id, path_key(project)],
            )
            .map(|n| n > 0)
            .map_err(err)
    }
}

/// Las evidencias caducan a los 14 días (los árboles viejos ya no se van a commitear).
const EVIDENCE_TTL: i64 = 14 * 86_400;

impl Store {
    pub fn save_evidence(&self, project: &Path, evidence: &[Evidence]) -> Result<()> {
        for e in evidence {
            self.conn
                .execute(
                    "INSERT OR REPLACE INTO evidence (project, tree, check_id, command, duration_ms, output, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![path_key(project), e.tree, e.check_id, e.command, e.duration_ms as i64, e.output, e.created_at],
                )
                .map_err(err)?;
        }
        self.conn
            .execute(
                "DELETE FROM evidence WHERE created_at < ?1",
                params![now() - EVIDENCE_TTL],
            )
            .map(drop)
            .map_err(err)
    }

    pub fn evidence(&self, project: &Path) -> Result<Vec<Evidence>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT check_id, command, tree, duration_ms, output, created_at FROM evidence
                 WHERE project = ?1 AND created_at >= ?2",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![path_key(project), now() - EVIDENCE_TTL], |r| {
                Ok(Evidence {
                    check_id: r.get(0)?,
                    command: r.get(1)?,
                    tree: r.get(2)?,
                    duration_ms: r.get::<_, i64>(3)? as u64,
                    output: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)
    }

    /// Guarda una validación terminada en el historial.
    pub fn save_validation(&self, project: &Path, report: &Report) -> Result<i64> {
        let json = serde_json::to_string(report).map_err(err)?;
        let candidate = report
            .candidate
            .as_ref()
            .map_or_else(String::new, |c| c.id.clone());
        self.conn
            .execute(
                "INSERT INTO validations (project, stage, candidate, verdict, created_at, report)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    path_key(project),
                    report.stage.as_str(),
                    candidate,
                    format!("{:?}", report.verdict),
                    report.finished_at,
                    json
                ],
            )
            .map_err(err)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Últimas validaciones del proyecto, de la más reciente a la más antigua.
    pub fn validations(&self, project: &Path, limit: usize) -> Result<Vec<(i64, Report)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, report FROM validations WHERE project = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![path_key(project), limit as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(err)?;
        let rows: Vec<(i64, String)> = rows.collect::<rusqlite::Result<_>>().map_err(err)?;
        // Un reporte de una versión anterior que ya no encaja se omite.
        Ok(rows
            .into_iter()
            .filter_map(|(id, json)| Some((id, serde_json::from_str(&json).ok()?)))
            .collect())
    }
}

/// Consumo de modelos de un mes: (proveedor, modelo, tarea, llamadas, tokens entrada, salida, coste).
pub type UsageRow = (String, String, String, i64, i64, i64, f64);

impl Store {
    pub fn save_usage(&self, project: &Path, usage: &[Usage]) -> Result<()> {
        for u in usage {
            self.conn
                .execute(
                    "INSERT INTO usage (project, task, provider, model, input_tokens, output_tokens, cost_usd, local, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        path_key(project),
                        u.task.key(),
                        u.provider,
                        u.model,
                        u.input_tokens as i64,
                        u.output_tokens as i64,
                        u.cost_usd,
                        u.local,
                        u.created_at
                    ],
                )
                .map_err(err)?;
        }
        Ok(())
    }

    /// Gasto del mes natural en proveedores de pago (los locales no cuentan).
    pub fn month_spent(&self, project: &Path) -> Result<f64> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(cost_usd), 0) FROM usage WHERE project = ?1 AND local = 0 AND created_at >= ?2",
                params![path_key(project), crate::evidence::month_start(now())],
                |r| r.get(0),
            )
            .map_err(err)
    }

    pub fn month_usage(&self, project: &Path) -> Result<Vec<UsageRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT provider, model, task, COUNT(*), SUM(input_tokens), SUM(output_tokens), SUM(cost_usd)
                 FROM usage WHERE project = ?1 AND created_at >= ?2
                 GROUP BY provider, model, task ORDER BY SUM(cost_usd) DESC",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(
                params![path_key(project), crate::evidence::month_start(now())],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)
    }

    // ---------------------------------------------------------------- sesiones activas de agentes

    /// Registra o renueva una sesión activa de un agente en un proyecto y rama.
    pub fn register_agent_session(
        &self,
        id: &str,
        project: &Path,
        branch: &str,
        agent: &str,
        pid: Option<u32>,
    ) -> Result<()> {
        let key = self.project_key(project);
        let t = now();
        self.conn
            .execute(
                "INSERT INTO agent_sessions (id, project_key, branch, agent, pid, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    project_key = excluded.project_key,
                    branch = excluded.branch,
                    agent = excluded.agent,
                    pid = excluded.pid,
                    updated_at = excluded.updated_at",
                params![id, key, branch.trim(), agent.trim(), pid, t],
            )
            .map(drop)
            .map_err(err)
    }

    /// Actualiza el latido de una sesión para que no caduque.
    pub fn heartbeat_agent_session(&self, id: &str) -> Result<bool> {
        let t = now();
        self.conn
            .execute(
                "UPDATE agent_sessions SET updated_at = ?2 WHERE id = ?1",
                params![id, t],
            )
            .map(|n| n > 0)
            .map_err(err)
    }

    /// Elimina una sesión al cerrarse la terminal o salir el agente.
    pub fn remove_agent_session(&self, id: &str) -> Result<bool> {
        self.conn
            .execute("DELETE FROM agent_sessions WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .map_err(err)
    }

    /// Limpia sesiones que no han emitido latido en más de `max_age_secs` segundos.
    pub fn prune_stale_agent_sessions(&self, max_age_secs: i64) -> Result<usize> {
        let threshold = now() - max_age_secs;
        self.conn
            .execute(
                "DELETE FROM agent_sessions WHERE updated_at < ?1",
                params![threshold],
            )
            .map_err(err)
    }

    /// Sesiones activas de otros agentes en la misma rama del repositorio (excluyendo opcionalmente una sesión).
    pub fn active_agent_sessions_on_branch(
        &self,
        project: &Path,
        branch: &str,
        exclude_id: Option<&str>,
    ) -> Result<Vec<ActiveAgentSession>> {
        let _ = self.prune_stale_agent_sessions(30);
        let key = self.project_key(project);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, project_key, branch, agent, pid, updated_at
                 FROM agent_sessions
                 WHERE project_key = ?1 AND branch = ?2 AND (?3 IS NULL OR id != ?3)
                 ORDER BY updated_at DESC",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![key, branch.trim(), exclude_id], |r| {
                Ok(ActiveAgentSession {
                    id: r.get(0)?,
                    project_key: r.get(1)?,
                    branch: r.get(2)?,
                    agent: r.get(3)?,
                    pid: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            })
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)
    }

    /// Agentes de las otras sesiones activas en esta rama (sin contar la actual), sin repetir.
    pub fn other_agents_on_branch(&self, project: &Path, branch: &str) -> Vec<String> {
        let me = current_session_id();
        let mut agents: Vec<String> = self
            .active_agent_sessions_on_branch(project, branch, me.as_deref())
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.agent)
            .collect();
        agents.sort();
        agents.dedup();
        agents
    }

    /// Todas las sesiones activas en el repositorio del proyecto.
    pub fn active_agent_sessions(&self, project: &Path) -> Result<Vec<ActiveAgentSession>> {
        let _ = self.prune_stale_agent_sessions(30);
        let key = self.project_key(project);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, project_key, branch, agent, pid, updated_at
                 FROM agent_sessions
                 WHERE project_key = ?1
                 ORDER BY updated_at DESC",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![key], |r| {
                Ok(ActiveAgentSession {
                    id: r.get(0)?,
                    project_key: r.get(1)?,
                    branch: r.get(2)?,
                    agent: r.get(3)?,
                    pid: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            })
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)
    }
}

/// Identificador de sesión para un panel de un proceso de Forge.
pub fn session_id_for_panel(app_pid: u32, panel_id: u64) -> String {
    format!("{app_pid}:{panel_id}")
}

/// Sesión del agente actual: la que Forge pone en el entorno de cada terminal de agente.
pub fn current_session_id() -> Option<String> {
    std::env::var("FORGE_SESSION_ID")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// "hace 5 min", "hace 2 h", "hace 3 días".
pub fn ago_precise(timestamp: i64) -> String {
    match now() - timestamp {
        ..60 => tr!("hace un momento").into(),
        s @ 60..3600 => tr!("hace {p0} min", p0 = s / 60),
        s @ 3600..86_400 => tr!("hace {p0} h", p0 = s / 3600),
        s => tr!("hace {p0} días", p0 = s / 86_400),
    }
}

/// "hoy", "ayer", "hace 3 días"...
pub fn ago(timestamp: i64) -> String {
    match (now() - timestamp) / 86_400 {
        ..=0 => crate::tr!("hoy").into(),
        1 => crate::tr!("ayer").into(),
        days @ 2..=30 => tr!("hace {days} días", days = days),
        days => tr!("hace {p0} meses", p0 = days / 30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writers_wait_for_each_other_instead_of_failing() {
        // Cada agente abre su propio `forge mcp`: varias conexiones escriben a la vez.
        // rusqlite pone 5 s de espera por defecto; si se perdiera, una escritura
        // simultánea fallaría con «database is locked» y se perdería lo guardado.
        let store = Store::in_memory().unwrap();
        let wait: i64 = store
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert!(wait >= 1000, "busy_timeout = {wait} ms");
    }

    #[test]
    fn verbatim_windows_paths_are_migrated_to_plain_ones() {
        // Forge 0.7.1 guardaba los proyectos de Windows como `\\?\C:\…`.
        let store = Store::in_memory().unwrap();
        let old = r"\\?\C:\p\jarvis";
        store.touch(Path::new(old), "jarvis").unwrap();
        store
            .add_idea(Some(Path::new(old)), "plan", "", "x")
            .unwrap();
        store.set_setting("active", old).unwrap();
        strip_verbatim_keys(&store.conn);
        let plain = r"C:\p\jarvis";
        let count =
            |sql: &str| -> i64 { store.conn.query_row(sql, [plain], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT count(*) FROM projects WHERE path = ?1"), 1);
        assert_eq!(count("SELECT count(*) FROM ideas WHERE project = ?1"), 1);
        assert_eq!(store.setting("active").unwrap().as_deref(), Some(plain));
    }

    #[test]
    fn the_database_lives_where_each_system_keeps_app_data() {
        let home = Path::new("/home/ana");
        assert_eq!(
            db_path(home, None, None, "macos"),
            home.join("Library/Application Support/Forge/forge.db")
        );
        assert_eq!(
            db_path(home, None, None, "linux"),
            home.join(".local/share/forge/forge.db")
        );
        assert_eq!(
            db_path(home, Some("/data".into()), None, "linux"),
            Path::new("/data/forge/forge.db")
        );
        // Windows: %LOCALAPPDATA%\\Forge (o AppData\\Local de la carpeta personal).
        let local = Path::new(r"C:\Users\ana\AppData\Local");
        assert_eq!(
            db_path(home, None, Some(local.into()), "windows"),
            local.join("Forge").join("forge.db")
        );
        assert_eq!(
            db_path(home, None, None, "windows"),
            home.join("AppData")
                .join("Local")
                .join("Forge")
                .join("forge.db")
        );
    }

    #[test]
    fn workspace_round_trip_and_recents() {
        let store = Store::in_memory().unwrap();
        let a = Path::new("/p/bovinapp");
        let b = Path::new("/p/reparappi");
        store.touch(a, "Bovinapp").unwrap();
        store.touch(b, "ReparAppi").unwrap();

        let state = serde_json::json!({"layout": {"Leaf": 0}, "focus": 4, "running": ["dev"]});
        store.save_workspace(a, &state, 1).unwrap();
        store
            .save_workspace(b, &serde_json::json!({"focus": 0}), 0)
            .unwrap();
        assert_eq!(store.workspace(a).unwrap(), Some(state));
        assert_eq!(
            store.open_projects().unwrap(),
            vec![b.to_path_buf(), a.to_path_buf()]
        );

        // Cerrar conserva el layout; quitar lo borra de recientes.
        store.set_closed(b).unwrap();
        assert_eq!(store.open_projects().unwrap(), vec![a.to_path_buf()]);
        let recent = store.recent().unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(
            recent.iter().find(|r| r.path == a).map(|r| r.name.as_str()),
            Some("Bovinapp")
        );
        store.remove(b).unwrap();
        assert_eq!(store.recent().unwrap().len(), 1);

        store.set_setting("active", "/p/bovinapp").unwrap();
        assert_eq!(
            store.setting("active").unwrap().as_deref(),
            Some("/p/bovinapp")
        );
    }

    #[test]
    fn exceptions_history() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/x");
        let e = Exception {
            id: 0,
            rule: "lines".into(),
            reason: "archivos generados por Drizzle".into(),
            scope: Scope::Commit,
            stage: Stage::Commit,
            candidate: "sha256:abc".into(),
            branch: "main".into(),
            user: "yo".into(),
            created_at: now(),
            revoked: false,
        };
        let id = store.add_exception(p, &e).unwrap();
        let saved = &store.exceptions(p).unwrap()[0];
        assert_eq!(
            (saved.id, saved.rule.as_str(), saved.scope),
            (id, "lines", Scope::Commit)
        );
        assert!(saved.applies("lines", Stage::Commit, "sha256:abc", "main", now()));
        assert!(
            !saved.applies("lines", Stage::Commit, "sha256:otro", "main", now()),
            "otro diff"
        );
        assert!(store.revoke_exception(p, id).unwrap());
        let revoked = &store.exceptions(p).unwrap()[0];
        assert!(
            revoked.revoked
                && !revoked.applies("lines", Stage::Commit, "sha256:abc", "main", now())
        );
        assert!(store.exceptions(Path::new("/otro")).unwrap().is_empty());
    }

    #[test]
    fn evidence_and_history() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/x");
        let e = Evidence {
            check_id: "tests".into(),
            command: "npm test".into(),
            tree: "abc".into(),
            duration_ms: 1200,
            output: "ok".into(),
            created_at: now(),
        };
        store.save_evidence(p, std::slice::from_ref(&e)).unwrap();
        store.save_evidence(p, std::slice::from_ref(&e)).unwrap(); // repetir no duplica
        assert_eq!(store.evidence(p).unwrap(), vec![e]);
        assert!(store.evidence(Path::new("/otro")).unwrap().is_empty());

        let report = Report {
            project: "x".into(),
            stage: Stage::Push,
            candidate: None,
            branch: "main".into(),
            started_at: now(),
            finished_at: now(),
            verdict: crate::guard::Verdict::Ready,
            progress: (3, 3),
            items: vec![],
            checks: vec![],
            error: None,
        };
        let first = store.save_validation(p, &report).unwrap();
        let second = store.save_validation(p, &report).unwrap();
        let history = store.validations(p, 10).unwrap();
        assert_eq!(
            history.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![second, first]
        );
        assert_eq!(history[0].1.stage, Stage::Push);
    }

    #[test]
    fn usage_and_month_budget() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/x");
        let u = |cost: f64, local: bool, created_at: i64| Usage {
            task: crate::router::Task::CodeReview,
            provider: "anthropic".into(),
            model: "haiku".into(),
            input_tokens: 100,
            output_tokens: 10,
            cost_usd: cost,
            local,
            created_at,
        };
        let old = crate::evidence::month_start(now()) - 10;
        store
            .save_usage(
                p,
                &[
                    u(0.25, false, now()),
                    u(0.50, false, now()),
                    u(9.0, true, now()),
                    u(5.0, false, old),
                ],
            )
            .unwrap();
        assert!(
            (store.month_spent(p).unwrap() - 0.75).abs() < 1e-9,
            "solo pago y solo este mes"
        );
        let rows = store.month_usage(p).unwrap();
        assert_eq!(rows.iter().map(|r| r.3).sum::<i64>(), 3);
    }

    #[test]
    fn relative_days() {
        assert_eq!(ago(now()), "hoy");
        assert_eq!(ago(now() - 86_400), "ayer");
        assert_eq!(ago(now() - 5 * 86_400), "hace 5 días");
    }

    #[test]
    fn agent_sessions_tracking() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/forge");

        // Registrar sesiones
        store
            .register_agent_session("s1", p, "feat/branch-a", "claude", Some(101))
            .unwrap();
        store
            .register_agent_session("s2", p, "feat/branch-a", "agy", Some(102))
            .unwrap();
        store
            .register_agent_session("s3", p, "main", "codex", None)
            .unwrap();

        // Listar sesiones en feat/branch-a sin exclusión
        let sessions = store
            .active_agent_sessions_on_branch(p, "feat/branch-a", None)
            .unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.id == "s1" && s.agent == "claude"));
        assert!(sessions.iter().any(|s| s.id == "s2" && s.agent == "agy"));

        // Listar sesiones en feat/branch-a excluyendo s1 (por ejemplo, el propio panel)
        let other = store
            .active_agent_sessions_on_branch(p, "feat/branch-a", Some("s1"))
            .unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].id, "s2");

        // Listar todas las sesiones del proyecto
        let all = store.active_agent_sessions(p).unwrap();
        assert_eq!(all.len(), 3);

        // Heartbeat renueva la sesión
        assert!(store.heartbeat_agent_session("s1").unwrap());

        // Eliminar s2
        assert!(store.remove_agent_session("s2").unwrap());
        let remaining = store
            .active_agent_sessions_on_branch(p, "feat/branch-a", None)
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "s1");

        // Probar caducidad / poda de sesiones obsoletas
        let old_time = now() - 100;
        store
            .conn
            .execute(
                "INSERT INTO agent_sessions (id, project_key, branch, agent, pid, updated_at)
                 VALUES ('s_old', ?1, 'feat/branch-a', 'qwen', NULL, ?2)",
                params![store.project_key(p), old_time],
            )
            .unwrap();

        // active_agent_sessions_on_branch poda automáticamente (> 30s)
        let after_prune = store
            .active_agent_sessions_on_branch(p, "feat/branch-a", None)
            .unwrap();
        assert_eq!(after_prune.len(), 1);
        assert_eq!(after_prune[0].id, "s1");

        // Exclusión con formato compuesto <app_pid>:<panel> o panel simple
        let comp_id = session_id_for_panel(9999, 42);
        assert_eq!(comp_id, "9999:42");
        store
            .register_agent_session(&comp_id, p, "feat/comp", "claude", None)
            .unwrap();
        // Solo se excluye la sesión exacta: "42" puede ser el panel de otra instancia
        let res = store
            .active_agent_sessions_on_branch(p, "feat/comp", Some("42"))
            .unwrap();
        assert_eq!(res.len(), 1);
        // Excluyendo con "9999:42" debe excluir "9999:42"
        let res2 = store
            .active_agent_sessions_on_branch(p, "feat/comp", Some("9999:42"))
            .unwrap();
        assert!(res2.is_empty());
        // Excluyendo con otro panel ("43") no lo excluye
        let res3 = store
            .active_agent_sessions_on_branch(p, "feat/comp", Some("43"))
            .unwrap();
        assert_eq!(res3.len(), 1);
    }
}
