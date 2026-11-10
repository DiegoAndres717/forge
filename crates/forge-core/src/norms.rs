// Normas del proyecto (plan #237): reglas que escribe el dueño en `.forge/norms.toml` y que el
// revisor aplica solo a los archivos que tocan sus rutas. No es un sistema aparte: se cargan
// como un revisor especializado más (id "norms") y corren cuando la etapa pide revisión con
// IA (`require_ai_review` en rules.toml) o a mano con `forge review`.
use crate::tr;
use std::path::Path;

use serde::Deserialize;

use crate::guard::glob_match;
use crate::reviewers::{ReviewerSpec, Tier};
use crate::router::Finding;

/// Id del revisor que aplica las normas (`reviewer:norms` en Guard y en `forge allow`).
pub const REVIEWER_ID: &str = "norms";

/// Gravedad de una norma, con la semántica de los revisores: una bloqueante puede detener el
/// commit; una sugerencia solo avisa.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Blocking,
    #[default]
    Suggestion,
}

impl Severity {
    /// Palabra fija para el prompt y la huella (no se traduce: es un dato).
    pub fn key(self) -> &'static str {
        match self {
            Severity::Blocking => "blocking",
            Severity::Suggestion => "suggestion",
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Norm {
    pub id: String,
    /// Qué buscar, en texto libre (va tal cual al revisor).
    pub look_for: String,
    #[serde(default)]
    pub severity: Severity,
    /// Rutas o tipos de archivo a los que aplica (globs como los de rules.toml).
    pub paths: Vec<String>,
}

impl Norm {
    pub fn applies_to(&self, path: &str) -> bool {
        self.paths.iter().any(|g| glob_match(g, path))
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct File {
    norms: Vec<Norm>,
}

pub fn path(project: &Path) -> std::path::PathBuf {
    project.join(".forge/norms.toml")
}

/// Lee `.forge/norms.toml`; sin archivo, ninguna norma (nada cambia).
pub fn load(project: &Path) -> Result<Vec<Norm>, String> {
    let path = path(project);
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(text) => text,
    };
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse(text: &str) -> Result<Vec<Norm>, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::new();
    for n in &file.norms {
        if n.id.trim().is_empty() {
            return Err(tr!("una norma no tiene `id`").into());
        }
        if !seen.insert(n.id.as_str()) {
            return Err(tr!("la norma \"{p0}\" está repetida", p0 = n.id));
        }
        if n.look_for.trim().is_empty() {
            return Err(tr!(
                "la norma \"{p0}\" necesita `look_for` (qué buscar)",
                p0 = n.id
            ));
        }
        if n.paths.iter().all(|p| p.trim().is_empty()) {
            return Err(tr!(
                "la norma \"{p0}\" necesita `paths` (a qué archivos aplica)",
                p0 = n.id
            ));
        }
    }
    Ok(file.norms)
}

/// El revisor que lleva las normas: bloquea si alguna norma es bloqueante y, entonces, usa la
/// ruta potente (como el de seguridad). `[[reviewers]] id = "norms"` en agents.toml lo ajusta.
pub fn reviewer(norms: Vec<Norm>) -> ReviewerSpec {
    let blocking = norms.iter().any(|n| n.severity == Severity::Blocking);
    ReviewerSpec {
        id: REVIEWER_ID.into(),
        name: tr!("Revisor de normas del proyecto").into(),
        enabled: true,
        tier: if blocking {
            Tier::Powerful
        } else {
            Tier::Balanced
        },
        provider: None,
        model: None,
        effort: None,
        blocking,
        paths: Vec::new(),
        focus: tr!("las normas del proyecto").into(),
        detector: None,
        norms,
    }
}

/// Las normas que tocan alguno de estos archivos (las demás no se envían).
pub fn applicable(norms: &[Norm], files: &[String]) -> Vec<Norm> {
    norms
        .iter()
        .filter(|n| files.iter().any(|f| n.applies_to(f)))
        .cloned()
        .collect()
}

/// Instrucciones del revisor: cada norma con su gravedad y solo los archivos a los que aplica.
pub fn role(norms: &[Norm], files: &[String]) -> String {
    let list: Vec<String> = norms
        .iter()
        .map(|n| {
            let mine: Vec<&str> = files
                .iter()
                .filter(|f| n.applies_to(f))
                .map(String::as_str)
                .collect();
            format!(
                "- [{}] ({}) {}: {}",
                n.id,
                n.severity.key(),
                mine.join(", "),
                n.look_for.trim()
            )
        })
        .collect();
    tr!(
        "Eres el revisor de normas del proyecto. Revisa el cambio SOLO contra estas normas, cada una solo en los archivos que indica; lo que no incumpla ninguna no es un hallazgo y, si nada las incumple, aprueba.\n{p0}\nEn cada hallazgo añade \"norm\": \"<id de la norma que incumple>\". Marca \"blocking\" solo lo que incumple una norma blocking; lo de una suggestion va como \"warning\".",
        p0 = list.join("\n")
    )
}

/// Solo una norma bloqueante bloquea: un hallazgo grave de una sugerencia (o sin norma
/// conocida) queda como aviso. El id se normaliza por si el modelo lo envuelve en corchetes.
pub fn settle(findings: Vec<Finding>, norms: &[Norm]) -> Vec<Finding> {
    findings
        .into_iter()
        .map(|mut f| {
            f.norm = f
                .norm
                .trim()
                .trim_matches(|c| c == '[' || c == ']')
                .to_string();
            let blocks = norms
                .iter()
                .any(|n| n.id == f.norm && n.severity == Severity::Blocking);
            if f.severity == "blocking" && !blocks {
                f.severity = "warning".into();
            }
            f
        })
        .collect()
}

/// Huella de las normas aplicadas: si cambian, una aprobación previa del mismo árbol no vale.
pub fn fingerprint(norms: &[Norm]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for n in norms {
        h.update(n.id.as_bytes());
        h.update([0]);
        h.update(n.severity.key().as_bytes());
        h.update([0]);
        h.update(n.look_for.as_bytes());
        h.update([0]);
        h.update(n.paths.join("\n").as_bytes());
        h.update([1]);
    }
    crate::guard::diff::hex(&h.finalize())[..16].to_string()
}

/// Crea `.forge/norms.toml` con normas de seguridad de partida. Nada se enciende solo: las
/// normas corren donde la etapa ya pide revisión con IA o con `forge review`.
pub fn write_template(project: &Path) -> Result<std::path::PathBuf, String> {
    let file = path(project);
    if file.exists() {
        return Err(tr!("{p0} ya existe", p0 = file.display()));
    }
    std::fs::create_dir_all(project.join(".forge")).map_err(|e| e.to_string())?;
    std::fs::write(&file, template()).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(file)
}

/// Plantilla en el idioma de la interfaz (la traducción está en i18n_en.rs). Los `look_for`
/// van en inglés en los dos idiomas: son texto para el revisor.
pub fn template() -> &'static str {
    crate::i18n::t(TEMPLATE)
}

pub(crate) const TEMPLATE: &str = r#"# Normas del proyecto: lo que el revisor de Forge comprueba en cada cambio.
# Cada norma solo se aplica a los archivos de `paths` que el cambio toca; las demás no se
# envían. Corren cuando la etapa pide revisión con IA (require_ai_review en rules.toml) o
# con `forge review`. El revisor recibe el diff y las normas, nunca quién hizo el cambio.
#
# id        nombre corto; aparece en cada hallazgo
# look_for  qué buscar, en texto libre
# severity  "blocking" (puede bloquear) o "suggestion" (solo avisa)
# paths     rutas o tipos de archivo ("src/api/**", "*.py")

[[norms]]
id = "no-secrets-in-code"
severity = "blocking"
paths = ["**"]
look_for = "Secrets written in the code or config: API keys, tokens, passwords, private keys or connection strings with credentials. They must come from the environment or a secret manager."

[[norms]]
id = "no-sensitive-data-in-logs"
severity = "blocking"
paths = ["*.ts", "*.tsx", "*.js", "*.py", "*.rb", "*.go", "*.rs", "*.java", "*.kt", "*.cs", "*.php"]
look_for = "Logging, printing or sending to error trackers passwords, tokens, session cookies, full request bodies or personal data (emails, IDs, card numbers) without masking."

[[norms]]
id = "auth-on-endpoints"
severity = "blocking"
paths = ["**/api/**", "**/routes/**", "**/controllers/**", "**/handlers/**"]
look_for = "A new or changed endpoint that does not check that the caller is authenticated and allowed to act on that resource (missing auth middleware, missing ownership or role check)."

[[norms]]
id = "parameterized-sql"
severity = "blocking"
paths = ["*.ts", "*.js", "*.py", "*.rb", "*.go", "*.rs", "*.java", "*.kt", "*.cs", "*.php", "*.sql"]
look_for = "SQL built by concatenating or interpolating values (string + id, f-strings, template literals, format!) instead of bound parameters or the query builder's placeholders."
"#;

pub(crate) const TEMPLATE_EN: &str = r#"# Project norms: what Forge's reviewer checks on every change.
# Each norm applies only to the files in `paths` that the change touches; the others are not
# sent. They run when the stage asks for an AI review (require_ai_review in rules.toml) or
# with `forge review`. The reviewer gets the diff and the norms, never who made the change.
#
# id        short name; it shows up in every finding
# look_for  what to look for, in free text
# severity  "blocking" (can block) or "suggestion" (only warns)
# paths     paths or file types ("src/api/**", "*.py")

[[norms]]
id = "no-secrets-in-code"
severity = "blocking"
paths = ["**"]
look_for = "Secrets written in the code or config: API keys, tokens, passwords, private keys or connection strings with credentials. They must come from the environment or a secret manager."

[[norms]]
id = "no-sensitive-data-in-logs"
severity = "blocking"
paths = ["*.ts", "*.tsx", "*.js", "*.py", "*.rb", "*.go", "*.rs", "*.java", "*.kt", "*.cs", "*.php"]
look_for = "Logging, printing or sending to error trackers passwords, tokens, session cookies, full request bodies or personal data (emails, IDs, card numbers) without masking."

[[norms]]
id = "auth-on-endpoints"
severity = "blocking"
paths = ["**/api/**", "**/routes/**", "**/controllers/**", "**/handlers/**"]
look_for = "A new or changed endpoint that does not check that the caller is authenticated and allowed to act on that resource (missing auth middleware, missing ownership or role check)."

[[norms]]
id = "parameterized-sql"
severity = "blocking"
paths = ["*.ts", "*.js", "*.py", "*.rb", "*.go", "*.rs", "*.java", "*.kt", "*.cs", "*.php", "*.sql"]
look_for = "SQL built by concatenating or interpolating values (string + id, f-strings, template literals, format!) instead of bound parameters or the query builder's placeholders."
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(id: &str, severity: Severity, paths: &[&str]) -> Norm {
        Norm {
            id: id.into(),
            look_for: format!("look for {id}"),
            severity,
            paths: paths.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn the_file_is_validated() {
        let norms = parse(
            "[[norms]]\nid = \"a\"\nlook_for = \"x\"\npaths = [\"src/**\"]\n\
             [[norms]]\nid = \"b\"\nlook_for = \"y\"\nseverity = \"blocking\"\npaths = [\"*.py\"]\n",
        )
        .unwrap();
        assert_eq!(norms.len(), 2);
        assert_eq!(
            norms[0].severity,
            Severity::Suggestion,
            "de serie solo avisa"
        );
        assert_eq!(norms[1].severity, Severity::Blocking);
        assert!(parse("").unwrap().is_empty());
        for (bad, why) in [
            (
                "[[norms]]\nid = \"a\"\nlook_for = \"x\"\npaths = []\n",
                "paths",
            ),
            ("[[norms]]\nid = \"a\"\npaths = [\"**\"]\n", "look_for"),
            (
                "[[norms]]\nid = \"a\"\nlook_for = \"x\"\npaths = [\"**\"]\nseverity = \"fatal\"\n",
                "fatal",
            ),
            (
                "[[norms]]\nid = \"a\"\nlook_for = \"x\"\npaths = [\"**\"]\nglobs = []\n",
                "globs",
            ),
            (
                "[[norms]]\nid = \"a\"\nlook_for = \"x\"\npaths = [\"**\"]\n[[norms]]\nid = \"a\"\nlook_for = \"y\"\npaths = [\"**\"]\n",
                "\"a\"",
            ),
        ] {
            let e = parse(bad).unwrap_err();
            assert!(e.contains(why), "{why}: {e}");
        }
    }

    #[test]
    fn only_norms_whose_paths_the_change_touches_apply() {
        let norms = [
            norm("api", Severity::Blocking, &["src/api/**"]),
            norm("py", Severity::Suggestion, &["*.py"]),
        ];
        let files = ["src/api/users.ts".to_string(), "lib/util.ts".to_string()];
        let found = applicable(&norms, &files);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "api");
        let role = role(&found, &files);
        assert!(
            role.contains("[api] (blocking) src/api/users.ts: look for api"),
            "{role}"
        );
        assert!(!role.contains("lib/util.ts"), "solo sus archivos: {role}");
        assert!(applicable(&norms, &["docs/a.md".into()]).is_empty());
    }

    #[test]
    fn only_a_blocking_norm_blocks() {
        let norms = [
            norm("hard", Severity::Blocking, &["**"]),
            norm("soft", Severity::Suggestion, &["**"]),
        ];
        let f = |severity: &str, norm: &str| Finding {
            severity: severity.into(),
            norm: norm.into(),
            ..Default::default()
        };
        let settled = settle(
            vec![
                f("blocking", "[hard]"),
                f("blocking", "soft"),
                f("blocking", ""),
                f("warning", "soft"),
            ],
            &norms,
        );
        let got: Vec<(&str, &str)> = settled
            .iter()
            .map(|f| (f.severity.as_str(), f.norm.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("blocking", "hard"),
                ("warning", "soft"),
                ("warning", ""),
                ("warning", "soft")
            ]
        );
    }

    #[test]
    fn a_changed_norm_changes_the_fingerprint() {
        let a = [norm("x", Severity::Blocking, &["**"])];
        let mut b = a.clone();
        assert_eq!(fingerprint(&a), fingerprint(&b));
        b[0].look_for.push('!');
        assert_ne!(fingerprint(&a), fingerprint(&b));
        b[0] = a[0].clone();
        b[0].severity = Severity::Suggestion;
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn the_starter_template_parses_and_is_written_once() {
        for lang in [crate::i18n::Lang::Es, crate::i18n::Lang::En] {
            let text = crate::i18n::translate(lang, TEMPLATE);
            let norms = parse(text).unwrap();
            let ids: Vec<&str> = norms.iter().map(|n| n.id.as_str()).collect();
            assert_eq!(
                ids,
                [
                    "no-secrets-in-code",
                    "no-sensitive-data-in-logs",
                    "auth-on-endpoints",
                    "parameterized-sql"
                ]
            );
            if lang == crate::i18n::Lang::En {
                assert_ne!(text, TEMPLATE, "falta la traducción en i18n_en.rs");
            }
        }
        let dir = std::env::temp_dir().join(format!("forge-norms-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = write_template(&dir).unwrap();
        assert_eq!(load(&dir).unwrap().len(), 4);
        assert!(write_template(&dir).is_err(), "no pisa lo que haya");
        assert!(file.ends_with(".forge/norms.toml"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
