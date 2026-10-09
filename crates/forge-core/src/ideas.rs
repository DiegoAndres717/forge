// Planes: lo que hay por hacer en cada proyecto (o en general, sin proyecto), que el
// usuario y los agentes leen, escriben y tachan. Un elemento sin fases es una idea del
// backlog; con fases es un plan (cada fase con su estado, rama opcional y tareas). Vive en
// la base de Forge, nunca en el repositorio.
use crate::tr;
use std::path::Path;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::store::{Store, now};

/// Estados (de un plan y de cada fase): (id, etiqueta).
pub const STATUSES: [(&str, &str); 3] = [
    ("pending", "Backlog"),
    ("doing", "En progreso"),
    ("done", "Completado"),
];

/// Estado interno a partir de lo que diga un agente o el usuario: acepta los ids y
/// sinónimos habituales (`backlog`, `in_progress`, `completed`…).
pub fn normalize_status(status: &str) -> Option<&'static str> {
    match status
        .trim()
        .to_lowercase()
        .replace([' ', '-'], "_")
        .as_str()
    {
        "pending" | "backlog" | "todo" => Some("pending"),
        "doing" | "in_progress" | "progress" | "started" => Some("doing"),
        "done" | "completed" | "complete" | "finished" => Some("done"),
        _ => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Phase {
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default = "pending")]
    pub status: String,
    /// Rama de Git de la fase, si la tiene (no siempre: una fase puede no ser una rama).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

fn pending() -> String {
    "pending".into()
}

impl Phase {
    pub fn new(title: &str) -> Self {
        Phase {
            title: title.trim().to_string(),
            notes: String::new(),
            status: pending(),
            branch: None,
            tasks: Vec::new(),
        }
    }
}

/// Cambio en una fase (lo que venga).
#[derive(Default)]
pub struct PhaseChange<'a> {
    pub status: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub notes: Option<&'a str>,
    /// Tarea (índice desde 0) a marcar como hecha o no.
    pub task: Option<(usize, bool)>,
    pub add_task: Option<&'a str>,
}

