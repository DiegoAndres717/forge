// Catálogo único de agentes (plan #202, fase 1): todo lo que Forge sabe de cada agente que
// sabe lanzar, en una sola entrada. Añadir un agente nuevo es añadir una fila a `CATALOG`
// (y su icono, si tiene; si no, va su letra). `agents::builtins`, la empresa de la revisión
// cruzada, el logo de la barra lateral y la fuente del estado en vivo salen de aquí.
use crate::agents::McpStyle;

/// Quién hace el agente (el modelo de serie que trae). Modelos de empresas distintas fallan en
/// cosas distintas: por eso el revisor cruzado es de otra.
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

impl Company {
    /// Su nombre (propio: no se traduce).
    pub fn name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAI => "OpenAI",
            Self::Google => "Google",
            Self::Alibaba => "Alibaba",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
        }
    }
}

/// Cómo se le da el primer mensaje al abrirlo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Argumento suelto: `claude "mensaje"`.
    Positional,
    /// Con una opción delante: `opencode --prompt "mensaje"`.
    Flag(&'static str),
    /// No verificado o no admitido: Forge no lo abre con mensaje.
    Unsupported,
}

/// El icono de su fila: un logo propio pequeño (`assets/agents/<id>.png` o dibujado) o, si no
/// hay, su letra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Logo,
    Letter(char),
}

/// De dónde sabe Forge qué hace el agente ahora (su fila en la barra lateral y `work_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    /// Claude Code (mod de Forge): subagentes, herramientas y archivos editados.
    Full,
    /// Codex, Gemini CLI y Antigravity: el hook previo de Guard ve comandos y lecturas, no
    /// las ediciones; los archivos cambiados salen de `git status` de su worktree.
    Commands,
    /// Sin hook que Forge use (OpenCode, Qwen Code, Pi, propios): solo el título y la salida
    /// de su terminal, menos fiable.
    None,
}

/// Un agente que Forge sabe lanzar.
#[derive(Debug)]
pub struct AgentInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub company: Company,
    /// Comando que lo abre; su primera palabra es el programa que se busca en el PATH.
    pub command: &'static str,
    pub resume: Option<&'static str>,
    pub headless: Option<&'static str>,
    pub mcp: bool,
    pub mcp_style: Option<McpStyle>,
    pub install_hint: &'static str,
    pub prompt: Prompt,
    pub icon: Icon,
    pub live: Live,
    /// Entrega sus subagentes (arranque y fin) a Forge.
    pub subagents: bool,
}

pub const CATALOG: &[AgentInfo] = &[
    AgentInfo {
        id: "claude",
        name: "Claude Code",
        company: Company::Anthropic,
        command: "claude",
        resume: Some("claude --continue"),
        headless: Some("claude -p"),
        mcp: true,
        mcp_style: Some(McpStyle::ClaudeJson),
        install_hint: "curl -fsSL https://claude.ai/install.sh | bash",
        prompt: Prompt::Positional,
        icon: Icon::Logo,
        live: Live::Full,
        subagents: true,
    },
    AgentInfo {
        id: "codex",
        name: "Codex",
        company: Company::OpenAI,
        command: "codex",
        resume: Some("codex resume --last"),
        headless: Some("codex exec"),
        mcp: true,
        mcp_style: Some(McpStyle::CodexConfig),
        install_hint: "brew install codex",
        prompt: Prompt::Positional,
        icon: Icon::Logo,
        live: Live::Commands,
        subagents: false,
    },
    AgentInfo {
        id: "opencode",
        name: "OpenCode",
        company: Company::OpenCode,
        command: "opencode",
        resume: Some("opencode --continue"),
        headless: Some("opencode run"),
        mcp: true,
        mcp_style: Some(McpStyle::OpenCodeEnv),
        install_hint: "brew install opencode",
        // Toma el argumento suelto como carpeta.
        prompt: Prompt::Flag("--prompt"),
        icon: Icon::Logo,
        live: Live::None,
        subagents: false,
    },
    // ponytail: opciones de reanudar sin verificar (no instalados aquí); se añaden al comprobarlas.
    AgentInfo {
        id: "gemini",
        name: "Gemini CLI",
        company: Company::Google,
        command: "gemini",
        resume: None,
        headless: Some("gemini -p"),
        mcp: true,
        mcp_style: None,
        install_hint: "npm install -g @google/gemini-cli",
        prompt: Prompt::Unsupported,
        icon: Icon::Logo,
        live: Live::Commands,
        subagents: false,
    },
    AgentInfo {
        id: "qwen",
        name: "Qwen Code",
        company: Company::Alibaba,
        command: "qwen",
        resume: None,
        headless: Some("qwen -p"),
        mcp: true,
        mcp_style: None,
        install_hint: "npm install -g @qwen-code/qwen-code",
        prompt: Prompt::Unsupported,
        icon: Icon::Logo,
        live: Live::None,
        subagents: false,
    },
    AgentInfo {
        id: "pi",
        name: "Pi",
        company: Company::Pi,
        command: "pi",
        resume: None,
        headless: None,
        mcp: false,
        mcp_style: None,
        install_hint: "npm install -g @mariozechner/pi-coding-agent",
        prompt: Prompt::Unsupported,
        icon: Icon::Letter('P'),
        live: Live::None,
        subagents: false,
    },
    AgentInfo {
        id: "agy",
        name: "Antigravity",
        company: Company::Google,
        command: "agy",
        resume: Some("agy --continue"),
        headless: Some("agy -p"),
        mcp: true,
        mcp_style: Some(McpStyle::AgyConfig),
        install_hint: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
        prompt: Prompt::Flag("-i"),
        icon: Icon::Logo,
        live: Live::Commands,
        subagents: false,
    },
];

