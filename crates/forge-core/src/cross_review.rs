// Revisión por otra empresa (plan #84): qué agente hizo una fase, de qué empresa es, y qué
// agente instalado de otra empresa debería revisarla. Solo elige y explica: abrir la
// revisión sigue siendo el botón «Revisar».
use std::path::Path;

use crate::activity::Activity;
use crate::agents::AgentSpec;
use crate::store::ActiveAgentSession;
use crate::worktrees::Record;

/// Quién hace el agente (el modelo de serie que trae). Modelos de empresas distintas fallan en
/// cosas distintas: por eso el revisor es de otra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Company {
    Anthropic,
    OpenAI,
    Google,
    Alibaba,
    /// OpenCode (Anomaly, antes SST): proyecto propio que puede usar modelos de cualquiera.
    OpenCode,
    /// Pi (pi-coding-agent): proyecto independiente, también con cualquier modelo.
    Pi,
}

/// La empresa de cada agente que Forge sabe lanzar (`agents::builtins`), en un solo sitio.
/// Un agente nuevo sin fila aquí hace fallar `every_launchable_agent_has_a_company`.
const COMPANIES: &[(&str, Company)] = &[
    ("claude", Company::Anthropic),
    ("codex", Company::OpenAI),
    ("gemini", Company::Google),
    ("agy", Company::Google),
    ("qwen", Company::Alibaba),
    ("opencode", Company::OpenCode),
    ("pi", Company::Pi),
];

/// La empresa de un agente por su id (`claude`, `codex`…). `None` para agentes propios de
/// `agents.toml` que Forge no conoce.
pub fn company(agent: &str) -> Option<Company> {
    COMPANIES
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, c)| *c)
}

/// De dónde se sabe quién hizo la fase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// El registro de su worktree (`forge worktree new`: quién lo pidió).
    Worktree,
    /// La terminal de Forge donde trabaja (la del worktree, o una abierta en su rama).
    Terminal,
    /// Lo que editó en su worktree (`agent_activity`).
    Activity,
    /// Quien marcó la fase en curso en el plan (cliente MCP).
    Plan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Author {
    /// Id del agente (`claude`, `codex`…).
    pub agent: String,
    pub source: Source,
}

/// Lo que Forge ya sabe de los agentes del proyecto.
#[derive(Default)]
pub struct Evidence<'a> {
    pub records: &'a [Record],
    pub sessions: &'a [ActiveAgentSession],
    pub activity: &'a [Activity],
}

/// El id de un agente a partir de cómo se le nombra: su id, su nombre («Claude Code»,
/// `FORGE_ORIGIN`) o el nombre de su cliente MCP («claude-code», «codex-mcp-client»).
pub fn agent_id_of(text: &str, agents: &[AgentSpec]) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    agents
        .iter()
        .find(|a| a.id == text || a.name.eq_ignore_ascii_case(text))
        .map(|a| a.id.clone())
        .or_else(|| crate::ideas::client_agent_id(text).map(String::from))
}

/// Qué agente hizo la fase de `branch`, por orden: el registro de su worktree, la terminal de
/// ese worktree, lo que se editó en él, quién la marcó en curso (`worked_by`) y, por último,
/// una terminal abierta en la rama (la más floja: la del revisor también puede estar en ella).
pub fn author(
    branch: Option<&str>,
    worked_by: Option<&str>,
    evidence: &Evidence,
    agents: &[AgentSpec],
) -> Option<Author> {
    let id = |text: &str| agent_id_of(text, agents);
    let found = |source| move |agent| Author { agent, source };
    let record = branch.and_then(|b| evidence.records.iter().find(|r| r.branch == b));
    let terminal = |session: &str| {
        evidence
            .sessions
            .iter()
            .find(|s| s.id == session)
            .and_then(|s| id(&s.agent))
    };
    let edited_in = |root: &Path| {
        let root = crate::canonical(root);
        evidence
            .activity
            .iter()
            .filter(|a| !a.files.is_empty())
            .filter(|a| {
                a.root
                    .as_deref()
                    .is_some_and(|r| crate::canonical(r) == root)
            })
            .max_by_key(|a| a.updated_at)
            .and_then(|a| id(&a.agent))
    };
    record
        .and_then(|r| r.origin.as_deref().and_then(id))
        .map(found(Source::Worktree))
        .or_else(|| {
            record
                .and_then(|r| r.session.as_deref().and_then(terminal))
                .map(found(Source::Terminal))
        })
        .or_else(|| {
            record
                .and_then(|r| edited_in(&r.path))
                .map(found(Source::Activity))
        })
        .or_else(|| worked_by.and_then(id).map(found(Source::Plan)))
        .or_else(|| {
            let branch = branch?;
            evidence
                .sessions
                .iter()
                .filter(|s| s.branch == branch)
                .find_map(|s| id(&s.agent))
                .map(found(Source::Terminal))
        })
}

