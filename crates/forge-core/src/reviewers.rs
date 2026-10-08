// Revisores especializados (arquitectura, seguridad, base de datos, frontend…):
// cada uno solo se activa si el diff toca su área y solo recibe esos archivos.
use crate::tr;
use std::path::Path;

use serde::Deserialize;

use crate::guard::{FileChange, glob_match};
use crate::router::{Risk, RiskLevel};

/// Nivel de modelo del revisor (plan 9.3: `route`).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Cheap,
    Balanced,
    Powerful,
}

/// Detección incluida para los revisores conocidos cuando no se dan `paths`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Detector {
    Architecture,
    Security,
    Database,
    Frontend,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReviewerSpec {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub tier: Tier,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Si su veredicto puede bloquear; si no, sus objeciones son informativas.
    pub blocking: bool,
    pub paths: Vec<String>,
    /// En qué se fija (va al prompt).
    pub focus: String,
    pub detector: Option<Detector>,
}

pub fn builtins() -> Vec<ReviewerSpec> {
    let reviewer = |id: &str, name: &str, tier, blocking, detector, focus: &str| ReviewerSpec {
        id: id.into(),
        name: name.into(),
        enabled: true,
        tier,
        provider: None,
        model: None,
        effort: None,
        blocking,
        paths: Vec::new(),
        focus: focus.into(),
        detector: Some(detector),
    };
    vec![
        reviewer(
            "architecture",
            tr!("Revisor de arquitectura"),
            Tier::Powerful,
            false,
            Detector::Architecture,
            tr!(
                "arquitectura: límites entre módulos, acoplamiento, responsabilidades, duplicación y riesgos de mantenimiento"
            ),
        ),
        reviewer(
            "security",
            tr!("Revisor de seguridad"),
            Tier::Powerful,
            true,
            Detector::Security,
            tr!(
                "seguridad: autenticación, autorización y permisos, inyección, exposición de secretos y datos personales, pagos"
            ),
        ),
        reviewer(
            "database",
            tr!("Revisor de base de datos"),
            Tier::Balanced,
            true,
            Detector::Database,
            tr!(
                "base de datos: migraciones destructivas o irreversibles, pérdida de datos, bloqueos largos, índices, restricciones, RLS y consultas N+1"
            ),
        ),
        reviewer(
            "frontend",
            tr!("Revisor de frontend"),
            Tier::Balanced,
            false,
            Detector::Frontend,
            tr!(
                "frontend: accesibilidad (contraste, foco, etiquetas), regresiones visuales, estado, rendimiento de renderizado y textos"
            ),
        ),
    ]
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct File {
    reviewers: Vec<ReviewerEntry>,
    /// Los agentes los valida `agents::load`.
    #[serde(rename = "agents")]
    _agents: Vec<toml::Table>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewerEntry {
    id: String,
    name: Option<String>,
    enabled: Option<bool>,
    route: Option<Tier>,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    blocking: Option<bool>,
    paths: Option<Vec<String>>,
    focus: Option<String>,
}

/// Revisores del proyecto: los incluidos con los cambios de `[[reviewers]]` y los propios.
pub fn load(project: &Path) -> Result<Vec<ReviewerSpec>, String> {
    let path = project.join(".forge/agents.toml");
    let file: File = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
    };
    let mut reviewers = builtins();
    for entry in file.reviewers {
        let i = match reviewers.iter().position(|r| r.id == entry.id) {
            Some(i) => i,
            None => {
                if entry.paths.as_ref().is_none_or(Vec::is_empty) {
                    return Err(tr!(
                        "{p0}: el revisor \"{p1}\" necesita `paths` (rutas que activan la revisión)",
                        p0 = path.display(),
                        p1 = entry.id
                    ));
                }
                reviewers.push(ReviewerSpec {
                    id: entry.id.clone(),
                    name: entry.id.clone(),
                    enabled: true,
                    tier: Tier::Balanced,
                    provider: None,
                    model: None,
                    effort: None,
                    blocking: false,
                    paths: Vec::new(),
                    focus: tr!("el área \"{p0}\"", p0 = entry.id),
                    detector: None,
                });
                reviewers.len() - 1
            }
        };
        let r = &mut reviewers[i];
        if let Some(v) = entry.name {
            r.name = v;
        }
        if let Some(v) = entry.enabled {
            r.enabled = v;
        }
        if let Some(v) = entry.route {
            r.tier = v;
        }
        r.provider = entry.provider.or(r.provider.take());
        r.model = entry.model.or(r.model.take());
        r.effort = entry.effort.or(r.effort.take());
        if let Some(v) = entry.blocking {
            r.blocking = v;
        }
        if let Some(v) = entry.paths {
            r.paths = v;
        }
        if let Some(v) = entry.focus {
            r.focus = v;
        }
    }
    Ok(reviewers)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Activation {
    pub reviewer: ReviewerSpec,
    /// Archivos de su área (los únicos que recibe).
    pub files: Vec<String>,
}

const DATABASE: [&str; 10] = [
    "migration",
    "drizzle",
    "prisma",
    "schema",
    ".sql",
    "seed",
    "knex",
    "typeorm",
    "sequelize",
    "supabase/",
];
const FRONTEND_EXT: [&str; 8] = [
    ".tsx", ".jsx", ".vue", ".svelte", ".css", ".scss", ".html", ".astro",
];
const FRONTEND_DIRS: [&str; 3] = ["components/", "pages/", "styles/"];

fn matches_any(path: &str, words: &[&str]) -> bool {
    let p = path.to_lowercase();
    words.iter().any(|w| p.contains(w))
}

/// Qué revisores corren con este diff (y con qué archivos) y por qué los demás no.
pub fn activate(
    reviewers: &[ReviewerSpec],
    files: &[FileChange],
    risk: &Risk,
) -> (Vec<Activation>, Vec<(String, String)>) {
    let mut active = Vec::new();
    let mut skipped = Vec::new();
    for r in reviewers.iter().filter(|r| r.enabled) {
        let pick = |pred: &dyn Fn(&str) -> bool| {
            files
                .iter()
                .map(|f| f.path.clone())
                .filter(|p| pred(p))
                .collect::<Vec<_>>()
        };
        let (matched, why_not) = if !r.paths.is_empty() {
            (
                pick(&|p| r.paths.iter().any(|g| glob_match(g, p))),
                tr!("no toca sus rutas"),
            )
        } else {
            match r.detector {
                Some(Detector::Security) => {
                    let words: Vec<&str> = crate::router::security_words();
                    (
                        pick(&|p| matches_any(p, &words)),
                        tr!("no toca autenticación, permisos, pagos ni secretos"),
                    )
                }
                Some(Detector::Database) => (
                    pick(&|p| matches_any(p, &DATABASE)),
                    tr!("no toca migraciones ni esquema"),
                ),
                Some(Detector::Frontend) => (
                    pick(&|p| {
                        FRONTEND_EXT.iter().any(|e| p.ends_with(e))
                            || matches_any(p, &FRONTEND_DIRS)
                    }),
                    tr!("no toca la interfaz"),
                ),
                Some(Detector::Architecture) => {
                    let top_dirs: std::collections::HashSet<&str> = files
                        .iter()
                        .filter_map(|f| f.path.split_once('/').map(|(d, _)| d))
                        .collect();
                    let wide =
                        risk.level >= RiskLevel::High || top_dirs.len() >= 4 || files.len() > 20;
                    (
                        if wide { pick(&|_| true) } else { Vec::new() },
                        tr!("cambio acotado (riesgo bajo o medio, pocas carpetas)"),
                    )
                }
                None => (Vec::new(), tr!("sin rutas configuradas")),
            }
        };
        if matched.is_empty() {
            skipped.push((r.name.clone(), why_not.to_string()));
        } else {
            active.push(Activation {
                reviewer: r.clone(),
                files: matched,
            });
        }
    }
    (active, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> FileChange {
        FileChange {
            path: path.into(),
            added: 10,
            deleted: 0,
            binary: false,
            removed: false,
        }
    }

    fn low() -> Risk {
        Risk {
            score: 0,
            level: RiskLevel::Low,
            signals: vec![],
            security: false,
        }
    }

    #[test]
    fn activates_only_relevant_reviewers_with_their_files() {
        let files = [
            file("drizzle/0004_lotes.sql"),
            file("src/components/Vacunas.tsx"),
            file("src/lib/fechas.ts"),
        ];
        let (active, skipped) = activate(&builtins(), &files, &low());
        let ids: Vec<&str> = active.iter().map(|a| a.reviewer.id.as_str()).collect();
        assert_eq!(ids, ["database", "frontend"]);
        assert_eq!(
            active[0].files,
            ["drizzle/0004_lotes.sql"],
            "solo recibe su área"
        );
        assert_eq!(active[1].files, ["src/components/Vacunas.tsx"]);
        assert!(
            skipped
                .iter()
                .any(|(n, why)| n.contains("seguridad") && why.contains("autenticación"))
        );
        assert!(skipped.iter().any(|(n, _)| n.contains("arquitectura")));

        let (active, _) = activate(&builtins(), &[file("src/auth/session.ts")], &low());
        assert_eq!(
            active
                .iter()
                .map(|a| a.reviewer.id.as_str())
                .collect::<Vec<_>>(),
            ["security"]
        );
        let high = Risk {
            score: 6,
            level: RiskLevel::High,
            signals: vec![],
            security: false,
        };
        let (active, _) = activate(&builtins(), &[file("src/lib/fechas.ts")], &high);
        assert!(
            active.iter().any(|a| a.reviewer.id == "architecture"),
            "riesgo alto activa arquitectura"
        );
    }

    #[test]
    fn config_overrides_and_custom_reviewers() {
        let dir = std::env::temp_dir().join(format!("forge-reviewers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        std::fs::write(
            dir.join(".forge/agents.toml"),
            "[[agents]]\nid = \"claude\"\n\
             [[reviewers]]\nid = \"frontend\"\nenabled = false\n\
             [[reviewers]]\nid = \"database\"\nblocking = false\nroute = \"cheap\"\n\
             [[reviewers]]\nid = \"vacunas\"\nname = \"Revisor sanitario\"\npaths = [\"src/vacunas/**\"]\nblocking = true\nfocus = \"normativa sanitaria\"\n",
        )
        .unwrap();
        let reviewers = load(&dir).unwrap();
        let get = |id: &str| reviewers.iter().find(|r| r.id == id).unwrap();
        assert!(!get("frontend").enabled);
        assert_eq!(
            (get("database").blocking, get("database").tier),
            (false, Tier::Cheap)
        );
        let (active, _) = activate(
            &reviewers,
            &[file("src/vacunas/dosis.ts"), file("src/App.tsx")],
            &low(),
        );
        assert_eq!(
            active
                .iter()
                .map(|a| a.reviewer.id.as_str())
                .collect::<Vec<_>>(),
            ["vacunas"],
            "frontend desactivado"
        );

        std::fs::write(
            dir.join(".forge/agents.toml"),
            "[[reviewers]]\nid = \"nuevo\"\n",
        )
        .unwrap();
        assert!(load(&dir).unwrap_err().contains("necesita `paths`"));
        std::fs::write(
            dir.join(".forge/agents.toml"),
            "[[reviewers]]\nid = \"database\"\nrute = \"cheap\"\n",
        )
        .unwrap();
        assert!(load(&dir).unwrap_err().contains("rute"));
    }
}
