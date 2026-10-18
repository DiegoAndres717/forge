// Memoria del proyecto: decisiones, arquitectura, errores resueltos, comandos útiles…
// Local (SQLite con búsqueda de texto completo) e independiente del agente y la terminal.
use crate::tr;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
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
        .map_or(kind, |(_, label)| crate::i18n::t(label))
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
    /// "active" (vigente), "superseded" (reemplazada) o "archived" (archivada).
    pub status: String,
    /// La memoria que la reemplazó.
    pub superseded_by: Option<i64>,
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
        END;
        CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, body, tags) VALUES ('delete', old.id, old.title, old.body, old.tags);
            INSERT INTO memories_fts(rowid, title, body, tags) VALUES (new.id, new.title, new.body, new.tags);
        END;
        CREATE TABLE IF NOT EXISTS memory_reads (
            memory_id INTEGER PRIMARY KEY,
            reads     INTEGER NOT NULL,
            last_read INTEGER NOT NULL
        );",
    )?;
    // Estados (vigente, reemplazada, archivada). Las bases de antes ganan las columnas con
    // valor por defecto: todas sus memorias siguen vigentes.
    // Columna a columna: si otro proceso (la app, la CLI, el servidor MCP) la añadió a la
    // vez, el «duplicate column» no es un error.
    for (column, ddl) in [
        ("status", "status TEXT NOT NULL DEFAULT 'active'"),
        ("superseded_by", "superseded_by INTEGER"),
    ] {
        if conn
            .prepare(&format!("SELECT {column} FROM memories LIMIT 0"))
            .is_err()
            && let Err(e) = conn.execute_batch(&format!("ALTER TABLE memories ADD COLUMN {ddl}"))
            && !e.to_string().contains("duplicate column")
        {
            return Err(e);
        }
    }
    Ok(())
}

/// Estados de una memoria. Solo las vigentes llegan solas a los agentes.
pub const STATUSES: [&str; 3] = ["active", "superseded", "archived"];

const COLUMNS: &str =
    "m.id, m.kind, m.title, m.body, m.tags, m.source, m.created_at, m.status, m.superseded_by";

fn from_row(r: &rusqlite::Row, snippet: bool) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: r.get(0)?,
        kind: r.get(1)?,
        title: r.get(2)?,
        body: r.get(3)?,
        tags: r.get(4)?,
        source: r.get(5)?,
        created_at: r.get(6)?,
        status: r.get(7)?,
        superseded_by: r.get(8)?,
        snippet: if snippet { r.get(9)? } else { None },
    })
}

/// Consulta FTS5 segura: cada palabra entre comillas y como prefijo (`"vacun"*`). Basta con
/// una; bm25 pone primero las que tienen más.
fn fts_query(query: &str) -> Option<String> {
    let all: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    // Con OR, «de» o «la» como prefijo traerían casi todo: fuera el relleno y lo muy corto,
    // salvo que no quede nada más.
    let meaningful: Vec<&str> = all
        .iter()
        .copied()
        .filter(|t| t.chars().count() >= 3 && !words(t).is_empty())
        .collect();
    let terms = if meaningful.is_empty() {
        all
    } else {
        meaningful
    };
    (!terms.is_empty()).then(|| {
        terms
            .iter()
            .map(|t| format!("\"{t}\"*"))
            .collect::<Vec<_>>()
            .join(" OR ")
    })
}

/// Palabras de relleno que no dicen de qué trata un título.
const STOPWORDS: [&str; 19] = [
    "con", "por", "para", "los", "las", "del", "que", "una", "uno", "sin", "como", "pero", "esto",
    "the", "and", "for", "with", "from", "not",
];

/// La nota de la última sesión (`save_session_summary`), en cualquiera de los dos idiomas.
fn is_session_note(m: &Memory) -> bool {
    m.kind == "note"
        && ["Resumen de la última sesión", "Summary of the last session"]
            .iter()
            .any(|t| m.title.starts_with(t))
}

/// Palabras significativas de un título: minúsculas, sin tildes, de 3 letras o más.
fn words(text: &str) -> Vec<String> {
    let folded: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            'ñ' => 'n',
            c => c,
        })
        .collect();
    let mut out: Vec<String> = folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(w))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Dos títulos hablan de lo mismo si comparten al menos 2/3 de sus palabras.
