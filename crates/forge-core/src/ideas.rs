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
    /// Punta de la rama cuando se le asignó a la fase; si no se movió, la fase no se
    /// cierra aunque el contenido ya esté en main (puede venir de otra rama ya fusionada).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_tip: Option<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// Agente que empezó la fase (el nombre del cliente MCP que la marcó en curso, p. ej.
    /// «claude-code»): las correcciones de una revisión irán a él.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worked_by: Option<String>,
}

/// El mensaje para el agente que revisa la fase `index` de un plan (texto para agentes, en
/// inglés). Solo lectura: quien corrige es el agente que hizo la fase. `None` si la fase no
/// existe o no tiene rama.
pub fn review_prompt(idea: &Idea, index: usize) -> Option<String> {
    let phase = idea.phases.get(index)?;
    let branch = phase.branch.as_deref()?;
    let tasks: String = phase
        .tasks
        .iter()
        .map(|t| format!("- [{}] {}\n", if t.done { "x" } else { " " }, t.text))
        .collect();
    let notes = if phase.notes.is_empty() {
        String::new()
    } else {
        format!("Phase notes: {}\n", phase.notes)
    };
    Some(format!(
        "You are a code reviewer. Review phase {n} (\"{title}\") of plan #{id} (\"{plan}\") in Forge's plans, on branch `{branch}`.\n\
         {notes}Tasks of the phase:\n{tasks}\n\
         This review is READ-ONLY. Do NOT edit files, and do NOT run git checkout, switch, stash, reset, commit or anything else that changes the working tree or the branch: another agent may be working in this folder. Read the changes with `git diff main...{branch}` and `git log main..{branch}` (use the repository's default branch if it is not main), and read files with your normal read tools.\n\
         Check, in this order: 1) bugs and wrong behavior; 2) every task above is really done; 3) tests that cannot fail or are missing; 4) the project's conventions (read them with project_context and memory_search from Forge's MCP tools). List your findings ordered by severity; for each give the file and line, what is wrong and why. If you find nothing, say so. Do not fix anything: the agent that wrote the phase will fix what you report.",
        n = index + 1,
        title = phase.title,
        id = idea.id,
        plan = idea.title,
    ))
}

fn pending() -> String {
    "pending".into()
}

impl Phase {
    /// ¿Se puede pedir una revisión? Tiene rama (con qué comparar), está en curso y le queda
    /// como mucho una tarea. Las fases hechas ya están en main y las pendientes no empezaron.
    pub fn reviewable(&self) -> bool {
        let left = self.tasks.iter().filter(|t| !t.done).count();
        self.branch.is_some() && self.status == "doing" && left <= 1
    }

    pub fn new(title: &str) -> Self {
        Phase {
            title: title.trim().to_string(),
            ..Default::default()
        }
    }
}