/// Por qué se eligió ese revisor (para mostrarlo).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// De otra empresa que el autor (su id).
    OtherCompany(String),
    /// Es el único agente instalado que puede revisar.
    OnlyAgent,
    /// No hay ninguno de otra empresa que el autor (su id).
    NoOtherCompany(String),
    /// No se sabe qué agente hizo la fase.
    UnknownAuthor,
    /// Se sabe quién (su id), pero no de qué empresa es (un agente propio).
    UnknownCompany(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub agent: String,
    pub why: Why,
}

/// El revisor de una fase. `candidates`: los agentes instalados que pueden revisar, en orden
/// estable (el de la lista de agentes); `in_use`: los que el dueño usa, el más usado primero
/// (la terminal enfocada, las otras abiertas, el de serie). Determinista: mismas entradas,
/// mismo revisor. Nunca elige uno que no esté en `candidates`.
pub fn choose(author: Option<&str>, candidates: &[&str], in_use: &[&str]) -> Option<Choice> {
    // De una lista, el que el dueño usa (por su orden); si no usa ninguno, el primero.
    let preferred = |list: &[&str]| -> Option<String> {
        in_use
            .iter()
            .find(|u| list.contains(u))
            .or(list.first())
            .map(|a| a.to_string())
    };
    let pick = |list: &[&str], why| preferred(list).map(|agent| Choice { agent, why });
    if candidates.len() == 1 {
        return pick(candidates, Why::OnlyAgent);
    }
    let Some(author) = author else {
        return pick(candidates, Why::UnknownAuthor);
    };
    let Some(theirs) = company(author) else {
        return pick(candidates, Why::UnknownCompany(author.into()));
    };
    let others: Vec<&str> = candidates
        .iter()
        .copied()
        .filter(|c| company(c).is_some_and(|c| c != theirs))
        .collect();
    if !others.is_empty() {
        return pick(&others, Why::OtherCompany(author.into()));
    }
    // Ninguno de otra empresa: el mismo autor, como hasta ahora (si puede revisar).
    let why = Why::NoOtherCompany(author.into());
    if candidates.contains(&author) {
        return Some(Choice {
            agent: author.into(),
            why,
        });
    }
    pick(candidates, why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn every_launchable_agent_has_a_company() {
        for agent in crate::agents::builtins() {
            assert!(
                company(&agent.id).is_some(),
                "«{}» no tiene empresa en COMPANIES",
                agent.id
            );
        }
    }

    #[test]
    fn work_by_claude_is_reviewed_by_an_installed_agent_of_another_company() {
        let choice = choose(Some("claude"), &["claude", "codex"], &["claude"]).unwrap();
        assert_eq!(choice.agent, "codex");
        assert_eq!(choice.why, Why::OtherCompany("claude".into()));
        // Antigravity y Gemini son de la misma empresa: no se revisan entre ellos.
        let choice = choose(Some("agy"), &["gemini", "agy", "qwen"], &[]).unwrap();
        assert_eq!(choice.agent, "qwen");
    }

    #[test]
    fn with_a_single_agent_installed_it_reviews_its_own_work() {
        let choice = choose(Some("claude"), &["claude"], &["claude"]).unwrap();
        assert_eq!(
            choice,
            Choice {
                agent: "claude".into(),
                why: Why::OnlyAgent
            }
        );
        // Varios, pero todos de la misma empresa: el mismo autor, como hoy.
        let choice = choose(Some("agy"), &["gemini", "agy"], &["gemini"]).unwrap();
        assert_eq!(choice.agent, "agy");
        assert_eq!(choice.why, Why::NoOtherCompany("agy".into()));
        assert_eq!(choose(Some("claude"), &[], &["claude"]), None);
    }

    #[test]
    fn an_unknown_author_gets_the_agent_in_use() {
        let choice = choose(None, &["claude", "codex"], &["codex"]).unwrap();
        assert_eq!(choice.agent, "codex");
        assert_eq!(choice.why, Why::UnknownAuthor);
        // Sin ninguno en uso: el primero de la lista.
        assert_eq!(
            choose(None, &["claude", "codex"], &[]).unwrap().agent,
            "claude"
        );
        // Un agente propio de agents.toml: no se sabe su empresa.
        let choice = choose(Some("mio"), &["claude", "codex"], &[]).unwrap();
        assert_eq!(choice.agent, "claude");
        assert_eq!(choice.why, Why::UnknownCompany("mio".into()));
    }

    #[test]
    fn between_two_other_companies_it_prefers_the_one_in_use_and_is_stable() {
        let candidates = ["claude", "codex", "gemini"];
        // Ninguno en uso: el primero de la lista, siempre.
        for _ in 0..3 {
            assert_eq!(
                choose(Some("claude"), &candidates, &[]).unwrap().agent,
                "codex"
            );
        }
        // El que el dueño usa gana al orden de la lista (el autor en uso no cuenta).
        assert_eq!(
            choose(Some("claude"), &candidates, &["claude", "gemini"])
                .unwrap()
                .agent,
            "gemini"
        );
    }

    #[test]
    fn it_never_chooses_an_agent_that_is_not_installed() {
        // Codex está en uso (el de serie) pero no instalado: no es candidato.
        let choice = choose(Some("claude"), &["claude", "gemini"], &["codex"]).unwrap();
        assert_eq!(choice.agent, "gemini");
        let choice = choose(None, &["claude"], &["codex"]).unwrap();
        assert_eq!(choice.agent, "claude");
    }

    fn record(branch: &str, origin: Option<&str>, session: Option<&str>) -> Record {
        Record {
            path: PathBuf::from(format!("/wt/{}", branch.replace('/', "-"))),
            branch: branch.into(),
            origin: origin.map(String::from),
            session: session.map(String::from),
            ..Default::default()
        }
    }

    fn session(id: &str, branch: &str, agent: &str) -> ActiveAgentSession {
        ActiveAgentSession {
            id: id.into(),
            project_key: String::new(),
            branch: branch.into(),
            agent: agent.into(),
            pid: None,
            updated_at: 0,
        }
    }

    #[test]
    fn the_author_comes_from_the_worktree_then_its_terminal_then_its_edits_then_the_plan() {
        let agents = crate::agents::builtins();
        let by = |records: &[Record], sessions: &[ActiveAgentSession], activity: &[Activity]| {
            let ev = Evidence {
                records,
                sessions,
                activity,
            };
            author(Some("feat/x"), Some("qwen-code"), &ev, &agents)
        };
        let edits = Activity {
            agent: "Gemini CLI".into(),
            root: Some(PathBuf::from("/wt/feat-x")),
            files: vec!["src/a.rs".into()],
            ..Default::default()
        };
        let sessions = [session("s1", "feat/x", "OpenCode")];
        // El registro del worktree dice quién lo pidió («Claude Code», su FORGE_ORIGIN).
        let records = [record("feat/x", Some("Claude Code"), Some("s1"))];
        assert_eq!(
            by(&records, &sessions, std::slice::from_ref(&edits)),
            Some(Author {
                agent: "claude".into(),
                source: Source::Worktree
            })
        );
        // Sin origen: la terminal del worktree.
        let records = [record("feat/x", None, Some("s1"))];
        assert_eq!(
            by(&records, &sessions, std::slice::from_ref(&edits)),
            Some(Author {
                agent: "opencode".into(),
                source: Source::Terminal
            })
        );
        // Sin terminal: quien editó en el worktree.
        let records = [record("feat/x", Some("proceso dev"), None)];
        assert_eq!(
            by(&records, &[], std::slice::from_ref(&edits)),
            Some(Author {
                agent: "gemini".into(),
                source: Source::Activity
            })
        );
        // Sin worktree registrado: quien marcó la fase en curso.
        assert_eq!(
            by(&[], &sessions, &[]),
            Some(Author {
                agent: "qwen".into(),
                source: Source::Plan
            })
        );
        // Y sin nada de eso, una terminal abierta en la rama.
        let ev = Evidence {
            sessions: &sessions,
            ..Default::default()
        };
        assert_eq!(
            author(Some("feat/x"), None, &ev, &agents),
            Some(Author {
                agent: "opencode".into(),
                source: Source::Terminal
            })
        );
    }

    #[test]
    fn an_unknown_author_does_not_break_anything() {
        let agents = crate::agents::builtins();
        let records = [record("feat/x", Some("proceso dev"), None)];
        let ev = Evidence {
            records: &records,
            ..Default::default()
        };
        assert_eq!(author(Some("feat/x"), Some("  "), &ev, &agents), None);
        assert_eq!(author(None, None, &Evidence::default(), &agents), None);
        // Otra rama: lo de esa no cuenta.
        assert_eq!(author(Some("feat/y"), None, &ev, &agents), None);
    }

    #[test]
    fn edits_of_a_worktree_match_its_folder_even_written_differently() {
        let agents = crate::agents::builtins();
        let dir = std::env::temp_dir().join(format!("forge-xr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let records = [Record {
            path: dir.clone(),
            branch: "feat/x".into(),
            ..Default::default()
        }];
        // Sin editar nada (un revisor solo lee): no cuenta como autor.
        let reading = Activity {
            agent: "Codex".into(),
            root: Some(crate::canonical(&dir)),
            ..Default::default()
        };
        let editing = Activity {
            agent: "Claude Code".into(),
            root: Some(crate::canonical(&dir)),
            files: vec!["a".into()],
            ..Default::default()
        };
        let activity = [reading, editing];
        let ev = Evidence {
            records: &records,
            activity: &activity,
            ..Default::default()
        };
        assert_eq!(
            author(Some("feat/x"), None, &ev, &agents).map(|a| a.agent),
            Some("claude".into())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