fn same_topic(a: &str, b: &str) -> bool {
    let (wa, wb) = (words(a), words(b));
    let shared = wa.iter().filter(|w| wb.contains(w)).count();
    let most = wa.len().max(wb.len());
    most > 0 && shared * 3 >= most * 2
}

impl Store {
    /// Un agente leyó estas memorias (memory_get o un acierto de memory_search). Aparte de
    /// `memories` para no reindexar la búsqueda en cada lectura.
    // ponytail: las filas de memorias borradas se quedan; los ids no se reutilizan.
    pub fn note_memory_reads(&self, ids: &[i64]) -> Result<(), String> {
        let at = now();
        for id in ids {
            self.conn
                .execute(
                    "INSERT INTO memory_reads (memory_id, reads, last_read) VALUES (?1, 1, ?2)
                     ON CONFLICT(memory_id) DO UPDATE SET reads = reads + 1, last_read = ?2",
                    params![id, at],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Lecturas de cada memoria leída alguna vez: id → (veces, última).
    pub fn memory_reads(&self) -> Result<std::collections::HashMap<i64, (i64, i64)>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT memory_id, reads, last_read FROM memory_reads")
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))
            .and_then(|rows| rows.collect())
            .map_err(|e| e.to_string())
    }

    pub fn add_memory(
        &self,
        project: &Path,
        kind: &str,
        title: &str,
        body: &str,
        tags: &str,
        source: &str,
    ) -> Result<i64, String> {
        // La memoria se reinyecta en todos los agentes: nunca guarda secretos pegados.
        let (title, body) = (
            crate::guard::mask_secrets(title),
            crate::guard::mask_secrets(body),
        );
        let (title, body) = (title.as_str(), body.as_str());
        if !KINDS.iter().any(|(id, _)| *id == kind) {
            let valid: Vec<&str> = KINDS.iter().map(|(id, _)| *id).collect();
            return Err(tr!(
                "tipo de memoria desconocido \"{kind}\" (válidos: {p0})",
                p0 = valid.join(", "),
                kind = kind
            ));
        }
        if title.trim().is_empty() {
            return Err(tr!("la memoria necesita un título").into());
        }
        self.conn
            .execute(
                "INSERT INTO memories (project, kind, title, body, tags, source, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![self.project_key(project), kind, title.trim(), body.trim(), tags.trim(), source, now()],
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
        self.fts_memories(project, &fts, kind, limit)
    }

    /// Memorias que vienen al caso para un mensaje libre (las pistas del mod en cada mensaje).
    /// Cada palabra significativa del mensaje suma 2 si está en el título o las etiquetas y 1
    /// si solo está en el texto; hacen falta 2 puntos: una palabra suelta en un texto largo no
    /// basta. La nota de la última sesión nunca: casi cualquier mensaje coincide con ella.
    pub fn recall_memories(
        &self,
        project: &Path,
        text: &str,
        limit: usize,
    ) -> Result<Vec<Memory>, String> {
        // Las más largas primero: son las más específicas. Una palabra que sale en más de un
        // cuarto de las memorias («forge» en el proyecto Forge) no dice de qué trata ninguna.
        // ponytail: una consulta por palabra (12 como mucho); con miles de memorias, cachear.
        let total = self.count_memories(project)? as usize;
        let key = self.project_key(project);
        let common = |t: &str| {
            self.conn
                .query_row(
                    "SELECT COUNT(*) FROM memories_fts JOIN memories m ON m.id = memories_fts.rowid
                     WHERE memories_fts MATCH ?1 AND m.project = ?2",
                    params![format!("\"{t}\"*"), key],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0) as usize
                * 4
                > total.max(8)
        };
        let mut terms = words(text);
        terms.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
        let terms: Vec<String> = terms.into_iter().filter(|t| !common(t)).take(12).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let query: Vec<String> = terms.iter().map(|w| format!("\"{w}\"*")).collect();
        let found = self.fts_memories(project, &query.join(" OR "), None, limit * 5)?;
        let has = |haystack: &[String], term: &str| haystack.iter().any(|w| w.starts_with(term));
        Ok(found
            .into_iter()
            .filter(|m| !is_session_note(m) && m.status == "active")
            .filter(|m| {
                let head = words(&format!("{} {}", m.title, m.tags));
                let body = words(&m.body);
                let score: usize = terms
                    .iter()
                    .map(|t| {
                        if has(&head, t) {
                            2
                        } else {
                            usize::from(has(&body, t))
                        }
                    })
                    .sum();
                score >= 2
            })
            .take(limit)
            .collect())
    }

    fn fts_memories(
        &self,
        project: &Path,
        fts: &str,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Memory>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS}, snippet(memories_fts, -1, '[', ']', '…', 14)
                 FROM memories_fts JOIN memories m ON m.id = memories_fts.rowid
                 WHERE memories_fts MATCH ?1 AND m.project = ?2 AND (?3 IS NULL OR m.kind = ?3)
                 ORDER BY bm25(memories_fts, 10.0, 1.0, 5.0) LIMIT ?4"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                params![fts, self.project_key(project), kind, limit as i64],
                |r| from_row(r, true),
            )
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
            .prepare(&format!(
                "SELECT {COLUMNS} FROM memories m
                 WHERE project = ?1 AND (?2 IS NULL OR kind = ?2) ORDER BY id DESC LIMIT ?3"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                params![self.project_key(project), kind, limit as i64],
                |r| from_row(r, false),
            )
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }

