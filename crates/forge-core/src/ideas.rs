// Ideas: lista de pendientes por proyecto (o general, sin proyecto) que el usuario y los
// agentes leen, escriben y tachan. Vive en la base de Forge, nunca en el repositorio.
use crate::tr;
use std::path::Path;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::store::{Store, now};

/// Estados: (id, etiqueta).
pub const STATUSES: [(&str, &str); 3] = [
    ("pending", "Pendiente"),
    ("doing", "En curso"),
    ("done", "Hecha"),
];

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
}

impl Idea {
    pub fn done(&self) -> bool {
        self.status == "done"
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
    )
}

fn key(project: Option<&Path>) -> String {
    project.map_or(String::new(), |p| p.to_string_lossy().into_owned())
}

fn check_status(status: &str) -> Result<(), String> {
    if STATUSES.iter().any(|(id, _)| *id == status) {
        Ok(())
    } else {
        Err(tr!(
            "estado desconocido \"{status}\" (válidos: pending, doing, done)",
            status = status
        ))
    }
}

impl Store {
    pub fn add_idea(
        &self,
        project: Option<&Path>,
        title: &str,
        note: &str,
        source: &str,
    ) -> Result<i64, String> {
        if title.trim().is_empty() {
            return Err(tr!("la idea necesita un título").into());
        }
        let t = now();
        self.conn
            .execute(
                "INSERT INTO ideas (project, title, note, source, updated_by, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?5)",
                params![key(project), title.trim(), note.trim(), source, t],
            )
            .map_err(|e| e.to_string())?;
        Ok(self.conn.last_insert_rowid())
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
                "SELECT id, title, note, status, source, updated_by, created_at, updated_at FROM ideas
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
        if let Some(status) = status {
            check_status(status)?;
        }
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

/// Una línea para agentes y CLI: `#3 ◐ Notificar vacunas vencidas (claude-code)`.
pub fn render(idea: &Idea) -> String {
    let mark = match idea.status.as_str() {
        "done" => "✓",
        "doing" => "◐",
        _ => "○",
    };
    let mut line = format!("#{} {mark} {} ({})", idea.id, idea.title, idea.source);
    if !idea.note.is_empty() {
        line += &format!("\n    {}", idea.note.replace('\n', "\n    "));
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
