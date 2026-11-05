// Lanzar varias fases de un plan en paralelo: una terminal por fase, cada una en su worktree.
// Aquí lo que no depende de la interfaz: qué fases se pueden lanzar (y por qué no), los
// avisos cuando se pisan (mismos archivos, una nombra a otra) y el mensaje inicial de cada
// agente (en inglés: es para el agente, sea cual sea).
use crate::ideas::{Idea, Phase};
use crate::tr;
use std::path::Path;

/// Lo que conviene saber antes de lanzar (no bloquea).
#[derive(Clone, Debug, PartialEq)]
pub enum Warning {
    /// Dos fases nombran los mismos archivos o carpetas (números desde 1).
    SharedFiles {
        phases: (usize, usize),
        files: Vec<String>,
    },
    /// El texto de `phase` nombra a `other` (puede depender de ella). Números desde 1.
    Mentions { phase: usize, other: usize },
}

impl Warning {
    pub fn text(&self) -> String {
        match self {
            Warning::SharedFiles { phases, files } => tr!(
                "Las fases {a} y {b} nombran los mismos archivos ({f}): pueden pisarse.",
                a = phases.0,
                b = phases.1,
                f = files.join(", ")
            ),
            Warning::Mentions { phase, other } => tr!(
                "La fase {a} nombra a la fase {b}: puede depender de ella.",
                a = phase,
                b = other
            ),
        }
    }
}

/// Por qué la fase `index` (desde 0) no se puede lanzar en paralelo; `None` si se puede.
pub fn refusal(phase: &Phase, index: usize) -> Option<String> {
    let n = index + 1;
    match phase.status.as_str() {
        "done" => return Some(tr!("La fase {n} ya está completada.", n = n)),
        "doing" => {
            return Some(tr!(
                "La fase {n} ya está en curso: ya hay alguien trabajando en ella.",
                n = n
            ));
        }
        _ => {}
    }
    if phase.branch.as_deref().is_none_or(|b| b.trim().is_empty()) {
        return Some(tr!(
            "La fase {n} no tiene rama: cada fase en paralelo trabaja en su propio worktree, que sale de su rama. Ponle una rama y vuelve a lanzarla.",
            n = n
        ));
    }
    None
}

/// ¿Se pueden lanzar juntas las fases `phases` (índices desde 0)? Los avisos, o el motivo
/// por el que no (todas las fases que fallan, no solo la primera).
pub fn check(idea: &Idea, phases: &[usize]) -> Result<Vec<Warning>, String> {
    let mut chosen: Vec<usize> = phases.to_vec();
    chosen.sort_unstable();
    chosen.dedup();
    if chosen.len() < 2 {
        return Err(tr!("Elige al menos dos fases para lanzarlas en paralelo.").into());
    }
    let mut reasons = Vec::new();
    for &i in &chosen {
        match idea.phases.get(i) {
            None => reasons.push(tr!(
                "El plan #{id} no tiene fase {n}.",
                id = idea.id,
                n = i + 1
            )),
            Some(p) => reasons.extend(refusal(p, i)),
        }
    }
    for (k, &a) in chosen.iter().enumerate() {
        for &b in &chosen[k + 1..] {
            let branch = |i: usize| idea.phases.get(i).and_then(|p| p.branch.as_deref());
            if let (Some(x), Some(y)) = (branch(a), branch(b))
                && x.trim() == y.trim()
            {
                reasons.push(tr!(
                    "Las fases {a} y {b} tienen la misma rama ({r}): cada una necesita la suya.",
                    a = a + 1,
                    b = b + 1,
                    r = x.trim()
                ));
            }
        }
    }
    if !reasons.is_empty() {
        return Err(reasons.join("\n"));
    }
    Ok(warnings(idea, &chosen))
}

/// Mismos archivos entre cada par de fases y fases que nombran a otra de las elegidas.
fn warnings(idea: &Idea, chosen: &[usize]) -> Vec<Warning> {
    let mut out = Vec::new();
    let files: Vec<Vec<String>> = chosen
        .iter()
        .map(|&i| files_named(&idea.phases[i]))
        .collect();
    for (k, &a) in chosen.iter().enumerate() {
        for (j, &b) in chosen.iter().enumerate().skip(k + 1) {
            let shared: Vec<String> = files[k]
                .iter()
                .filter(|f| files[j].iter().any(|g| overlap(f, g)))
                .cloned()
                .collect();
            if !shared.is_empty() {
                out.push(Warning::SharedFiles {
                    phases: (a + 1, b + 1),
                    files: shared,
                });
            }
        }
    }
    for &a in chosen {
        let named = phase_numbers(&text_of(&idea.phases[a]));
        for &b in chosen {
            if a != b && named.contains(&(b + 1)) {
                out.push(Warning::Mentions {
                    phase: a + 1,
                    other: b + 1,
                });
            }
        }
    }
    out
}

