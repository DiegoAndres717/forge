// Pull requests: borrador (título y descripción) a partir de los commits de la rama y del
// resultado de Guard, y el comando `gh pr create` que lo publica.
use std::path::{Path, PathBuf};

use crate::agents::shell_quote;
use crate::evidence::{Report, mark};
use crate::guard::{self, Level, Rules};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    /// Rama destino tal como la espera `gh` (sin `origin/`).
    pub base: String,
    pub title: String,
    pub body: String,
    /// Crear como borrador (draft) en GitHub.
    pub draft: bool,
}

/// Prepara el borrador del PR de la rama actual contra la base de `[pull_request]`.
// ponytail: texto determinista (sin IA); si se quiere redactado por un modelo, pasar
// este borrador al router como contexto.
pub fn draft(repo: &Path, rules: &Rules, report: &Report) -> Result<Draft, String> {
    let base_ref = match &rules.pull_request.base {
        Some(base) => base.clone(),
        None => guard::default_branch(repo)?,
    };
    let base = base_ref
        .strip_prefix("origin/")
        .unwrap_or(&base_ref)
        .to_string();
    let branch = guard::git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    if branch == base || branch == "HEAD" {
        return Err(format!(
            "estás en `{branch}`: crea una rama para el PR (git switch -c mi-cambio)"
        ));
    }
    let range = format!("{base_ref}..HEAD");
    let subjects: Vec<String> = guard::git(repo, &["log", "--reverse", "--format=%s", &range])?
        .lines()
        .map(str::to_string)
        .collect();
    if subjects.is_empty() {
        return Err(format!(
            "la rama `{branch}` no tiene commits nuevos frente a `{base}`"
        ));
    }
    let title = match subjects.as_slice() {
        [only] => only.clone(),
        _ => title_from_branch(&branch),
    };
    let stat = guard::git(
        repo,
        &["diff", "--shortstat", &format!("{base_ref}...HEAD")],
    )
    .unwrap_or_default();
    Ok(Draft {
        base,
        title,
        body: body(&subjects, stat.trim(), report),
        draft: false,
    })
}

/// "feat/vacunas-por-lote" → "Vacunas por lote".
fn title_from_branch(branch: &str) -> String {
    let name = branch
        .rsplit('/')
        .next()
        .unwrap_or(branch)
        .replace(['-', '_'], " ");
    let mut chars = name.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => branch.to_string(),
    }
}

fn body(subjects: &[String], stat: &str, report: &Report) -> String {
    let mut md = String::from("## Cambios\n\n");
    for s in subjects {
        md += &format!("- {s}\n");
    }
    if !stat.is_empty() {
        md += &format!("\n{stat}\n");
    }
    md += "\n## Validación\n\nValidado con Forge Guard antes de abrir el PR:\n\n";
    for c in &report.checks {
        md += &format!("- {} {} (`{}`)\n", mark(c.level), c.name, c.command);
    }
    // Lo que el revisor debe mirar: avisos y reglas omitidas con motivo.
    for i in report
        .items
        .iter()
        .filter(|i| matches!(i.level, Level::Warn | Level::Excepted))
    {
        md += &format!("- {} {}\n", mark(i.level), i.label);
        for d in &i.details {
            md += &format!("  - {d}\n");
        }
    }
    if report.checks.is_empty() && !report.items.iter().any(|i| i.level == Level::Warn) {
        md += "- ✓ Sin bloqueos en las reglas del proyecto\n";
    }
    md
}

/// Guarda la descripción en el directorio de Git (no ensucia el árbol de trabajo) y
/// devuelve el comando `gh pr create` que la usa.
pub fn command(repo: &Path, draft: &Draft) -> Result<String, String> {
    let path = PathBuf::from(
        guard::git(
            repo,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "FORGE_PR_BODY.md",
            ],
        )?
        .trim(),
    );
    std::fs::write(&path, &draft.body).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "gh pr create --base {} --title {} --body-file {}{}",
        shell_quote(&draft.base),
        shell_quote(draft.title.trim()),
        shell_quote(&path.to_string_lossy()),
        if draft.draft { " --draft" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::{ReportCheck, ReportItem};
    use crate::guard::{Stage, Verdict};
    use std::process::Command;

    fn sh(dir: &Path, script: &str) {
        let ok = Command::new("sh")
            .args(["-c", script])
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "{script}");
    }

    fn report() -> Report {
        Report {
            project: "Bovinapp".into(),
            stage: Stage::PullRequest,
            candidate: None,
            branch: "feat/vacunas-por-lote".into(),
            started_at: 0,
            finished_at: 0,
            verdict: Verdict::Warnings,
            progress: (2, 2),
            items: vec![ReportItem {
                id: "lines".into(),
                label: "2.400 líneas cambiadas".into(),
                level: Level::Warn,
                details: vec![],
            }],
            checks: vec![ReportCheck {
                id: "tests".into(),
                name: "Tests".into(),
                command: "npm test".into(),
                level: Level::Pass,
                status: "ok".into(),
                duration_ms: 10,
                output: String::new(),
                reused_at: None,
            }],
            error: None,
        }
    }

    #[test]
    fn draft_from_branch_commits_and_guard_report() {
        let dir = std::env::temp_dir().join(format!("forge-pr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        sh(
            &dir,
            "git init -q -b main && git config user.email t@t && git config user.name t \
             && echo a > a && git add . && git commit -qm inicio \
             && git switch -qc feat/vacunas-por-lote \
             && echo b > b && git add . && git commit -qm 'Registrar vacunas por lote' \
             && echo c > c && git add . && git commit -qm 'Validar lote duplicado'",
        );
        let rules = Rules::default();
        let d = draft(&dir, &rules, &report()).unwrap();
        assert_eq!(d.base, "main");
        assert_eq!(
            d.title, "Vacunas por lote",
            "varios commits: título desde la rama"
        );
        assert!(
            d.body
                .contains("- Registrar vacunas por lote\n- Validar lote duplicado")
        );
        assert!(d.body.contains("2 files changed"));
        assert!(d.body.contains("✓ Tests (`npm test`)"));
        assert!(d.body.contains("⚠ 2.400 líneas cambiadas"));

        let cmd = command(
            &dir,
            &Draft {
                title: "Vacunas' por lote".into(),
                ..d
            },
        )
        .unwrap();
        assert!(cmd.starts_with(
            "gh pr create --base 'main' --title 'Vacunas'\\'' por lote' --body-file '"
        ));
        let body_file = dir.join(".git/FORGE_PR_BODY.md");
        assert!(
            std::fs::read_to_string(body_file)
                .unwrap()
                .starts_with("## Cambios")
        );

        // Desde la rama base no hay PR que crear.
        sh(&dir, "git switch -q main");
        assert!(
            draft(&dir, &rules, &report())
                .unwrap_err()
                .contains("crea una rama")
        );
    }
}