/// La entrada de un agente por su id (`claude`, `codex`…).
pub fn get(id: &str) -> Option<&'static AgentInfo> {
    CATALOG.iter().find(|a| a.id == id)
}

/// La entrada de un agente por el nombre con que firma su actividad (`FORGE_ORIGIN`, el de su
/// terminal: «Claude Code», «Codex · Trabajo»…) o por su id. `None` para agentes propios.
pub fn by_name(name: &str) -> Option<&'static AgentInfo> {
    // Por palabras enteras: «Pi» no es «Mi agente propio».
    let words = |s: &str| {
        let w: Vec<String> = s
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect();
        format!(" {} ", w.join(" "))
    };
    let hay = words(name);
    CATALOG
        .iter()
        .find(|a| hay.contains(&words(a.name)))
        .or_else(|| CATALOG.iter().find(|a| hay.contains(&words(a.id))))
}

/// El icono de un agente por su id; los propios llevan la inicial de su nombre.
pub fn icon(id: &str, name: &str) -> Icon {
    match get(id) {
        Some(a) => a.icon,
        None => Icon::Letter(letter(name)),
    }
}

/// La inicial en mayúscula (o «?» si no hay letra).
pub fn letter(name: &str) -> char {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map_or('?', |c| c.to_uppercase().next().unwrap_or(c))
}

/// Cómo sabe Forge qué hace un agente, por el nombre con que firma (ver `by_name`).
pub fn live(name: &str) -> Live {
    by_name(name).map_or(Live::None, |a| a.live)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un agente nuevo en `agents::builtins` sin su entrada completa aquí hace fallar esto.
    #[test]
    fn every_launchable_agent_has_its_icon_company_and_live_source() {
        for agent in crate::agents::builtins() {
            let info = get(&agent.id)
                .unwrap_or_else(|| panic!("«{}» no tiene entrada en CATALOG", agent.id));
            assert!(!info.name.is_empty(), "«{}» sin nombre", agent.id);
            assert!(
                !info.company.name().is_empty(),
                "«{}» sin empresa",
                agent.id
            );
            if let Icon::Letter(c) = info.icon {
                assert!(
                    c.is_alphanumeric(),
                    "«{}»: letra de respaldo vacía",
                    agent.id
                );
            }
            assert_eq!(
                info.subagents,
                info.live == Live::Full,
                "«{}»: solo con hooks completos llegan subagentes",
                agent.id
            );
        }
        assert_eq!(CATALOG.len(), crate::agents::builtins().len());
    }

    #[test]
    fn agents_are_found_by_the_name_they_sign_with() {
        assert_eq!(by_name("Claude Code").map(|a| a.id), Some("claude"));
        assert_eq!(by_name("Codex · Trabajo").map(|a| a.id), Some("codex"));
        assert_eq!(by_name("antigravity").map(|a| a.id), Some("agy"));
        assert!(by_name("Mi agente propio").is_none());
        assert_eq!(by_name("pi").map(|a| a.id), Some("pi"));
        assert_eq!(live("Claude Code"), Live::Full);
        assert_eq!(live("Codex"), Live::Commands);
        assert_eq!(live("Qwen Code"), Live::None);
        assert_eq!(icon("mio", "agente propio"), Icon::Letter('A'));
        assert_eq!(icon("pi", "Pi"), Icon::Letter('P'));
    }
}