/// Título, notas y tareas de la fase.
fn text_of(phase: &Phase) -> String {
    let mut text = format!("{}\n{}", phase.title, phase.notes);
    for t in &phase.tasks {
        text.push('\n');
        text.push_str(&t.text);
    }
    text
}

/// El mismo archivo (sin mirar mayúsculas ni `./`; sin carpeta, por su nombre), o uno
/// dentro de una carpeta nombrada (`src/app/` y `src/app/x.rs`).
fn overlap(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim_start_matches("./").to_lowercase();
    let (a, b) = (norm(a), norm(b));
    let name = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    a == b
        || (a.ends_with('/') && b.starts_with(&a))
        || (b.ends_with('/') && a.starts_with(&b))
        || (!a.contains('/') && !b.ends_with('/') && name(&b) == a)
        || (!b.contains('/') && !a.ends_with('/') && name(&a) == b)
}

/// Extensiones de archivo habituales: `self.error` o `ws.ideas` son código, no archivos.
const EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "json", "toml", "yaml", "yml", "md", "py", "go",
    "rb", "java", "kt", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "css", "scss", "html", "vue",
    "svelte", "sh", "ps1", "sql", "lock", "txt", "xml", "proto", "gradle",
];

/// Archivos y carpetas que nombra la fase (en sus notas o tareas): palabras con extensión
/// (`ideas_panel.rs`, `src/cli.rs`) o rutas que acaban en `/` (`src/app/`). Las ramas de
/// la fase no cuentan aunque lleven `/`.
pub fn files_named(phase: &Phase) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text_of(phase).split(|c: char| c.is_whitespace() || ",;()[]{}\"'«»`".contains(c))
    {
        let word = word.trim_end_matches(['.', ':', '!', '?']);
        let word = word.split(':').next().unwrap_or(word); // `src/x.rs:12`
        if word.contains("://") || phase.branch.as_deref() == Some(word) {
            continue;
        }
        if (is_file(word) || is_dir(word)) && !out.iter().any(|f| f == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// `nombre.ext`: extensión de 1 a 5 letras y nombre con algo más que puntos.
fn is_file(word: &str) -> bool {
    let Some((stem, ext)) = word.rsplit_once('.') else {
        return false;
    };
    let name = stem.rsplit('/').next().unwrap_or(stem);
    let path = word.contains('/');
    // «Node.js», «Next.js»: productos, no archivos.
    let product = !path && ext == "js" && name.starts_with(|c: char| c.is_ascii_uppercase());
    (path || EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        && !product
        && (1..=5).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphabetic())
        && name.chars().any(|c| c.is_ascii_alphanumeric())
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
        // «e.g», «i.e»: abreviaturas, no archivos.
        && name.len() > 1
}

/// `src/app/`: al menos dos segmentos y acaba en `/`.
fn is_dir(word: &str) -> bool {
    let Some(path) = word.strip_suffix('/') else {
        return false;
    };
    path.contains('/')
        && path.split('/').all(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        })
}

/// Números de fase que nombra el texto: «fase 2», «Phase 3», «fases 2 y 3», «phases 2, 4».
fn phase_numbers(text: &str) -> Vec<usize> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '#')
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if !matches!(*w, "fase" | "fases" | "phase" | "phases") {
            continue;
        }
        // Los números que siguen (con «y», «and», «o», «or» entre medias).
        for next in &words[i + 1..] {
            if let Ok(n) = next.trim_start_matches('#').parse::<usize>() {
                out.push(n);
            } else if !matches!(*next, "y" | "and" | "o" | "or" | "e") {
                break;
            }
        }
    }
    out
}

/// «plan #194, fase 2»: como lo apunta `forge worktree new` en el registro del worktree.
pub fn record_label(idea: &Idea, index: usize) -> String {
    tr!("plan #{id}, fase {n}", id = idea.id, n = index + 1)
}