/// Estado de un plan según sus fases: todas completadas → completado; alguna empezada →
/// en progreso; ninguna → el que tenía (pero no completado).
fn derived_status(phases: &[Phase], current: &str) -> String {
    if phases.is_empty() {
        current.to_string()
    } else if phases.iter().all(|p| p.status == "done") {
        "done".into()
    } else if phases.iter().any(|p| p.status != "pending") {
        "doing".into()
    } else if current == "done" {
        "pending".into()
    } else {
        current.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Idea {
    pub id: i64,
    pub title: String,
    /// Detalle opcional (contexto, enlace a la conversación, criterio de "hecho").
    pub note: String,
    pub status: String,
    /// Quién la creó: "usuario", "claude-code"…
    pub source: String,
    /// Quién la cambió de estado por última vez.
    pub updated_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Fases del plan (vacío: una idea del backlog sin planificar).
    pub phases: Vec<Phase>,
}

impl Idea {
    pub fn done(&self) -> bool {
        self.status == "done"
    }

    /// Fase en la que va el plan: la primera sin completar.
    pub fn current_phase(&self) -> Option<(usize, &Phase)> {
        self.phases
            .iter()
            .enumerate()
            .find(|(_, p)| p.status != "done")
    }
}

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ideas (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            project    TEXT NOT NULL,          -- '' = lista general
            title      TEXT NOT NULL,
            note       TEXT NOT NULL DEFAULT '',
            status     TEXT NOT NULL DEFAULT 'pending',
            source     TEXT NOT NULL,
            updated_by TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS ideas_project ON ideas(project, status);
        -- Las memorias de tipo «tarea pendiente» (versiones anteriores) pasan a ser ideas.
        INSERT INTO ideas (project, title, note, status, source, updated_by, created_at, updated_at)
            SELECT project, title, body, 'pending', source, source, created_at, created_at
            FROM memories WHERE kind = 'task';
        DELETE FROM memories WHERE kind = 'task';",
    )?;
    // Bases anteriores a los planes: las ideas pasan a ser el backlog (sin fases).
    if conn.prepare("SELECT phases FROM ideas LIMIT 0").is_err() {
        conn.execute(
            "ALTER TABLE ideas ADD COLUMN phases TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    Ok(())
}

fn key(project: Option<&Path>) -> String {
    project.map_or(String::new(), |p| p.to_string_lossy().into_owned())
}

fn check_status(status: &str) -> Result<&'static str, String> {
    normalize_status(status).ok_or_else(|| {
        tr!(
            "estado desconocido \"{status}\" (válidos: backlog, in_progress, done)",
            status = status
        )
    })
}

impl Store {
    /// Una idea para el backlog (sin fases).
    pub fn add_idea(
        &self,
        project: Option<&Path>,
        title: &str,
        note: &str,
        source: &str,
    ) -> Result<i64, String> {
        self.add_plan(project, title, note, &[], source)
    }

    /// Un plan con sus fases (sin fases: una idea del backlog).
    pub fn add_plan(
        &self,
        project: Option<&Path>,
        title: &str,
        note: &str,
        phases: &[Phase],
        source: &str,
    ) -> Result<i64, String> {
        if title.trim().is_empty() {
            return Err(tr!("la idea necesita un título").into());
        }
        let phases = clean_phases(phases)?;
        let status = derived_status(&phases, "pending");
        let t = now();
        self.conn
            .execute(
                "INSERT INTO ideas (project, title, note, status, source, updated_by, created_at, updated_at, phases)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?6, ?7)",
                params![key(project), title.trim(), note.trim(), status, source, t, to_json(&phases)],
            )
            .map_err(|e| e.to_string())?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_idea(&self, project: Option<&Path>, id: i64) -> Result<Option<Idea>, String> {
        Ok(self
            .list_ideas(project, true)?
            .into_iter()
            .find(|i| i.id == id))
    }

    /// Reemplaza las fases del plan (p. ej. al planificar una idea del backlog); el estado
    /// del plan se recalcula. `false` si no es de esa lista.
    pub fn set_phases(
        &self,
        project: Option<&Path>,
        id: i64,
        phases: &[Phase],
        by: &str,
    ) -> Result<bool, String> {
        let Some(idea) = self.get_idea(project, id)? else {
            return Ok(false);
        };
        let phases = clean_phases(phases)?;
        let status = derived_status(&phases, &idea.status);
        let changed = self
            .conn
            .execute(
                "UPDATE ideas SET phases = ?3, status = ?4, updated_by = ?5, updated_at = ?6
                 WHERE project = ?1 AND id = ?2",
                params![key(project), id, to_json(&phases), status, by, now()],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    /// Cambia una fase (índice desde 0): estado, rama, notas o sus tareas.
    pub fn update_phase(
        &self,
        project: Option<&Path>,
        id: i64,
        index: usize,
        change: PhaseChange,
        by: &str,
    ) -> Result<bool, String> {
        let Some(idea) = self.get_idea(project, id)? else {
            return Ok(false);
        };
        let mut phases = idea.phases;
        let count = phases.len();
        let phase = phases.get_mut(index).ok_or_else(|| {
            tr!(
                "el plan #{id} tiene {count} fases; no existe la fase {n}",
                id = id,
                count = count,
                n = index + 1
            )
        })?;
        if let Some(status) = change.status {
            phase.status = check_status(status)?.into();
        }
        if let Some(branch) = change.branch {
            let branch = branch.trim();
            phase.branch = (!branch.is_empty()).then(|| branch.to_string());
        }
        if let Some(notes) = change.notes {
            phase.notes = notes.trim().to_string();
        }
        if let Some(text) = change.add_task.map(str::trim).filter(|t| !t.is_empty()) {
            phase.tasks.push(Task {
                text: text.to_string(),
                done: false,
            });
        }
        if let Some((task, done)) = change.task {
            let tasks = phase.tasks.len();
            let item = phase.tasks.get_mut(task).ok_or_else(|| {
                tr!(
                    "la fase {n} tiene {tasks} tareas; no existe la tarea {t}",
                    n = index + 1,
                    tasks = tasks,
                    t = task + 1
                )
            })?;
            item.done = done;
        }
        self.set_phases(project, id, &phases, by)
    }

    /// Abiertas primero (en curso, luego pendientes, de la más antigua a la más nueva);
    /// después las hechas, de la más reciente a la más antigua.
    pub fn list_ideas(
        &self,
        project: Option<&Path>,
        include_done: bool,
    ) -> Result<Vec<Idea>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, note, status, source, updated_by, created_at, updated_at, phases FROM ideas
                 WHERE project = ?1 AND (?2 OR status != 'done')
                 ORDER BY status = 'done', status != 'doing',
                          CASE WHEN status = 'done' THEN -updated_at ELSE created_at END",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![key(project), include_done], |r| {
                Ok(Idea {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    note: r.get(2)?,
                    status: r.get(3)?,
                    source: r.get(4)?,
                    updated_by: r.get(5)?,
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                    phases: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }

    /// Ideas sin hacer (para el contador de la barra lateral).
    pub fn open_ideas(&self, project: Option<&Path>) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM ideas WHERE project = ?1 AND status != 'done'",
                params![key(project)],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }

    /// Cambia estado, título o nota (lo que venga). `false` si la idea no es de esa lista.
    pub fn update_idea(
        &self,
        project: Option<&Path>,
        id: i64,
        status: Option<&str>,
        title: Option<&str>,
        note: Option<&str>,
        by: &str,
    ) -> Result<bool, String> {
        let status = status.map(check_status).transpose()?;
        if title.is_some_and(|t| t.trim().is_empty()) {
            return Err(tr!("la idea necesita un título").into());
        }
        let changed = self
            .conn
            .execute(
                "UPDATE ideas SET status = COALESCE(?3, status), title = COALESCE(?4, title),
                        note = COALESCE(?5, note), updated_by = ?6, updated_at = ?7
                 WHERE project = ?1 AND id = ?2",
                params![
                    key(project),
                    id,
                    status,
                    title.map(str::trim),
                    note.map(str::trim),
                    by,
                    now()
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn delete_idea(&self, project: Option<&Path>, id: i64) -> Result<bool, String> {
        self.conn
            .execute(
                "DELETE FROM ideas WHERE project = ?1 AND id = ?2",
                params![key(project), id],
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())
    }
}

fn to_json(phases: &[Phase]) -> String {
    serde_json::to_string(phases).unwrap_or_else(|_| "[]".into())
}

/// Fases válidas: con título, estados conocidos y ramas sin espacios.
fn clean_phases(phases: &[Phase]) -> Result<Vec<Phase>, String> {
    phases
        .iter()
        .map(|p| {
            if p.title.trim().is_empty() {
                return Err(tr!("cada fase necesita un título").to_string());
            }
            let branch = p.branch.as_deref().map(str::trim).filter(|b| !b.is_empty());
            if branch.is_some_and(|b| b.contains(char::is_whitespace)) {
                return Err(tr!("nombre de rama no válido").to_string());
            }
            Ok(Phase {
                title: p.title.trim().to_string(),
                notes: p.notes.trim().to_string(),
                status: check_status(&p.status)?.into(),
                branch: branch.map(str::to_string),
                tasks: p
                    .tasks
                    .iter()
                    .filter(|t| !t.text.trim().is_empty())
                    .map(|t| Task {
                        text: t.text.trim().to_string(),
                        done: t.done,
                    })
                    .collect(),
            })
        })
        .collect()
}

fn mark(status: &str) -> &'static str {
    match status {
        "done" => "✓",
        "doing" => "◐",
        _ => "○",
    }
}

/// Para agentes y CLI: `#3 ◐ Notificar vacunas vencidas (claude-code)`, la nota y, si es
/// un plan, sus fases numeradas con rama y tareas.
pub fn render(idea: &Idea) -> String {
    let mut line = format!(
        "#{} {} {} ({})",
        idea.id,
        mark(&idea.status),
        idea.title,
        idea.source
    );
    if !idea.note.is_empty() {
        line += &format!("\n    {}", idea.note.replace('\n', "\n    "));
    }
    for (i, phase) in idea.phases.iter().enumerate() {
        line += &format!("\n    {}. {} {}", i + 1, mark(&phase.status), phase.title);
        if let Some(branch) = &phase.branch {
            line += &format!(" [{branch}]");
        }
        if !phase.notes.is_empty() {
            line += &format!("\n       {}", phase.notes.replace('\n', "\n       "));
        }
        for (t, task) in phase.tasks.iter().enumerate() {
            let check = if task.done { "x" } else { " " };
            line += &format!("\n       {}.{} [{check}] {}", i + 1, t + 1, task.text);
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ideas_lifecycle_per_project_and_general() {
        let store = Store::in_memory().unwrap();
        let bovinapp = Path::new("/p/bovinapp");
        let a = store
            .add_idea(Some(bovinapp), "Exportar a Excel", "", "usuario")
            .unwrap();
        let b = store
            .add_idea(
                Some(bovinapp),
                "Notificar vencidas",
                "push y correo",
                "claude-code",
            )
            .unwrap();
        store
            .add_idea(None, "App para talleres", "", "usuario")
            .unwrap();
        assert!(store.add_idea(None, "  ", "", "usuario").is_err());

        // En curso primero; las hechas al final y solo si se piden.
        assert!(
            store
                .update_idea(Some(bovinapp), b, Some("doing"), None, None, "claude-code")
                .unwrap()
        );
        let list = store.list_ideas(Some(bovinapp), true).unwrap();
        assert_eq!(list.iter().map(|i| i.id).collect::<Vec<_>>(), vec![b, a]);
        assert!(
            store
                .update_idea(Some(bovinapp), a, Some("done"), None, None, "codex")
                .unwrap()
        );
        assert_eq!(store.open_ideas(Some(bovinapp)).unwrap(), 1);
        assert_eq!(store.list_ideas(Some(bovinapp), false).unwrap().len(), 1);
        let done = store
            .list_ideas(Some(bovinapp), true)
            .unwrap()
            .pop()
            .unwrap();
        assert!(done.done() && done.updated_by == "codex");

        // Cada lista es independiente: no se toca una idea de otro proyecto.
        assert!(
            !store
                .update_idea(None, a, Some("done"), None, None, "x")
                .unwrap()
        );
        assert!(
            store
                .update_idea(Some(bovinapp), a, Some("tal vez"), None, None, "x")
                .is_err()
        );
        assert_eq!(
            store.list_ideas(None, true).unwrap()[0].title,
            "App para talleres"
        );
        assert!(store.delete_idea(Some(bovinapp), a).unwrap());
        assert!(
            render(&store.list_ideas(Some(bovinapp), true).unwrap()[0]).starts_with(&format!(
                "#{b} ◐ Notificar vencidas (claude-code)\n    push y correo"
            ))
        );
    }

    #[test]
    fn plans_advance_by_phases_and_derive_their_status() {
        let store = Store::in_memory().unwrap();
        let p = Some(Path::new("/p/forge"));
        let mut model = Phase::new("Modelo de datos");
        model.branch = Some("feat/plans-model".into());
        model.tasks = vec![Task {
            text: "Migración".into(),
            done: false,
        }];
        let id = store
            .add_plan(
                p,
                "Planes por fases",
                "",
                &[model, Phase::new("Panel")],
                "claude-code",
            )
            .unwrap();
        let plan = store.get_idea(p, id).unwrap().unwrap();
        assert_eq!(plan.status, "pending", "sin empezar: en el backlog");
        assert_eq!(plan.current_phase().unwrap().0, 0);

        // Empezar la fase 1 pone el plan en progreso; los sinónimos valen.
        let start = PhaseChange {
            status: Some("in_progress"),
            ..Default::default()
        };
        assert!(store.update_phase(p, id, 0, start, "claude-code").unwrap());
        assert_eq!(store.get_idea(p, id).unwrap().unwrap().status, "doing");

        // Tachar la tarea, añadir otra y cerrar las dos fases: completado.
        let tick = PhaseChange {
            task: Some((0, true)),
            add_task: Some("Tests"),
            status: Some("done"),
            ..Default::default()
        };
        store.update_phase(p, id, 0, tick, "claude-code").unwrap();
        let done = PhaseChange {
            status: Some("completed"),
            branch: Some("feat/plans-panel"),
            ..Default::default()
        };
        store.update_phase(p, id, 1, done, "codex").unwrap();
        let plan = store.get_idea(p, id).unwrap().unwrap();
        assert_eq!(plan.status, "done");
        assert!(plan.phases[0].tasks[0].done && !plan.phases[0].tasks[1].done);
        assert_eq!(plan.phases[1].branch.as_deref(), Some("feat/plans-panel"));
        assert!(render(&plan).contains("1. ✓ Modelo de datos [feat/plans-model]"));
        assert!(render(&plan).contains("1.1 [x] Migración"));

        // Errores claros: fase o tarea que no existe, estado desconocido, rama con espacios.
        let bad = |c: PhaseChange| store.update_phase(p, id, 0, c, "x").unwrap_err();
        assert!(
            store
                .update_phase(p, id, 5, PhaseChange::default(), "x")
                .unwrap_err()
                .contains("no existe la fase 6")
        );
        assert!(
            bad(PhaseChange {
                task: Some((9, true)),
                ..Default::default()
            })
            .contains("no existe la tarea 10")
        );
        assert!(
            bad(PhaseChange {
                status: Some("tal vez"),
                ..Default::default()
            })
            .contains("estado desconocido")
        );
        let mut spaced = Phase::new("X");
        spaced.branch = Some("mi rama".into());
        assert!(store.set_phases(p, id, &[spaced], "x").is_err());

        // Una idea del backlog que se planifica después.
        let idea = store.add_idea(p, "Exportar a CSV", "", "usuario").unwrap();
        assert!(store.get_idea(p, idea).unwrap().unwrap().phases.is_empty());
        assert!(
            store
                .set_phases(p, idea, &[Phase::new("Botón")], "claude-code")
                .unwrap()
        );
        assert_eq!(store.get_idea(p, idea).unwrap().unwrap().phases.len(), 1);
    }

    #[test]
    fn older_databases_get_the_phases_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (project TEXT, kind TEXT, title TEXT, body TEXT, source TEXT, created_at INTEGER);
             CREATE TABLE ideas (id INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL, title TEXT NOT NULL,
                 note TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'pending', source TEXT NOT NULL,
                 updated_by TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
             INSERT INTO ideas (project, title, source, updated_by, created_at, updated_at)
                 VALUES ('/p', 'Idea vieja', 'usuario', 'usuario', 1, 1);",
        )
        .unwrap();
        init(&conn).unwrap();
        init(&conn).unwrap(); // dos veces: no falla
        let phases: String = conn
            .query_row("SELECT phases FROM ideas", [], |r| r.get(0))
            .unwrap();
        assert_eq!(phases, "[]");
    }

    #[test]
    fn task_memories_become_ideas() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/x");
        store
            .conn
            .execute(
                "INSERT INTO memories (project, kind, title, body, source, created_at) VALUES ('/p/x', 'task', 'Migrar a PG17', 'antes de junio', 'usuario', 1)",
                [],
            )
            .unwrap();
        init(&store.conn).unwrap();
        let ideas = store.list_ideas(Some(p), true).unwrap();
        assert_eq!(
            (ideas[0].title.as_str(), ideas[0].note.as_str()),
            ("Migrar a PG17", "antes de junio")
        );
        assert_eq!(store.count_memories(p).unwrap(), 0);
    }
}
