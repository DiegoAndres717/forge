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

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Store {
    /// `~/Library/Application Support/Forge/forge.db`
    /// `FORGE_DB` permite otra base (tests, varias instalaciones).
    pub fn default_path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("FORGE_DB") {
            return Some(PathBuf::from(path));
        }
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(home.join("Library/Application Support/Forge/forge.db"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        let mut store = Self::init(Connection::open(path).map_err(err)?)?;
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
            );",
        )
        .map_err(err)?;
        crate::memory::init(&conn).map_err(err)?;
        crate::ideas::init(&conn).map_err(err)?;
        crate::events::init(&conn).map_err(err)?;
        Ok(Self {
            conn,
            keys: Mutex::default(),
            accounts_root: None,
            home: std::env::var_os("HOME").map(PathBuf::from),
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
        if key != path {
            for table in ["memories", "ideas"] {
                let _ = self.conn.execute(
                    &format!("UPDATE {table} SET project = ?1 WHERE project = ?2"),
                    params![key, path],
                );
            }
        }
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
}