/// El mensaje inicial del agente que hace la fase `index` (desde 0) en `worktree`, junto a
/// las fases `others` (desde 0) que se lanzan a la vez. `checks`: los checks del proyecto
/// («Tests: `cargo test`»). En inglés: lo lee el agente, sea cual sea. `None` si la fase no
/// existe o no tiene rama.
pub fn brief(
    idea: &Idea,
    index: usize,
    worktree: &Path,
    others: &[usize],
    checks: &[String],
) -> Option<String> {
    let phase = idea.phases.get(index)?;
    let branch = phase.branch.as_deref()?.trim();
    let n = index + 1;
    let mut out = format!(
        "You are working on phase {n} (\"{title}\") of plan #{id} (\"{plan}\") in Forge's plans.\n",
        title = phase.title,
        id = idea.id,
        plan = idea.title,
    );
    let others: Vec<String> = others
        .iter()
        .filter(|&&o| o != index)
        .filter_map(|&o| Some(format!("{} (\"{}\")", o + 1, idea.phases.get(o)?.title)))
        .collect();
    if !others.is_empty() {
        out += &format!(
            "Other agents are working at the same time on phase {} of this plan, each in its own worktree.\n",
            others.join(", ")
        );
    }
    out += &format!(
        "\nYour worktree: {} (branch `{branch}`). You are already in it: work only here. Do not create other worktrees or clones and do not switch branches; if you really need another worktree, use `forge worktree new <branch>`.\n",
        // Sin el `\\?\` de Windows: el agente lo copia tal cual.
        worktree.display().to_string().trim_start_matches(r"\\?\")
    );
    out += &format!("\nGoal: {}\n", phase.title);
    if !phase.notes.trim().is_empty() {
        out += &format!("Notes from the plan: {}\n", phase.notes.trim());
    }
    if !idea.note.trim().is_empty() {
        out += &format!("About the whole plan: {}\n", idea.note.trim());
    }
    out += "\nTasks:\n";
    if phase.tasks.is_empty() {
        out += "- (the plan lists no tasks: work out the steps from the goal and add them with plan_update add_task)\n";
    }
    for t in &phase.tasks {
        out += &format!("- [{}] {}\n", if t.done { "x" } else { " " }, t.text);
    }
    let files = files_named(phase);
    out += &if files.is_empty() {
        "\nFiles in scope: the plan names none; keep to what the tasks need.\n".to_string()
    } else {
        format!(
            "\nFiles in scope (named by the plan): {}\n",
            files.join(", ")
        )
    };
    out += "\nDone when: every task above is done and ticked, each behavior you add or fix is covered by a test you saw fail before the change and pass after it, and ";
    out += &if checks.is_empty() {
        "the project's full test suite passes.\n".to_string()
    } else {
        format!("these project checks pass: {}.\n", checks.join("; "))
    };
    out += &format!(
        "\nRules:\n\
         - Forge already marked this phase in_progress with branch `{branch}`. Tick each task as soon as it is done with Forge's MCP tool plan_update (plan={id}, phase={n}, task, task_done=true).\n\
         - Commit only to `{branch}`. Push it and open a pull request when the phase is done. Do not merge it and do not commit to the main branch: the owner merges.\n\
         - Other agents edit other phases in parallel: stay inside your scope; if you must touch a file outside it, say so in the pull request. `forge work-status` shows who is touching what.\n\
         - Before you start, read the project's conventions (Forge's MCP tools project_context and memory_search, and AGENTS.md or the project's instructions if there are any), and ask if something essential is unclear.\n",
        id = idea.id,
    );
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ideas::Task;

    fn phase(title: &str, branch: Option<&str>, notes: &str, tasks: &[&str]) -> Phase {
        Phase {
            title: title.into(),
            notes: notes.into(),
            branch: branch.map(String::from),
            tasks: tasks
                .iter()
                .map(|t| Task {
                    text: t.to_string(),
                    done: false,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn plan(phases: Vec<Phase>) -> Idea {
        Idea {
            id: 194,
            title: "Agentes en paralelo".into(),
            note: String::new(),
            status: "doing".into(),
            source: "x".into(),
            updated_by: "x".into(),
            created_at: 0,
            updated_at: 0,
            phases,
        }
    }

    #[test]
    fn two_pending_phases_with_their_own_branch_can_go_together() {
        let idea = plan(vec![
            phase("Uno", Some("feat/uno"), "", &[]),
            phase("Dos", Some("feat/dos"), "", &[]),
        ]);
        assert_eq!(check(&idea, &[0, 1]), Ok(vec![]));
    }

    #[test]
    fn a_phase_without_a_branch_is_refused_and_says_why() {
        let idea = plan(vec![
            phase("Uno", Some("feat/uno"), "", &[]),
            phase("Dos", None, "", &[]),
        ]);
        let why = check(&idea, &[0, 1]).unwrap_err();
        assert!(
            why.contains("fase 2") && why.contains("no tiene rama"),
            "{why}"
        );
        assert!(refusal(&idea.phases[0], 0).is_none());
        assert!(refusal(&idea.phases[1], 1).is_some());
    }

    #[test]
    fn one_phase_done_ones_running_ones_and_shared_branches_are_refused() {
        let mut done = phase("Hecha", Some("feat/a"), "", &[]);
        done.status = "done".into();
        let mut doing = phase("En curso", Some("feat/b"), "", &[]);
        doing.status = "doing".into();
        let idea = plan(vec![
            done,
            doing,
            phase("C", Some("feat/c"), "", &[]),
            phase("D", Some("feat/c"), "", &[]),
        ]);
        assert!(check(&idea, &[2]).unwrap_err().contains("al menos dos"));
        let why = check(&idea, &[0, 1, 2, 3]).unwrap_err();
        assert!(why.contains("fase 1 ya está completada"), "{why}");
        assert!(why.contains("fase 2 ya está en curso"), "{why}");
        assert!(why.contains("misma rama (feat/c)"), "{why}");
        assert!(
            check(&idea, &[2, 9])
                .unwrap_err()
                .contains("no tiene fase 10")
        );
    }

    #[test]
    fn phases_that_name_the_same_files_or_each_other_warn_without_blocking() {
        let idea = plan(vec![
            phase(
                "Panel",
                Some("feat/panel"),
                "Toca `src/app/ideas_panel.rs` y src/app/",
                &["Botón en ideas_panel.rs"],
            ),
            phase(
                "Brief",
                Some("feat/brief"),
                "Tras la fase 1.",
                &["Cambiar src/app/ideas_panel.rs:120, p. ej. el plan"],
            ),
            phase("Otra", Some("feat/otra"), "", &["README.md"]),
        ]);
        let warnings = check(&idea, &[0, 1, 2]).unwrap();
        assert_eq!(
            warnings,
            vec![
                Warning::SharedFiles {
                    phases: (1, 2),
                    files: vec![
                        "src/app/ideas_panel.rs".into(),
                        "src/app/".into(),
                        "ideas_panel.rs".into()
                    ],
                },
                Warning::Mentions { phase: 2, other: 1 },
            ]
        );
        assert!(warnings[0].text().contains("src/app/ideas_panel.rs"));
    }

    #[test]
    fn branches_urls_and_abbreviations_are_not_files() {
        let p = phase(
            "X",
            Some("feat/x"),
            "Rama feat/x, ver https://a.b/c.html, e.g. algo, v0.15.1 y docs/ y Cargo.toml.",
            &[],
        );
        assert_eq!(files_named(&p), vec!["Cargo.toml".to_string()]);
        // Código y nombres de producto con punto tampoco: solo extensiones de archivo conocidas
        // (o una ruta con `/`).
        let p = phase(
            "X",
            None,
            "Pon self.error y ws.ideas como en Node.js; ver docs/guide.weird",
            &[],
        );
        assert_eq!(files_named(&p), vec!["docs/guide.weird".to_string()]);
    }

    #[test]
    fn the_same_file_named_with_or_without_its_folder_still_warns() {
        let idea = plan(vec![
            phase("A", Some("feat/a"), "", &["Botón en ideas_panel.rs"]),
            phase(
                "B",
                Some("feat/b"),
                "",
                &["Cambiar ./src/app/Ideas_Panel.rs"],
            ),
        ]);
        assert_eq!(
            check(&idea, &[0, 1]).unwrap(),
            vec![Warning::SharedFiles {
                phases: (1, 2),
                files: vec!["ideas_panel.rs".into()],
            }]
        );
    }

    #[test]
    fn the_brief_has_the_goal_tasks_scope_done_criteria_and_the_rules() {
        let idea = plan(vec![
            phase(
                "Panel de planes",
                Some("feat/panel"),
                "Hecho cuando el botón lanza las fases.",
                &["Añadir el botón", "Probarlo en src/app/e2e.rs"],
            ),
            phase("Brief", Some("feat/brief"), "", &[]),
        ]);
        let wt = Path::new("/tmp/wt/feat-panel");
        let checks = vec!["Tests: `cargo test`".to_string()];
        let brief = brief(&idea, 0, wt, &[0, 1], &checks).unwrap();
        for part in [
            "phase 1 (\"Panel de planes\") of plan #194",
            "Other agents are working at the same time on phase 2 (\"Brief\")",
            "Your worktree: /tmp/wt/feat-panel (branch `feat/panel`)",
            "You are already in it",
            "forge worktree new",
            "Goal: Panel de planes",
            "Hecho cuando el botón lanza las fases.",
            "- [ ] Añadir el botón",
            "- [ ] Probarlo en src/app/e2e.rs",
            "Files in scope (named by the plan): src/app/e2e.rs",
            "Done when:",
            "Tests: `cargo test`",
            "in_progress with branch `feat/panel`",
            "plan_update (plan=194, phase=1, task, task_done=true)",
            "Do not merge",
        ] {
            assert!(brief.contains(part), "falta «{part}» en:\n{brief}");
        }
        // Sin rama no hay worktree: no hay brief.
        let no_branch = plan(vec![phase("X", None, "", &[])]);
        assert!(super::brief(&no_branch, 0, wt, &[], &[]).is_none());
    }
}
