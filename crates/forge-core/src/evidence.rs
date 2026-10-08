// Evidencias: resultado de cada validación ligado a su candidato, historial y reporte exportable.
use serde::{Deserialize, Serialize};

use crate::candidate::Candidate;
use crate::guard::{Level, Stage, Verdict};

/// Un check que pasó sobre un árbol concreto. Se reutiliza solo para ese mismo árbol
/// y ese mismo comando: cualquier cambio de contenido o de regla obliga a repetirlo.
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub check_id: String,
    pub command: String,
    pub tree: String,
    pub duration_ms: u64,
    pub output: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportItem {
    pub id: String,
    pub label: String,
    pub level: Level,
    pub details: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportCheck {
    pub id: String,
    pub name: String,
    pub command: String,
    pub level: Level,
    /// Texto legible: "código 1", "tiempo agotado (300 s)"…
    pub status: String,
    pub duration_ms: u64,
    pub output: String,
    /// Si no se ejecutó porque había evidencia del mismo candidato: cuándo se obtuvo.
    pub reused_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub project: String,
    pub stage: Stage,
    pub candidate: Option<Candidate>,
    pub branch: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub verdict: Verdict,
    pub progress: (usize, usize),
    pub items: Vec<ReportItem>,
    pub checks: Vec<ReportCheck>,
    pub error: Option<String>,
}

fn mark(level: Level) -> &'static str {
    match level {
        Level::Pass => "✓",
        Level::Warn => "⚠",
        Level::Block => "✕",
        Level::Pending => "○",
        Level::Info => "•",
        Level::Excepted => "↷",
    }
}

pub fn verdict_label(verdict: Verdict, stage: Stage) -> String {
    let target = match stage {
        Stage::Commit => "commit",
        Stage::Push => "push",
        Stage::PullRequest => "pull request",
    };
    match verdict {
        Verdict::Ready => format!("Listo para {target}"),
        Verdict::Warnings => format!("Listo para {target} con advertencias"),
        Verdict::Blocked => "Bloqueado".into(),
        Verdict::Running => "Sin terminar".into(),
        Verdict::Pending => "Pendiente de revisión".into(),
        Verdict::NothingToCheck => "Sin cambios".into(),
    }
}

/// (año, mes, día) UTC de un instante Unix.
fn civil(timestamp: i64) -> (i64, i64, i64) {
    let z = timestamp.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Inicio (00:00 UTC del día 1) del mes natural que contiene `timestamp`.
pub fn month_start(timestamp: i64) -> i64 {
    let (y, m, _) = civil(timestamp);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400
}

/// Fecha UTC "2026-10-07 18:20 UTC" a partir de segundos Unix.
pub fn utc(timestamp: i64) -> String {
    // Algoritmo de días civiles (Howard Hinnant).
    let days = timestamp.div_euclid(86_400);
    let secs = timestamp.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60
    )
}

impl Report {
    pub fn markdown(&self) -> String {
        let mut md = format!(
            "# Forge Guard · {} · {}\n\n",
            self.project,
            self.stage.label()
        );
        md += &format!(
            "**{}** ({} de {} controles)\n\n",
            verdict_label(self.verdict, self.stage),
            self.progress.0,
            self.progress.1
        );
        if let Some(c) = &self.candidate {
            md += &format!("- Candidato: `{}`\n", c.id);
            md += &format!(
                "- HEAD: `{}` · rama `{}`\n",
                c.head.chars().take(10).collect::<String>(),
                self.branch
            );
            md += &format!("- Árbol: `{}`\n- Diff: `{}`\n", c.tree, c.diff_hash);
            md += &format!(
                "- Checks ejecutados en: {}\n",
                if c.isolated {
                    "copia aislada del candidato"
                } else {
                    "el árbol de trabajo (idéntico al candidato)"
                }
            );
        }
        md += &format!(
            "- Fecha: {} (duración {} s)\n\n",
            utc(self.started_at),
            (self.finished_at - self.started_at).max(0)
        );
        if let Some(e) = &self.error {
            md += &format!("> ✕ {e}\n\n");
        }
        md += "## Controles\n\n";
        for i in &self.items {
            md += &format!("- {} {}\n", mark(i.level), i.label);
            for d in &i.details {
                md += &format!("  - {d}\n");
            }
        }
        if !self.checks.is_empty() {
            md += "\n## Checks\n\n| | Check | Resultado | Duración | Comando |\n|---|---|---|---|---|\n";
            for c in &self.checks {
                let status = match c.reused_at {
                    Some(at) => format!("{} (evidencia del {})", c.status, utc(at)),
                    None => c.status.clone(),
                };
                md += &format!(
                    "| {} | {} | {} | {:.1} s | `{}` |\n",
                    mark(c.level),
                    c.name,
                    status,
                    c.duration_ms as f64 / 1000.0,
                    c.command.replace('|', "\\|")
                );
            }
            for c in self
                .checks
                .iter()
                .filter(|c| matches!(c.level, Level::Block | Level::Warn) && !c.output.is_empty())
            {
                md += &format!("\n### Salida de {}\n\n```text\n{}\n```\n", c.name, c.output);
            }
        }
        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_791_400_800), "2026-10-07 19:20 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc(month_start(1_791_400_800)), "2026-10-01 00:00 UTC");
        assert_eq!(utc(month_start(951_782_400)), "2000-02-01 00:00 UTC");
        assert_eq!(month_start(0), 0);
    }

    #[test]
    fn markdown_report() {
        let report = Report {
            project: "Bovinapp".into(),
            stage: Stage::Commit,
            candidate: Some(Candidate {
                id: "bovinapp-7f9a21".into(),
                tree: "7f9a21ab".into(),
                head: "a8f3c12999".into(),
                diff_hash: "sha256:91ea".into(),
                isolated: true,
            }),
            branch: "main".into(),
            started_at: 1_791_400_800,
            finished_at: 1_791_400_812,
            verdict: Verdict::Blocked,
            progress: (5, 6),
            items: vec![ReportItem {
                id: "lines".into(),
                label: "120 líneas".into(),
                level: Level::Pass,
                details: vec![],
            }],
            checks: vec![ReportCheck {
                id: "tests".into(),
                name: "Tests".into(),
                command: "npm test | tee".into(),
                level: Level::Block,
                status: "código 1".into(),
                duration_ms: 3200,
                output: "FAIL vacunas.test.ts".into(),
                reused_at: None,
            }],
            error: None,
        };
        let md = report.markdown();
        for expected in [
            "bovinapp-7f9a21",
            "copia aislada",
            "| ✕ | Tests | código 1 | 3.2 s | `npm test \\| tee` |",
            "FAIL vacunas.test.ts",
            "**Bloqueado**",
        ] {
            assert!(md.contains(expected), "falta {expected:?} en:\n{md}");
        }
    }
}
