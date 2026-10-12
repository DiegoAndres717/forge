// Subagentes baratos para Codex y Gemini CLI: el explorador y el revisor de Forge, como los
// que el mod registra en Claude Code (claude-plugin/hooks/register.tsx, mismos textos). Solo
// se escriben si el usuario lo pide en Ajustes: son carpetas suyas (~/.codex, ~/.gemini).
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// (nombre, descripción, instrucciones, esfuerzo en Codex).
const AGENTS: [(&str, &str, &str, &str); 2] = [
    (
        "forge-explorer",
        "Fast, cheap code explorer: where things are, what code does, finding files, symbols and call sites, reading and summarizing several files. Returns a short answer with paths. Use it whenever understanding something takes more than two searches or reads.",
        "You are a fast code explorer working for a senior engineer.
Answer exactly what you were asked about the codebase: where something lives, what a
piece of code does, which files are involved, how something is wired.
- Search first; read only the files that matter.
- Reply briefly: the answer first, then file paths with line numbers (path:line).
- Do not modify anything. Do not propose changes unless asked.",
        "low",
    ),
    (
        "forge-reviewer",
        "Runs full test suites, type checks and builds and explains failures; reviews a finished change before committing. Does not edit files. Use it instead of running whole suites yourself.",
        "You are a careful reviewer working for a senior engineer.
Run what you are asked (tests, type checks, linters, builds), read the failures and
report the cause, not the full log.
- Quote only the relevant lines of output.
- When reviewing a change, list concrete problems with path:line and why they matter.
- Do not modify files.",
        "medium",
    ),
];

/// Gemini: el modelo rápido.
// ponytail: id fijo; si Google lo retira, cambiarlo aquí (o leerlo de `gemini` si un día
// publica su catálogo como `codex debug models`).
const GEMINI_MODEL: &str = "gemini-3-flash-preview";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Agent {
    Codex,
    Gemini,
}

impl Agent {
    /// Carpeta de agentes: `~/.codex/agents` o `~/.gemini/agents`.
    pub fn dir(self, home: &Path) -> PathBuf {
        home.join(match self {
            Agent::Codex => ".codex",
            Agent::Gemini => ".gemini",
        })
        .join("agents")
    }

    fn file(self, name: &str) -> String {
        match self {
            Agent::Codex => format!("{name}.toml"),
            Agent::Gemini => format!("{name}.md"),
        }
    }
}

/// El modelo barato del catálogo de Codex (`codex debug models`): el primero visible cuya
/// descripción lo diga («fast», «affordable»…). Sin catálogo, None: hereda el modelo.
pub fn cheap_codex_model(catalog: &str) -> Option<String> {
    let catalog: Value = serde_json::from_str(catalog).ok()?;
    catalog["models"].as_array()?.iter().find_map(|m| {
        let desc = m["description"].as_str()?.to_lowercase();
        let cheap = ["affordable", "fast", "efficient"]
            .iter()
            .any(|w| desc.contains(w));
        (m["visibility"].as_str() == Some("list") && cheap && !desc.contains("older"))
            .then(|| m["slug"].as_str().map(String::from))?
    })
}

/// Los archivos de cada subagente: (nombre de archivo, contenido).
pub fn files(agent: Agent, codex_model: Option<&str>) -> Vec<(String, String)> {
    AGENTS
        .iter()
        .map(|(name, description, prompt, effort)| {
            let text = match agent {
                // Las cadenas van como JSON, que también es una cadena TOML válida.
                Agent::Codex => {
                    let model = codex_model
                        .map(|m| format!("model = {}\n", json!(m)))
                        .unwrap_or_default();
                    // El explorador no necesita escribir nada; el revisor sí (builds, tests).
                    let sandbox = if *name == "forge-explorer" {
                        "sandbox_mode = \"read-only\"\n"
                    } else {
                        ""
                    };
                    format!(
                        "name = {}\ndescription = {}\n{model}model_reasoning_effort = {}\n{sandbox}developer_instructions = {}\n",
                        json!(name),
                        json!(description),
                        json!(effort),
                        json!(prompt),
                    )
                }
                Agent::Gemini => format!(
                    "---\nname: {name}\ndescription: {}\nmodel: {GEMINI_MODEL}\n---\n\n{prompt}\n",
                    json!(description)
                ),
            };
            (agent.file(name), text)
        })
        .collect()
}

pub fn install(agent: Agent, home: &Path, codex_model: Option<&str>) -> Result<(), String> {
    let dir = agent.dir(home);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (file, text) in files(agent, codex_model) {
        std::fs::write(dir.join(file), text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Borra solo los archivos de Forge.
pub fn remove(agent: Agent, home: &Path) -> Result<(), String> {
    for (name, ..) in AGENTS {
        let path = agent.dir(home).join(agent.file(name));
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn installed(agent: Agent, home: &Path) -> bool {
    AGENTS
        .iter()
        .all(|(name, ..)| agent.dir(home).join(agent.file(name)).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_gets_the_cheap_model_of_its_catalog() {
        let catalog = r#"{"models":[
            {"slug":"gpt-6-sol","description":"Most capable model.","visibility":"list"},
            {"slug":"gpt-reserve","description":"Fast and affordable.","visibility":"hide"},
            {"slug":"gpt-5.6-luna","description":"Older fast and efficient model.","visibility":"list"},
            {"slug":"gpt-6-luna","description":"Fast and affordable model for easier tasks.","visibility":"list"}]}"#;
        assert_eq!(cheap_codex_model(catalog).as_deref(), Some("gpt-6-luna"));
        assert_eq!(cheap_codex_model("no es json"), None);
    }

    #[test]
    fn subagents_are_written_and_removed_without_touching_the_rest() {
        let home = std::env::temp_dir().join(format!("forge-subagents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mine = Agent::Codex.dir(&home).join("mine.toml");
        std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
        std::fs::write(&mine, "name = \"mine\"\n").unwrap();
        for agent in [Agent::Codex, Agent::Gemini] {
            assert!(!installed(agent, &home));
            install(agent, &home, Some("gpt-6-luna")).unwrap();
            assert!(installed(agent, &home));
        }
        let codex =
            std::fs::read_to_string(Agent::Codex.dir(&home).join("forge-reviewer.toml")).unwrap();
        let toml: toml::Table = codex.parse().unwrap();
        assert_eq!(toml["model"].as_str(), Some("gpt-6-luna"));
        assert_eq!(toml["model_reasoning_effort"].as_str(), Some("medium"));
        assert!(
            toml.get("sandbox_mode").is_none(),
            "el revisor corre builds"
        );
        assert!(
            toml["developer_instructions"]
                .as_str()
                .unwrap()
                .contains("Do not modify files.")
        );
        let gemini =
            std::fs::read_to_string(Agent::Gemini.dir(&home).join("forge-explorer.md")).unwrap();
        assert!(gemini.starts_with("---\nname: forge-explorer\n"));
        assert!(gemini.contains(&format!("model: {GEMINI_MODEL}\n")));
        for agent in [Agent::Codex, Agent::Gemini] {
            remove(agent, &home).unwrap();
            assert!(!installed(agent, &home));
        }
        assert!(mine.exists(), "los subagentes del usuario no se tocan");
        std::fs::remove_dir_all(home).unwrap();
    }
}
