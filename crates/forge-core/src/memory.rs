// Memoria del proyecto: decisiones, arquitectura, errores resueltos, comandos útiles…
// Local (SQLite con búsqueda de texto completo) e independiente del agente y la terminal.
use std::path::Path;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::store::{Store, now};

/// Tipos de memoria (plan §16): (id, etiqueta). Las tareas pendientes son ideas (`ideas`).
pub const KINDS: [(&str, &str); 6] = [
    ("decision", "Decisión"),
    ("architecture", "Arquitectura"),
    ("error", "Error resuelto"),
    ("command", "Comando útil"),
    ("convention", "Convención"),
    ("note", "Nota"),
];

pub fn kind_label(kind: &str) -> &str {
    KINDS
        .iter()
        .find(|(id, _)| *id == kind)
        .map_or(kind, |(_, label)| label)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Memory {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub tags: String,
    /// Quién la guardó: "usuario", "claude-code", "codex-mcp-client"…
    pub source: String,
    pub created_at: i64,
    /// Fragmento con las coincidencias marcadas [así] (solo en búsquedas).
    pub snippet: Option<String>,
}

/// Tablas de memoria + índice de texto completo (tildes ignoradas) sincronizado por triggers.
pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memories (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            project    TEXT NOT NULL,
            kind       TEXT NOT NULL,
            title      TEXT NOT NULL,
            body       TEXT NOT NULL,
            tags       TEXT NOT NULL DEFAULT '',
            source     TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
            title, body, tags,
            content = 'memories', content_rowid = 'id',
            tokenize = 'unicode61 remove_diacritics 2'
        );
        CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, title, body, tags) VALUES (new.id, new.title, new.body, new.tags);
        END;
        CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, body, tags) VALUES ('delete', old.id, old.title, old.body, old.tags);
        END;",
    )
}

/// Consulta FTS5 segura: cada palabra entre comillas y como prefijo (`"vacun"*`), todas obligatorias.
fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn key(project: &Path) -> String {
    project.to_string_lossy().into_owned()
}

impl Store {
    pub fn add_memory(
        &self,
        project: &Path,
        kind: &str,
        title: &str,
        body: &str,
        tags: &str,
        source: &str,
    ) -> Result<i64, String> {
        if !KINDS.iter().any(|(id, _)| *id == kind) {
            let valid: Vec<&str> = KINDS.iter().map(|(id, _)| *id).collect();
            return Err(format!(
                "tipo de memoria desconocido \"{kind}\" (válidos: {})",
                valid.join(", ")
            ));
        }
        if title.trim().is_empty() {
            return Err("la memoria necesita un título".into());
        }
        self.conn
            .execute(
                "INSERT INTO memories (project, kind, title, body, tags, source, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![key(project), kind, title.trim(), body.trim(), tags.trim(), source, now()],
            )
            .map_err(|e| e.to_string())?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Búsqueda por relevancia (el título pesa más). Sin texto: las más recientes.
    pub fn search_memories(
        &self,
        project: &Path,
        query: &str,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Memory>, String> {
        let Some(fts) = fts_query(query) else {
            return self.list_memories(project, kind, limit);
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT m.id, m.kind, m.title, m.body, m.tags, m.source, m.created_at,
                        snippet(memories_fts, -1, '[', ']', '…', 14)
                 FROM memories_fts JOIN memories m ON m.id = memories_fts.rowid
                 WHERE memories_fts MATCH ?1 AND m.project = ?2 AND (?3 IS NULL OR m.kind = ?3)
                 ORDER BY bm25(memories_fts, 10.0, 1.0, 5.0) LIMIT ?4",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![fts, key(project), kind, limit as i64], |r| {
                Ok(Memory {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    title: r.get(2)?,
                    body: r.get(3)?,
                    tags: r.get(4)?,
                    source: r.get(5)?,
                    created_at: r.get(6)?,
                    snippet: r.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }

    pub fn list_memories(
        &self,
        project: &Path,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Memory>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, kind, title, body, tags, source, created_at FROM memories
                 WHERE project = ?1 AND (?2 IS NULL OR kind = ?2) ORDER BY id DESC LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![key(project), kind, limit as i64], |r| {
                Ok(Memory {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    title: r.get(2)?,
                    body: r.get(3)?,
                    tags: r.get(4)?,
                    source: r.get(5)?,
                    created_at: r.get(6)?,
                    snippet: None,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }

    pub fn count_memories(&self, project: &Path) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE project = ?1",
                params![key(project)],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }

    pub fn delete_memory(&self, project: &Path, id: i64) -> Result<bool, String> {
        self.conn
            .execute(
                "DELETE FROM memories WHERE id = ?1 AND project = ?2",
                params![id, key(project)],
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())
    }
}

/// Texto de una memoria para un agente o la terminal.
pub fn render(m: &Memory) -> String {
    let tags = if m.tags.is_empty() {
        String::new()
    } else {
        format!(" · etiquetas: {}", m.tags)
    };
    format!(
        "#{} [{}] {}\n{}\n(guardada por {}, {}{tags})",
        m.id,
        kind_label(&m.kind),
        m.title,
        m.body,
        m.source,
        crate::store::ago_precise(m.created_at)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ignores_accents_uses_prefixes_and_filters() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/bovinapp");
        let other = Path::new("/p/otro");
        store
            .add_memory(
                p,
                "decision",
                "Vacunación en lotes",
                "Las vacunas se registran por lote para no duplicar animales.",
                "vacunas,lotes",
                "usuario",
            )
            .unwrap();
        store
            .add_memory(
                p,
                "error",
                "Error de migración",
                "drizzle-kit falló por un índice duplicado en vacunacion.",
                "",
                "claude-code",
            )
            .unwrap();
        store
            .add_memory(
                p,
                "command",
                "Levantar base de datos",
                "docker compose up postgres",
                "",
                "usuario",
            )
            .unwrap();
        store
            .add_memory(
                other,
                "decision",
                "Vacunación",
                "otro proyecto",
                "",
                "usuario",
            )
            .unwrap();

        // "vacunacion" sin tilde y "vacun" como prefijo encuentran "Vacunación".
        let hits = store.search_memories(p, "vacunacion", None, 10).unwrap();
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].title, "Vacunación en lotes", "el título pesa más");
        assert!(hits[0].snippet.as_deref().unwrap_or("").contains('['));
        assert_eq!(
            store
                .search_memories(p, "vacun lote", None, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search_memories(p, "vacun", Some("error"), 10)
                .unwrap()[0]
                .source,
            "claude-code"
        );
        // Caracteres raros no rompen la consulta.
        assert!(
            store
                .search_memories(p, "\"; DROP TABLE memories; --", None, 10)
                .is_ok()
        );

        assert_eq!(store.list_memories(p, None, 10).unwrap().len(), 3);
        assert_eq!(store.count_memories(p).unwrap(), 3);
        let id = hits[1].id;
        assert!(store.delete_memory(p, id).unwrap());
        assert_eq!(
            store
                .search_memories(p, "migracion", None, 10)
                .unwrap()
                .len(),
            0,
            "el índice se actualiza al borrar"
        );
        assert!(store.add_memory(p, "otra-cosa", "x", "", "", "u").is_err());
    }
}