    /// Una memoria por id, siempre dentro del proyecto actual.
    pub fn get_memory(&self, project: &Path, id: i64) -> Result<Option<Memory>, String> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM memories m WHERE id = ?1 AND project = ?2"),
                params![id, self.project_key(project)],
                |r| from_row(r, false),
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    /// Guarda el resumen de la última sesión (tras compactar): una sola nota por proyecto
    /// que se actualiza cada vez. Devuelve su id; un resumen vacío no cambia nada.
    pub fn save_session_summary(
        &self,
        project: &Path,
        summary: &str,
        agent: Option<&str>,
    ) -> Result<i64, String> {
        // Una nota por agente: el resumen de uno no pisa el de otro.
        let base = tr!("Resumen de la última sesión");
        let title = match agent.map(str::trim).filter(|a| !a.is_empty()) {
            Some(agent) => format!("{base} · {agent}"),
            None => base.to_string(),
        };
        let title = title.as_str();
        // ponytail: recorte fijo; es una pista para retomar, no el historial entero.
        let body: String = summary.trim().chars().take(4000).collect();
        let notes = self.list_memories(project, Some("note"), 500)?;
        // La nota de antes (sin agente) la adopta el primer resumen de un agente.
        let existing = match notes.iter().find(|m| m.title == title) {
            Some(m) => Some(m.clone()),
            None => notes.into_iter().find(|m| m.title == *base).inspect(|m| {
                let _ = self.update_memory(project, m.id, None, Some(title), None, None);
            }),
        };
        match existing {
            Some(m) if body.is_empty() => Ok(m.id),
            Some(m) => {
                self.update_memory(project, m.id, None, None, Some(&body), None)?;
                Ok(m.id)
            }
            None if body.is_empty() => Err(tr!("el resumen está vacío").into()),
            None => self.add_memory(project, "note", title, &body, "", "forge"),
        }
    }

    /// Memoria existente que trata de lo mismo que `title` (para no guardar duplicados).
    // ponytail: recorre las 500 más recientes; usar FTS si un proyecto llega a miles.
    pub fn similar_memory(&self, project: &Path, title: &str) -> Result<Option<Memory>, String> {
        Ok(self
            .list_memories(project, None, 500)?
            .into_iter()
            .find(|m| m.status == "active" && same_topic(&m.title, title)))
    }

    /// Cambia los campos dados de una memoria (los `None` se quedan como estaban).
    pub fn update_memory(
        &self,
        project: &Path,
        id: i64,
        kind: Option<&str>,
        title: Option<&str>,
        body: Option<&str>,
        tags: Option<&str>,
    ) -> Result<bool, String> {
        if let Some(kind) = kind
            && !KINDS.iter().any(|(id, _)| *id == kind)
        {
            let valid: Vec<&str> = KINDS.iter().map(|(id, _)| *id).collect();
            return Err(tr!(
                "tipo de memoria desconocido \"{kind}\" (válidos: {p0})",
                p0 = valid.join(", "),
                kind = kind
            ));
        }
        if title.is_some_and(|t| t.trim().is_empty()) {
            return Err(tr!("la memoria necesita un título").into());
        }
        let title = title.map(crate::guard::mask_secrets);
        let body = body.map(crate::guard::mask_secrets);
        let (title, body) = (title.as_deref(), body.as_deref());
        self.conn
            .execute(
                "UPDATE memories SET kind = COALESCE(?3, kind), title = COALESCE(?4, title),
                 body = COALESCE(?5, body), tags = COALESCE(?6, tags) WHERE id = ?1 AND project = ?2",
                params![
                    id,
                    self.project_key(project),
                    kind,
                    title.map(str::trim),
                    body.map(str::trim),
                    tags.map(str::trim)
                ],
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())
    }