impl Default for Phase {
    fn default() -> Self {
        Phase {
            title: String::new(),
            notes: String::new(),
            status: pending(),
            branch: None,
            branch_tip: None,
            tasks: Vec::new(),
            worked_by: None,
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

/// En qué se está trabajando, en una línea (~50 tokens): las fases en curso y sus
/// próximas tareas. Va al final del contexto en cada turno, donde el modelo mejor
/// atiende («lost in the middle»). En inglés: es para el agente.
pub fn anchor(ideas: &[Idea]) -> Option<String> {
    let clip = |s: &str| -> String {
        let s = s.trim();
        match s.char_indices().nth(80) {
            Some((i, _)) => format!("{}…", &s[..i]),
            None => s.to_string(),
        }
    };
    let lines: Vec<String> = ideas
        .iter()
        .filter(|i| !i.done())
        .filter_map(|i| {
            let (n, phase) = i
                .phases
                .iter()
                .enumerate()
                .find(|(_, p)| p.status == "doing")?;
            let next: Vec<String> = phase
                .tasks
                .iter()
                .filter(|t| !t.done)
                .take(3)
                .map(|t| clip(&t.text))
                .collect();
            let mut line = format!("plan #{} phase {} \"{}\"", i.id, n + 1, clip(&phase.title));
            if !next.is_empty() {
                line.push_str(&format!(" (next: {})", next.join("; ")));
            }
            Some(line)
        })
        .take(2)
        .collect();
    (!lines.is_empty()).then(|| format!("Working on {}.", lines.join(" and ")))
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

fn check_status(status: &str) -> Result<&'static str, String> {
    normalize_status(status).ok_or_else(|| {
        tr!(
            "estado desconocido \"{status}\" (válidos: backlog, in_progress, done)",
            status = status
        )
    })
}

impl Store {
    /// '' = lista general.
    fn list_key(&self, project: Option<&Path>) -> String {
        project.map_or(String::new(), |p| self.project_key(p))
    }

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
                params![self.list_key(project), title.trim(), note.trim(), status, source, t, to_json(&phases)],
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

    /// Cierra las fases abiertas cuya rama ya está en la rama principal del repo `project`
    /// (squash, merge o rebase): fase hecha, tareas tachadas y una nota. Devuelve cuáles.
    pub fn close_merged(&self, project: &Path) -> Result<Vec<String>, String> {
        let mut closed = Vec::new();
        let mut base = None;
        // Incluye los planes terminados: sus puntas también cuentan.
        let ideas = self.list_ideas(Some(project), true)?;
        // Puntas con las que ya se cerró alguna fase del proyecto: otra rama que sigue
        // en una de ellas se creó desde esa fase y aún no tiene commits propios.
        let mut taken: std::collections::HashMap<String, String> = ideas
            .iter()
            .flat_map(|i| &i.phases)
            .filter_map(|p| Some((p.branch_tip.clone()?, p.branch.clone()?)))
            .collect();
        for idea in ideas.into_iter().filter(|i| i.status != "done") {
            let mut phases = idea.phases;
            let mut changed = false;
            for n in 0..phases.len() {
                let Some(branch) = phases[n]
                    .branch
                    .clone()
                    .filter(|_| phases[n].status != "done")
                else {
                    continue;
                };
                // La rama principal se busca una vez, y solo si hay alguna fase con rama.
                let Some(base) = base
                    .get_or_insert_with(|| crate::git::main_branch(project))
                    .as_deref()
                else {
                    return Ok(closed);
                };
                let Some(tip) = crate::git::resolve_commit(project, &branch) else {
                    continue;
                };
                if crate::git::branch_merged_into(project, base, &branch) != Some(true) {
                    continue;
                }
                // Lo que hay en main vino de otra rama (p. ej. una ya fusionada con squash):
                // la punta es la de una fase ya cerrada o la de una fase anterior del plan.
                let from_earlier_phase = phases[..n].iter().any(|prev| {
                    prev.branch
                        .as_deref()
                        .and_then(|b| crate::git::resolve_commit(project, b))
                        .as_deref()
                        == Some(&tip)
                });
                if taken.get(&tip).is_some_and(|b| *b != branch) || from_earlier_phase {
                    continue;
                }
                taken.insert(tip.clone(), branch.clone());
                let phase = &mut phases[n];
                phase.status = "done".into();
                phase.branch_tip = Some(tip);
                phase.tasks.iter_mut().for_each(|t| t.done = true);
                let note = format!("Closed by Forge: branch {branch} is in main.");
                phase.notes = [phase.notes.trim(), &note].join("\n").trim().to_string();
                closed.push(format!("#{} phase {} ({branch})", idea.id, n + 1));
                changed = true;
            }
            if changed {
                self.set_phases(Some(project), idea.id, &phases, "forge")?;
            }
        }
        Ok(closed)
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
        let mut phases = clean_phases(phases)?;
        for (i, phase) in phases.iter_mut().enumerate() {
            // Quién trabajó la fase no se pierde al reescribir el plan.
            if phase.worked_by.is_none() {
                phase.worked_by = idea.phases.get(i).and_then(|prev| prev.worked_by.clone());
            }
            if let (None, Some(branch)) = (&phase.branch_tip, &phase.branch) {
                phase.branch_tip = idea
                    .phases
                    .get(i)
                    .filter(|prev| prev.branch.as_deref() == Some(branch))
                    .and_then(|prev| prev.branch_tip.clone())
                    .or_else(|| {
                        idea.phases
                            .iter()
                            .find(|prev| prev.branch.as_deref() == Some(branch))
                            .and_then(|prev| prev.branch_tip.clone())
                    });
            }
        }
        let status = derived_status(&phases, &idea.status);
        let changed = self
            .conn
            .execute(
                "UPDATE ideas SET phases = ?3, status = ?4, updated_by = ?5, updated_at = ?6
                 WHERE project = ?1 AND id = ?2",
                params![
                    self.list_key(project),
                    id,
                    to_json(&phases),
                    status,
                    by,
                    now()
                ],
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
            if phase.status == "doing" && !by.trim().is_empty() {
                phase.worked_by = Some(by.trim().to_string());
            }
        }
        if let Some(branch) = change.branch {
            let branch = branch.trim();
            if branch.is_empty() {
                phase.branch = None;
                phase.branch_tip = None;
            } else {
                // La punta guardada es la de esta rama al cerrarse: con otra rama no aplica.
                if phase.branch.as_deref() != Some(branch) {
                    phase.branch_tip = None;
                }
                phase.branch = Some(branch.to_string());
            }
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

    /// Abiertas primero (en curso, luego pendientes, de la más nueva a la más antigua);
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
                          CASE WHEN status = 'done' THEN -updated_at ELSE -created_at END, id DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![self.list_key(project), include_done], |r| {
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
                params![self.list_key(project)],
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
                    self.list_key(project),
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
                params![self.list_key(project), id],
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
                // Si la rama cambió, la punta guardada ya no aplica.
                branch_tip: branch
                    .filter(|b| p.branch.as_deref() == Some(b))
                    .and(p.branch_tip.clone()),
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
                worked_by: p.worked_by.clone().filter(|w| !w.trim().is_empty()),
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
    use std::process::Command;

    #[test]
    fn phases_whose_branch_reached_main_close_themselves() {
        let dir = crate::git::test_repo(
            "close-merged",
            "git switch -qc feat/uno && echo 2 > b && git add -A && git commit -qm uno \
             && git switch -q main && git merge -q --squash feat/uno && git commit -qm 'uno (#1)' \
             && git switch -qc feat/dos && echo 3 > c && git add -A && git commit -qm dos",
        );
        let store = Store::in_memory().unwrap();
        let p = Some(dir.as_path());
        let id = store.add_idea(p, "Plan", "", "claude-code").unwrap();
        let phase = |title: &str, branch: &str| Phase {
            title: title.into(),
            notes: String::new(),
            status: "doing".into(),
            branch: Some(branch.into()),
            tasks: vec![Task {
                text: "t".into(),
                done: false,
            }],
            ..Default::default()
        };
        store
            .set_phases(
                p,
                id,
                &[phase("Uno", "feat/uno"), phase("Dos", "feat/dos")],
                "claude-code",
            )
            .unwrap();

        let closed = store.close_merged(&dir).unwrap();
        assert_eq!(closed.len(), 1, "{closed:?}");
        let plan = store.get_idea(p, id).unwrap().unwrap();
        let (uno, dos) = (&plan.phases[0], &plan.phases[1]);
        assert!(uno.status == "done" && uno.tasks[0].done && uno.notes.contains("feat/uno"));
        assert!(
            dos.status == "doing" && !dos.tasks[0].done,
            "la que no se fusionó sigue"
        );
        assert_eq!(plan.status, "doing");
        assert!(store.close_merged(&dir).unwrap().is_empty(), "una sola vez");
    }

    #[test]
    fn phase_with_branch_from_squashed_parent_does_not_close_without_own_commits() {
        // Reproduce el caso del plan #70 fase 2: feat/dos se creó desde la punta de feat/uno,
        // que ya se había fusionado con squash en main. Sin commits propios, no debe cerrarse.
        let dir = crate::git::test_repo(
            "close-squashed-parent",
            "git switch -qc feat/uno && echo 2 > b && git add -A && git commit -qm uno \
             && git switch -q main && git merge -q --squash feat/uno && git commit -qm 'uno (#1)' \
             && git switch -qc feat/dos feat/uno",
        );
        let store = Store::in_memory().unwrap();
        let p = Some(dir.as_path());
        let id = store.add_idea(p, "Plan", "", "claude-code").unwrap();
        let phase = |title: &str, branch: &str| Phase {
            title: title.into(),
            notes: String::new(),
            status: "doing".into(),
            branch: Some(branch.into()),
            tasks: vec![Task {
                text: "t".into(),
                done: false,
            }],
            ..Default::default()
        };
        let mut dos_phase = phase("Dos", "");
        dos_phase.branch = None;
        store
            .set_phases(p, id, &[phase("Uno", "feat/uno"), dos_phase], "claude-code")
            .unwrap();
        // Al empezar la fase 2, se le asigna feat/dos (recién creada desde feat/uno):
        store
            .update_phase(
                p,
                id,
                1,
                PhaseChange {
                    status: Some("doing"),
                    branch: Some("feat/dos"),
                    ..Default::default()
                },
                "claude-code",
            )
            .unwrap();

        // Al verificar, solo feat/uno debe cerrarse; feat/dos no tiene commits propios.
        let closed = store.close_merged(&dir).unwrap();
        assert_eq!(closed, vec![format!("#{id} phase 1 (feat/uno)")]);
        let plan = store.get_idea(p, id).unwrap().unwrap();
        assert_eq!(plan.phases[0].status, "done");
        assert_eq!(plan.phases[1].status, "doing", "feat/dos no debe cerrarse");
        assert!(!plan.phases[1].tasks[0].done);

        // Ahora feat/dos avanza con un commit propio y se fusiona con squash en main.
        let sh = |c: &str| {
            assert!(
                Command::new("sh")
                    .args(["-c", c])
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success(),
                "{c}"
            );
        };
        sh("git switch -q feat/dos && echo 3 > c && git add -A && git commit -qm dos");
        sh("git switch -q main && git merge -q --squash feat/dos && git commit -qm 'dos (#2)'");

        // Ahora que feat/dos tiene commits propios y llegaron a main, se cierra.
        let closed = store.close_merged(&dir).unwrap();
        assert_eq!(closed, vec![format!("#{id} phase 2 (feat/dos)")]);
        let plan = store.get_idea(p, id).unwrap().unwrap();
        assert_eq!(plan.phases[1].status, "done");
        assert!(plan.phases[1].tasks[0].done);
        assert_eq!(plan.status, "done");

        // Si una fase añade su rama mediante update_phase cuando ya está en main:
        sh("git switch -qc feat/tres feat/uno");
        let id3 = store.add_idea(p, "Plan 3", "", "claude-code").unwrap();
        let phase_no_branch = Phase {
            title: "Tres".into(),
            notes: String::new(),
            status: "doing".into(),
            branch: None,
            branch_tip: None,
            tasks: vec![Task {
                text: "t".into(),
                done: false,
            }],
            worked_by: None,
        };
        store
            .set_phases(p, id3, &[phase_no_branch], "claude-code")
            .unwrap();
        store
            .update_phase(
                p,
                id3,
                0,
                PhaseChange {
                    branch: Some("feat/tres"),
                    ..Default::default()
                },
                "claude-code",
            )
            .unwrap();
        let closed3 = store.close_merged(&dir).unwrap();
        assert!(
            closed3.is_empty(),
            "feat/tres no debe cerrarse tras update_phase sin commits propios"
        );

        // Si el plan se crea con ambas ramas asignadas de entrada (plan_add con fases):
        let id4 = store.add_idea(p, "Plan 4", "", "claude-code").unwrap();
        store
            .set_phases(
                p,
                id4,
                &[phase("Uno", "feat/uno"), phase("Cuatro", "feat/tres")],
                "claude-code",
            )
            .unwrap();
        let closed4 = store.close_merged(&dir).unwrap();
        assert_eq!(
            closed4,
            vec![format!("#{id4} phase 1 (feat/uno)")],
            "solo feat/uno debe cerrarse; feat/tres apunta a una fase anterior"
        );

        // Si un agente reescribe las fases (plan_update phases) sin traer branch_tip,
        // la punta de las fases cerradas se conserva.
        let before = store.get_idea(p, id).unwrap().unwrap();
        assert!(before.phases[0].branch_tip.is_some());
        let incoming: Vec<Phase> = before
            .phases
            .iter()
            .map(|ph| Phase {
                branch_tip: None,
                ..ph.clone()
            })
            .collect();
        store.set_phases(p, id, &incoming, "claude-code").unwrap();
        let after = store.get_idea(p, id).unwrap().unwrap();
        assert_eq!(after.phases[0].branch_tip, before.phases[0].branch_tip);
        assert!(store.close_merged(&dir).unwrap().is_empty());
    }

    #[test]
    fn phase_whose_branch_is_assigned_after_its_commits_still_closes() {
        // Muchos agentes asignan la rama a la fase al final, con los commits ya hechos.
        let dir = crate::git::test_repo(
            "close-assigned-late",
            "git switch -qc feat/x && echo 2 > b && git add -A && git commit -qm x",
        );
        let store = Store::in_memory().unwrap();
        let p = Some(dir.as_path());
        let id = store.add_idea(p, "Plan", "", "claude-code").unwrap();
        store
            .set_phases(p, id, &[Phase::new("X")], "claude-code")
            .unwrap();
        let change = PhaseChange {
            branch: Some("feat/x"),
            ..Default::default()
        };
        store.update_phase(p, id, 0, change, "claude-code").unwrap();
        let merge = "git switch -q main && git merge -q --squash feat/x && git commit -qm 'x (#1)'";
        let ok = Command::new("sh")
            .args(["-c", merge])
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(ok.success());
        let closed = store.close_merged(&dir).unwrap();
        assert_eq!(closed, vec![format!("#{id} phase 1 (feat/x)")]);
    }

    #[test]
    fn anchor_names_the_phase_in_progress_and_its_next_tasks() {
        let store = Store::in_memory().unwrap();
        let p = Path::new("/p/x");
        let phase = |title: &str, tasks: &[&str]| Phase {
            tasks: tasks
                .iter()
                .map(|t| Task {
                    text: t.to_string(),
                    done: false,
                })
                .collect(),
            ..Phase::new(title)
        };
        let id = store
            .add_plan(
                Some(p),
                "Recibos",
                "",
                &[
                    phase("Modelo", &["a"]),
                    phase("Envío", &["Resend", "Plantilla", "Reintentos", "Logs"]),
                ],
                "u",
            )
            .unwrap();
        let anchor = |s: &Store| anchor(&s.list_ideas(Some(p), false).unwrap());
        assert_eq!(anchor(&store), None, "nada en curso: sin ancla");
        let done = PhaseChange {
            status: Some("done"),
            ..Default::default()
        };
        store.update_phase(Some(p), id, 0, done, "u").unwrap();
        let doing = PhaseChange {
            status: Some("in_progress"),
            task: Some((0, true)),
            ..Default::default()
        };
        store.update_phase(Some(p), id, 1, doing, "u").unwrap();
        assert_eq!(
            anchor(&store).unwrap(),
            format!("Working on plan #{id} phase 2 \"Envío\" (next: Plantilla; Reintentos; Logs).")
        );
    }

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

        // Pendientes: la más nueva arriba.
        let list = store.list_ideas(Some(bovinapp), true).unwrap();
        assert_eq!(list.iter().map(|i| i.id).collect::<Vec<_>>(), vec![b, a]);

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
    fn only_a_phase_with_a_branch_that_is_nearly_done_can_be_reviewed() {
        let task = |done: bool| Task {
            text: "t".into(),
            done,
        };
        let mut p = Phase::new("Fase");
        // Sin rama no hay nada que comparar con main.
        p.status = "doing".into();
        assert!(!p.reviewable());
        p.branch = Some("feat/x".into());
        // En curso y sin tareas: se puede revisar; con varias tareas por hacer, aún no.
        assert!(p.reviewable());
        p.tasks = vec![task(true), task(false), task(false)];
        assert!(!p.reviewable());
        // Queda una tarea o ninguna: sí.
        p.tasks[1].done = true;
        assert!(p.reviewable());
        p.tasks[2].done = true;
        assert!(p.reviewable());
        // Pendiente (sin empezar) no; hecha (ya en main) tampoco.
        p.status = "pending".into();
        assert!(!p.reviewable());
        p.status = "done".into();
        assert!(!p.reviewable());
    }

    #[test]
    fn the_review_prompt_asks_for_a_read_only_review_of_the_phase_branch() {
        let store = Store::in_memory().unwrap();
        let p = Some(Path::new("/p/forge"));
        let mut phase = Phase::new("Botón Revisar");
        phase.branch = Some("feat/plans-review-button".into());
        phase.notes = "El revisor no toca el árbol de trabajo.".into();
        phase.tasks = vec![
            Task {
                text: "Botón en la fase".into(),
                done: true,
            },
            Task {
                text: "Menú de agentes".into(),
                done: false,
            },
        ];
        let id = store
            .add_plan(p, "Revisión de fases", "", &[phase], "claude-code")
            .unwrap();
        let plan = store.get_idea(p, id).unwrap().unwrap();
        let prompt = review_prompt(&plan, 0).unwrap();
        assert!(prompt.contains("feat/plans-review-button"));
        assert!(prompt.contains("git diff"));
        assert!(prompt.contains("- [x] Botón en la fase"));
        assert!(prompt.contains("- [ ] Menú de agentes"));
        assert!(prompt.contains("El revisor no toca el árbol de trabajo."));
        // Solo lectura, dicho sin rodeos.
        assert!(prompt.contains("Do NOT edit files"));
        assert!(prompt.contains("checkout"));
        // Una fase sin rama no se puede revisar.
        let id2 = store
            .add_plan(p, "Otro", "", &[Phase::new("Sin rama")], "claude-code")
            .unwrap();
        let plan2 = store.get_idea(p, id2).unwrap().unwrap();
        assert!(review_prompt(&plan2, 0).is_none());
        assert!(review_prompt(&plan, 5).is_none());
    }

    #[test]
    fn the_agent_that_starts_a_phase_is_recorded_and_survives_a_replace() {
        let store = Store::in_memory().unwrap();
        let p = Some(Path::new("/p/forge"));
        let id = store
            .add_plan(
                p,
                "Plan",
                "",
                &[Phase::new("Uno"), Phase::new("Dos")],
                "claude-code",
            )
            .unwrap();
        let start = PhaseChange {
            status: Some("in_progress"),
            ..Default::default()
        };
        store.update_phase(p, id, 1, start, "codex").unwrap();
        let plan = store.get_idea(p, id).unwrap().unwrap();
        assert_eq!(plan.phases[0].worked_by, None);
        assert_eq!(plan.phases[1].worked_by.as_deref(), Some("codex"));
        // Tachar una tarea (otro agente) no cambia quién empezó la fase.
        let tick = PhaseChange {
            add_task: Some("Algo"),
            ..Default::default()
        };
        store.update_phase(p, id, 1, tick, "opencode").unwrap();
        let plan = store.get_idea(p, id).unwrap().unwrap();
        assert_eq!(plan.phases[1].worked_by.as_deref(), Some("codex"));
        // Reemplazar las fases (el agente reescribe el plan) conserva quién trabajó cada una.
        store
            .set_phases(
                p,
                id,
                &[Phase::new("Uno"), Phase::new("Dos")],
                "claude-code",
            )
            .unwrap();
        let plan = store.get_idea(p, id).unwrap().unwrap();
        assert_eq!(plan.phases[1].worked_by.as_deref(), Some("codex"));
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