    /// Vigente, reemplazada (por `by`) o archivada. Nada se borra: se puede restaurar.
    pub fn set_memory_status(
        &self,
        project: &Path,
        id: i64,
        status: &str,
        by: Option<i64>,
    ) -> Result<bool, String> {
        if !STATUSES.contains(&status) {
            return Err(tr!(
                "estado de memoria desconocido \"{status}\" (válidos: {p0})",
                p0 = STATUSES.join(", "),
                status = status
            ));
        }
        let by = by.filter(|_| status == "superseded");
        self.conn
            .execute(
                "UPDATE memories SET status = ?3, superseded_by = ?4 WHERE id = ?1 AND project = ?2",
                params![id, self.project_key(project), status, by],
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())
    }

    pub fn count_memories(&self, project: &Path) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE project = ?1",
                params![self.project_key(project)],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }

    pub fn delete_memory(&self, project: &Path, id: i64) -> Result<bool, String> {
        self.conn
            .execute(
                "DELETE FROM memories WHERE id = ?1 AND project = ?2",
                params![id, self.project_key(project)],
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())
    }
}

/// « (reemplazada por #N)» o « (archivada)»; nada si está vigente.
pub fn status_note(m: &Memory) -> String {
    match (m.status.as_str(), m.superseded_by) {
        ("superseded", Some(by)) => tr!(" (reemplazada por #{by})", by = by),
        ("superseded", None) => tr!(" (reemplazada)").into(),
        ("archived", _) => tr!(" (archivada)").into(),
        _ => String::new(),
    }
}

/// Texto de una memoria para un agente o la terminal.
pub fn render(m: &Memory) -> String {
    let tags = if m.tags.is_empty() {
        String::new()
    } else {
        tr!(" · etiquetas: {p0}", p0 = m.tags)
    };
    tr!(
        "#{p0} [{p1}] {p2}\n{p3}\n(guardada por {p4}, {p5}{tags})",
        p0 = m.id,
        p1 = kind_label(&m.kind),
        p2 = format!("{}{}", m.title, status_note(m)),
        p3 = m.body,
        p4 = m.source,
        p5 = crate::store::ago_precise(m.created_at),
        tags = tags
    )
}

/// Resultado breve de búsqueda: el detalle se obtiene con `memory_get`.
pub fn render_search(m: &Memory) -> String {
    let snippet = m.snippet.as_deref().unwrap_or(&m.body);
    tr!(
        "#{p0} [{p1}] {p2}\n{p3}",
        p0 = m.id,
        p1 = kind_label(&m.kind),
        p2 = format!("{}{}", m.title, status_note(m)),
        p3 = snippet
    )
}

/// Referencia mínima para inyectar en el contexto de un agente.
pub fn render_title(m: &Memory) -> String {
    format!("#{} [{}] {}", m.id, kind_label(&m.kind), m.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_skip_the_session_note_and_loose_matches() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/pistas");
        store
            .add_memory(
                p,
                "decision",
                "Atajos de PC",
                "Cmd es Ctrl+Shift fuera de macOS",
                "teclado",
                "x",
            )
            .unwrap();
        store
            .add_memory(
                p,
                "error",
                "Guard en Windows",
                "El commit tardaba por los shims del proyecto",
                "",
                "x",
            )
            .unwrap();
        // La nota de sesión es larga y comparte palabras con casi cualquier mensaje.
        store
            .save_session_summary(
                p,
                "Hicimos commit de los cambios del proyecto, revisamos los atajos y el panel de memoria.",
                Some("Claude Code"),
            )
            .unwrap();
        let titles = |text: &str| {
            store
                .recall_memories(p, text, 3)
                .unwrap()
                .into_iter()
                .map(|m| m.title)
                .collect::<Vec<_>>()
        };
        // Nunca la nota de sesión, aunque coincida.
        assert!(
            titles("haz commit de los cambios del proyecto")
                .iter()
                .all(|t| !t.contains("sesión")),
        );
        // Una palabra suelta en el cuerpo no basta…
        assert!(titles("los shims tardan").is_empty());
        // …una del título sí, y dos del cuerpo también.
        assert_eq!(titles("¿qué atajos usamos?"), ["Atajos de PC"]);
        assert_eq!(
            titles("el commit va lento por los shims"),
            ["Guard en Windows"]
        );
    }

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
        // Basta una palabra; la que tiene las dos va primero.
        assert_eq!(
            store.search_memories(p, "vacun lote", None, 10).unwrap()[0].title,
            "Vacunación en lotes"
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
        // El resumen de sesión es una sola nota que se actualiza.
        let first = store
            .save_session_summary(p, "Hecho: A. Pendiente: B.", None)
            .unwrap();
        let again = store
            .save_session_summary(p, "Hecho: A y B. Pendiente: C.", None)
            .unwrap();
        assert_eq!(first, again);
        assert!(
            store
                .get_memory(p, first)
                .unwrap()
                .unwrap()
                .body
                .contains("Pendiente: C")
        );
        store.delete_memory(p, first).unwrap();

        // Un mensaje libre encuentra lo relacionado aunque no estén todas sus palabras.
        let recalled = store
            .recall_memories(p, "¿Qué pasa con la vacunación de los lotes?", 3)
            .unwrap();
        assert_eq!(recalled[0].title, "Vacunación en lotes");
        assert!(
            store
                .recall_memories(p, "arregla el login", 3)
                .unwrap()
                .is_empty()
        );
        assert!(store.recall_memories(p, "¿y eso?", 3).unwrap().is_empty());
        let memory = store.get_memory(p, hits[0].id).unwrap().unwrap();
        assert_eq!(memory.title, "Vacunación en lotes");
        assert!(store.get_memory(other, memory.id).unwrap().is_none());

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

    #[test]
    fn secrets_never_reach_the_memory_and_each_agent_keeps_its_session() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/secretos");
        let token = format!("ghp_{}", "a".repeat(36));
        let id = store
            .add_memory(p, "note", "Deploy", &format!("usa {token}"), "", "x")
            .unwrap();
        let body = |id| store.get_memory(p, id).unwrap().unwrap().body;
        assert!(!body(id).contains(&token) && body(id).contains("[SECRETO OCULTO]"));
        store
            .update_memory(p, id, None, None, Some(&format!("otra {token}")), None)
            .unwrap();
        assert!(!body(id).contains(&token), "{}", body(id));
        let s = store
            .save_session_summary(p, &format!("pegó {token}"), Some("Claude Code"))
            .unwrap();
        assert!(!body(s).contains(&token));
        // La nota de antes (sin agente) la adopta el primer resumen por agente.
        let old = store.save_session_summary(p, "antigua", None).unwrap();
        let adopted = store
            .save_session_summary(p, "nueva", Some("Gemini"))
            .unwrap();
        assert_eq!(old, adopted);
        // Una nota por agente: la de Codex no pisa la de Claude.
        let c = store
            .save_session_summary(p, "Codex hizo X", Some("Codex"))
            .unwrap();
        assert_ne!(s, c);
        assert!(body(s).contains("pegó"));
    }

    #[test]
    fn memories_follow_the_project_when_its_remote_changes() {
        // Renombrar el repo (o moverlo de organización) cambia el remoto: la memoria y los
        // planes no se quedan huérfanos bajo la identidad vieja.
        let tmp = std::env::temp_dir().join(format!("forge-relink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "git@github.com:Org/Viejo.git"]);
        let db = tmp.join("forge.db");
        let id = Store::open(&db)
            .unwrap()
            .add_memory(&repo, "decision", "Usar Postgres", "", "", "x")
            .unwrap();
        // Otro clon del mismo repo, que se queda en el remoto viejo.
        let other = tmp.join("otro");
        std::fs::create_dir_all(&other).unwrap();
        let git_in = |dir: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git_in(&other, &["init", "-q"]);
        git_in(
            &other,
            &["remote", "add", "origin", "git@github.com:Org/Viejo.git"],
        );
        let shared = Store::open(&db)
            .unwrap()
            .add_memory(&other, "note", "Compartida", "", "", "x")
            .unwrap();
        let renamed = [
            "remote",
            "set-url",
            "origin",
            "git@github.com:Org/Nuevo.git",
        ];
        git(&renamed);
        let store = Store::open(&db).unwrap();
        assert!(
            store.get_memory(&repo, id).unwrap().is_none(),
            "con otro clon en el remoto viejo, la memoria no se muda"
        );
        assert!(store.get_memory(&other, shared).unwrap().is_some());
        // El otro clon cambia también: ya nadie usa el viejo y todo se muda.
        git_in(&other, &renamed);
        let store = Store::open(&db).unwrap();
        assert!(store.get_memory(&other, shared).unwrap().is_some());
        assert!(
            store.get_memory(&repo, id).unwrap().is_some(),
            "la memoria se quedó con el remoto viejo"
        );
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn memories_follow_the_git_remote_not_the_folder() {
        let tmp = std::env::temp_dir().join(format!("forge-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let git = |dir: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        let a = tmp.join("a");
        std::fs::create_dir_all(&a).unwrap();
        git(&a, &["init", "-q"]);
        git(
            &a,
            &["remote", "add", "origin", "git@github.com:Org/Repo.git"],
        );
        git(
            &a,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        );

        // Lo guardado antes con la ruta pasa a la clave del remoto.
        let store = Store::in_memory().unwrap();
        store
            .conn
            .execute(
                "INSERT INTO memories (project, kind, title, body, source, created_at) VALUES (?1, 'decision', 'Vieja', '', 'u', 1)",
                [a.to_string_lossy()],
            )
            .unwrap();
        store.add_memory(&a, "note", "Nueva", "", "", "u").unwrap();
        assert_eq!(store.project_key(&a), "git:github.com/org/repo");
        assert_eq!(store.count_memories(&a).unwrap(), 2);

        // Carpeta movida, otro clon (con https) y un worktree: la misma memoria.
        let moved = tmp.join("movida");
        std::fs::rename(&a, &moved).unwrap();
        let clone = tmp.join("clon");
        std::fs::create_dir_all(&clone).unwrap();
        git(&clone, &["init", "-q"]);
        git(
            &clone,
            &["remote", "add", "origin", "https://github.com/org/repo"],
        );
        let worktree = tmp.join("wt");
        git(
            &moved,
            &["worktree", "add", "-q", worktree.to_str().unwrap()],
        );
        for dir in [&moved, &clone, &worktree] {
            assert_eq!(store.count_memories(dir).unwrap(), 2, "{dir:?}");
        }
        // Una subcarpeta del repo es otro proyecto.
        let sub = moved.join("api");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(store.project_key(&sub), "git:github.com/org/repo/api");
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn words_shared_by_much_of_the_project_do_not_count() {
        // En el proyecto Forge casi todo dice «Forge»: esa palabra no dice de qué trata nada.
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/comunes");
        for title in [
            "Forge Guard en Windows",
            "Forge Memory por remoto",
            "Atajos de Forge",
            "Forge Jev mide modelos",
            "Sonidos de Forge",
        ] {
            store
                .add_memory(p, "decision", title, "x", "", "x")
                .unwrap();
        }
        let titles = |text: &str| {
            store
                .recall_memories(p, text, 2)
                .unwrap()
                .into_iter()
                .map(|m| m.title)
                .collect::<Vec<_>>()
        };
        assert!(titles("siento que forge no sirve").is_empty());
        assert_eq!(titles("los atajos de forge"), ["Atajos de Forge"]);
    }

    #[test]
    fn replaced_and_archived_memories_stay_searchable_but_are_not_suggested() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/estados");
        let old = store
            .add_memory(p, "decision", "Atajos de PC", "Ctrl", "", "x")
            .unwrap();
        let new = store
            .add_memory(p, "decision", "Atajos en Linux", "Ctrl+Shift", "", "x")
            .unwrap();
        let archived = store
            .add_memory(p, "decision", "Sonidos con afplay", "x", "", "x")
            .unwrap();
        assert!(
            store
                .set_memory_status(p, old, "superseded", Some(new))
                .unwrap()
        );
        assert!(
            store
                .set_memory_status(p, archived, "archived", None)
                .unwrap()
        );
        assert!(store.set_memory_status(p, old, "borrada", None).is_err());
        let recalled = |text: &str| {
            store
                .recall_memories(p, text, 3)
                .unwrap()
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(recalled("¿qué atajos usamos?"), [new]);
        assert!(recalled("los sonidos con afplay").is_empty());
        // Buscando a propósito siguen ahí, con su estado.
        let found = store.search_memories(p, "atajos", None, 10).unwrap();
        let replaced = found.iter().find(|m| m.id == old).unwrap();
        assert_eq!(replaced.status, "superseded");
        assert_eq!(replaced.superseded_by, Some(new));
        // Restaurada vuelve a sugerirse.
        store
            .set_memory_status(p, archived, "active", None)
            .unwrap();
        assert_eq!(recalled("los sonidos con afplay"), [archived]);
    }

    #[test]
    fn search_needs_only_some_of_the_words() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/alguna");
        let id = store
            .add_memory(p, "decision", "Vacunación en lotes", "x", "", "x")
            .unwrap();
        store
            .add_memory(p, "command", "Levantar la base de datos", "x", "", "x")
            .unwrap();
        // Las palabras de relleno («de», «la», «como») no traen todo lo demás.
        let found = store
            .search_memories(p, "como se hace de la vacunación del ganado", None, 10)
            .unwrap();
        assert_eq!(found.iter().map(|m| m.id).collect::<Vec<_>>(), [id]);
    }

    #[test]
    fn old_databases_keep_their_memories_as_active() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL, kind TEXT NOT NULL,
                title TEXT NOT NULL, body TEXT NOT NULL, tags TEXT NOT NULL DEFAULT '',
                source TEXT NOT NULL, created_at INTEGER NOT NULL);
             INSERT INTO memories (project, kind, title, body, source, created_at)
                VALUES ('p', 'decision', 'vieja', 'x', 'u', 1);",
        )
        .unwrap();
        init(&conn).unwrap();
        init(&conn).unwrap();
        let (status, by): (String, Option<i64>) = conn
            .query_row("SELECT status, superseded_by FROM memories", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((status.as_str(), by), ("active", None));
        // A medio migrar (otro proceso se cortó entre las dos columnas): se completa.
        let half = Connection::open_in_memory().unwrap();
        half.execute_batch(
            "CREATE TABLE memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL, kind TEXT NOT NULL,
                title TEXT NOT NULL, body TEXT NOT NULL, tags TEXT NOT NULL DEFAULT '',
                source TEXT NOT NULL, created_at INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'active');",
        )
        .unwrap();
        init(&half).unwrap();
        assert!(half.prepare("SELECT superseded_by FROM memories").is_ok());
    }

    #[test]
    fn finds_similar_memories_and_updates_them() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/x");
        let id = store
            .add_memory(p, "decision", "Pagos con Wompi", "Por PSE.", "", "u")
            .unwrap();
        // Mismo tema con otras palabras de relleno y tildes: es la misma.
        let hit = store.similar_memory(p, "pagos con WOMPI y PSE").unwrap();
        assert_eq!(hit.map(|m| m.id), Some(id));
        assert!(
            store
                .similar_memory(p, "Pagos con tarjeta")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .similar_memory(Path::new("/otro"), "Pagos con Wompi")
                .unwrap()
                .is_none()
        );

        assert!(
            store
                .update_memory(p, id, None, None, Some("Por PSE y Nequi."), None)
                .unwrap()
        );
        let m = &store.search_memories(p, "nequi", None, 5).unwrap()[0];
        assert_eq!((m.id, m.title.as_str()), (id, "Pagos con Wompi"));
        assert!(
            store.search_memories(p, "pse", None, 5).unwrap().len() == 1,
            "el índice no queda con la versión vieja"
        );
        assert!(
            !store
                .update_memory(Path::new("/otro"), id, None, None, Some("x"), None)
                .unwrap()
        );
        assert!(
            store
                .update_memory(p, id, Some("nada"), None, None, None)
                .is_err()
        );
        assert!(
            store
                .update_memory(p, id, None, Some(" "), None, None)
                .is_err()
        );
    }
}
