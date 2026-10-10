// Adaptadores de agentes: qué agentes hay, cómo se abren, reanudan y detectan.
// Cada agente es un dato (comando, reanudar, modo no interactivo); `.forge/agents.toml`
// puede cambiarlos o añadir agentes propios.
use crate::tr;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    /// Comando para abrirlo (puede llevar opciones: `claude --model opus`).
    pub command: String,
    /// Comando para reanudar la última sesión en la carpeta, si el agente lo permite.
    pub resume: Option<String>,
    /// Modo no interactivo (prompt → respuesta), para revisores automáticos.
    pub headless: Option<String>,
    pub mcp: bool,
    /// Cómo se le conecta un servidor MCP por línea de comandos (si se puede sin tocar su configuración).
    pub mcp_style: Option<McpStyle>,
    pub install_hint: Option<String>,
    pub enabled: bool,
    pub default: bool,
    /// Listado en agents.toml: se muestra aunque no esté instalado.
    pub configured: bool,
    pub environment: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum McpStyle {
    /// `claude --mcp-config '<json>'`
    ClaudeJson,
    /// `codex -c mcp_servers.<n>.command=… -c mcp_servers.<n>.args=[…]`
    CodexConfig,
    /// `OPENCODE_CONFIG_CONTENT='{"mcp":{…}}' opencode --standalone`: la configuración se
    /// suma a la del usuario sin tocarla; `--standalone` porque el servicio de fondo de
    /// OpenCode no ve el entorno de esta terminal.
    OpenCodeEnv,
}

/// Servidor MCP que se conecta a los agentes (la memoria de Forge u otro proveedor).
#[derive(Clone, Debug, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl AgentSpec {
    /// Comando con el servidor MCP conectado (los flags van justo tras el programa,
    /// así sirven también para subcomandos como `codex resume --last`).
    pub fn with_mcp(&self, command: &str, server: &McpServer) -> String {
        let Some(style) = self.mcp_style else {
            return command.to_string();
        };
        let flags = match style {
            McpStyle::ClaudeJson => {
                let config = serde_json::json!({"mcpServers": {&server.name: {"command": server.command, "args": server.args}}});
                format!("--mcp-config {}", quote(&config.to_string()))
            }
            McpStyle::OpenCodeEnv => {
                let mut command_line = vec![server.command.clone()];
                command_line.extend(server.args.iter().cloned());
                let config = serde_json::json!({"mcp": {&server.name: {"type": "local", "command": command_line, "enabled": true}}});
                let flagged = match command.split_once(' ') {
                    Some((program, rest)) => format!("{program} --standalone {rest}"),
                    None => format!("{command} --standalone"),
                };
                return format!(
                    "OPENCODE_CONFIG_CONTENT={} {flagged}",
                    quote(&config.to_string())
                );
            }
            McpStyle::CodexConfig => {
                // Cadenas TOML con escapes compatibles con JSON.
                let command = serde_json::to_string(&server.command).unwrap_or_default();
                let args = serde_json::to_string(&server.args).unwrap_or_default();
                format!(
                    "-c {} -c {}",
                    quote(&format!("mcp_servers.{}.command={command}", server.name)),
                    quote(&format!("mcp_servers.{}.args={args}", server.name))
                )
            }
        };
        match command.split_once(' ') {
            Some((program, rest)) => format!("{program} {flags} {rest}"),
            None => format!("{command} {flags}"),
        }
    }

    /// Añade los avisos del agente a Forge: hooks `Stop`/`Notification` en Claude Code y
    /// `notify` en Codex, que llaman a `forge agent-event`. Otros agentes, sin cambios.
    /// `has_mod`: el mod de Forge para Claude Code se carga en las terminales de Forge
    /// (CLAUDE_CODE_PLUGIN_DIRS) y avisa él mismo; sin él, hooks de Claude por --settings.
    pub fn with_events(&self, command: &str, forge: &str, has_mod: bool) -> String {
        let flags = match self.mcp_style {
            Some(McpStyle::ClaudeJson) if has_mod => return command.to_string(),
            Some(McpStyle::ClaudeJson) => {
                let hook = |kind: &str| serde_json::json!([{"hooks": [{"type": "command", "command": format!("{} agent-event {kind}", quote(forge))}]}]);
                let settings = serde_json::json!({"hooks": {"Stop": hook("stop"), "Notification": hook("waiting")}});
                format!("--settings {}", quote(&settings.to_string()))
            }
            Some(McpStyle::CodexConfig) => {
                let notify =
                    serde_json::to_string(&[forge, "agent-event", "stop"]).unwrap_or_default();
                format!("-c {}", quote(&format!("notify={notify}")))
            }
            // OpenCode no tiene hooks de aviso: Forge lo detecta por la salida.
            Some(McpStyle::OpenCodeEnv) | None => return command.to_string(),
        };
        match command.split_once(' ') {
            Some((program, rest)) => format!("{program} {flags} {rest}"),
            None => format!("{command} {flags}"),
        }
    }

    /// Abre Claude con el modelo elegido en Ajustes (`sonnet`, `opus`, `opusplan`), salvo
    /// que el comando ya diga uno. Otros agentes o sin elección: sin cambios.
    pub fn with_model(&self, command: &str, model: &str) -> String {
        if self.mcp_style != Some(McpStyle::ClaudeJson)
            || model.is_empty()
            || command.contains("--model")
        {
            return command.to_string();
        }
        match command.split_once(' ') {
            Some((program, rest)) => format!("{program} --model {} {rest}", quote(model)),
            None => format!("{command} --model {}", quote(model)),
        }
    }

    /// El agente avisa él mismo cuando termina o espera (ver `with_events`).
    /// Comando que abre el agente con `prompt` como primer mensaje: argumento suelto
    /// (Claude Code, Codex, agentes propios), `--prompt` en OpenCode (que toma el argumento
    /// suelto como carpeta); `None` en los integrados sin verificar.
    pub fn with_prompt(&self, command: &str, prompt: &str) -> Option<String> {
        match self.mcp_style {
            Some(McpStyle::OpenCodeEnv) => Some(format!("{command} --prompt {}", quote(prompt))),
            None if matches!(self.id.as_str(), "gemini" | "qwen" | "pi") => None,
            _ => Some(format!("{command} {}", quote(prompt))),
        }
    }

    pub fn sends_events(&self) -> bool {
        matches!(
            self.mcp_style,
            Some(McpStyle::ClaudeJson | McpStyle::CodexConfig)
        )
    }

    /// Programa a buscar en el PATH (primera palabra del comando).
    pub fn program(&self) -> &str {
        self.command.split_whitespace().next().unwrap_or("")
    }

    pub fn capabilities(&self) -> Vec<String> {
        let mut caps = Vec::new();
        if let Some(r) = &self.resume {
            caps.push(format!("reanuda sesiones ({r})"));
        }
        if let Some(h) = &self.headless {
            caps.push(tr!("modo no interactivo ({h})", h = h));
        }
        if self.mcp {
            caps.push("servidores MCP".into());
        }
        caps
    }
}

/// Plantilla comentada de `.forge/agents.toml`.
pub fn template() -> String {
    r#"# Agentes del proyecto. Cambia los que Forge ya conoce o añade los tuyos.
# Conocidos: claude, codex, opencode, gemini, qwen, pi.

# Abrir Claude Code con un modelo concreto y que sea el predeterminado (⌘⇧A):
# [[agents]]
# id = "claude"
# command = "claude --model opus"
# default = true

# Ocultar un agente que no usas:
# [[agents]]
# id = "opencode"
# enabled = false

# Un agente propio:
# [[agents]]
# id = "mi-agente"
# name = "Mi agente"
# command = "mi-agente --interactivo"
# resume_command = "mi-agente --continuar"   # opcional
# [agents.environment]
# MI_VARIABLE = "valor"
"#
    .into()
}

/// Agentes conocidos. Las opciones de reanudar y no interactivas se comprobaron con
/// `--help` de cada CLI; las que no se pudieron comprobar se dejan vacías.
pub fn builtins() -> Vec<AgentSpec> {
    let agent = |id: &str,
                 name: &str,
                 command: &str,
                 resume: Option<&str>,
                 headless: Option<&str>,
                 mcp: bool,
                 hint: &str| AgentSpec {
        id: id.into(),
        name: name.into(),
        command: command.into(),
        resume: resume.map(String::from),
        headless: headless.map(String::from),
        mcp,
        mcp_style: match id {
            "claude" => Some(McpStyle::ClaudeJson),
            "codex" => Some(McpStyle::CodexConfig),
            "opencode" => Some(McpStyle::OpenCodeEnv),
            _ => None,
        },
        install_hint: Some(hint.into()),
        enabled: true,
        default: false,
        configured: false,
        environment: HashMap::new(),
    };
    vec![
        agent(
            "claude",
            "Claude Code",
            "claude",
            Some("claude --continue"),
            Some("claude -p"),
            true,
            "curl -fsSL https://claude.ai/install.sh | bash",
        ),
        agent(
            "codex",
            "Codex",
            "codex",
            Some("codex resume --last"),
            Some("codex exec"),
            true,
            "brew install codex",
        ),
        agent(
            "opencode",
            "OpenCode",
            "opencode",
            Some("opencode --continue"),
            Some("opencode run"),
            true,
            "brew install opencode",
        ),
        // ponytail: opciones de reanudar sin verificar (no instalados aquí); se añaden al comprobarlas.
        agent(
            "gemini",
            "Gemini CLI",
            "gemini",
            None,
            Some("gemini -p"),
            true,
            "npm install -g @google/gemini-cli",
        ),
        agent(
            "qwen",
            "Qwen Code",
            "qwen",
            None,
            Some("qwen -p"),
            true,
            "npm install -g @qwen-code/qwen-code",
        ),
        agent(
            "pi",
            "Pi",
            "pi",
            None,
            None,
            false,
            "npm install -g @mariozechner/pi-coding-agent",
        ),
    ]
}

/// `.forge/agents.toml` (formato del plan 9.3).
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct AgentsFile {
    agents: Vec<AgentEntry>,
    /// Revisores especializados (Fase 11): se aceptan ya para no romper la configuración.
    #[serde(rename = "reviewers")]
    _reviewers: Vec<toml::Table>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentEntry {
    id: String,
    name: Option<String>,
    command: Option<String>,
    resume_command: Option<String>,
    headless_command: Option<String>,
    enabled: Option<bool>,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    environment: HashMap<String, String>,
}

/// Agentes del proyecto: los conocidos con los cambios de agents.toml, más los propios.
pub fn load(project: &Path) -> Result<Vec<AgentSpec>, String> {
    let path = project.join(".forge/agents.toml");
    let file: AgentsFile = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => AgentsFile::default(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
    };
    let mut agents = builtins();
    for entry in file.agents {
        let pos = agents.iter().position(|a| a.id == entry.id);
        let agent = match pos {
            Some(i) => &mut agents[i],
            None => {
                let Some(command) = entry.command.clone() else {
                    return Err(tr!(
                        "{p0}: el agente \"{p1}\" necesita `command`",
                        p0 = path.display(),
                        p1 = entry.id
                    ));
                };
                agents.push(AgentSpec {
                    id: entry.id.clone(),
                    name: entry.id.clone(),
                    command,
                    resume: None,
                    headless: None,
                    mcp: false,
                    mcp_style: None,
                    install_hint: None,
                    enabled: true,
                    default: false,
                    configured: true,
                    environment: HashMap::new(),
                });
                agents.last_mut().expect("recién añadido")
            }
        };
        agent.configured = true;
        if let Some(v) = entry.name {
            agent.name = v;
        }
        if let Some(v) = entry.command {
            agent.command = v;
        }
        if let Some(v) = entry.resume_command {
            agent.resume = Some(v);
        }
        if let Some(v) = entry.headless_command {
            agent.headless = Some(v);
        }
        if let Some(v) = entry.enabled {
            agent.enabled = v;
        }
        agent.default = entry.default;
        agent.environment.extend(entry.environment);
    }
    if agents.iter().filter(|a| a.default).count() > 1 {
        return Err(tr!(
            "{p0}: solo un agente puede tener default = true",
            p0 = path.display()
        ));
    }
    Ok(agents)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detection {
    pub path: Option<PathBuf>,
    pub version: Option<String>,
}

/// Busca los programas en el PATH del shell de login interactivo del usuario (la app
/// abierta desde el Finder no tiene ese PATH) y obtiene sus versiones en paralelo.
pub fn detect(programs: &[String]) -> HashMap<String, Detection> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let list = programs
        .iter()
        .map(|p| shell_quote(p))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "for p in {list}; do printf '\\nFORGE_AGENT\\t%s\\t%s\\n' \"$p\" \"$(command -v \"$p\")\"; done"
    );
    let output = run_with_timeout(
        Command::new(&shell).args(["-l", "-i", "-c", &script]),
        Duration::from_secs(10),
    );
    let paths = parse_lookup(&output.unwrap_or_default());

    let threads: Vec<_> = programs
        .iter()
        .map(|program| {
            let (program, path) = (program.clone(), paths.get(program).cloned());
            std::thread::spawn(move || {
                let version = path.as_ref().and_then(|p| {
                    let out =
                        run_with_timeout(Command::new(p).arg("--version"), Duration::from_secs(8))?;
                    out.lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .map(|l| l.chars().take(40).collect())
                });
                (program, Detection { path, version })
            })
        })
        .collect();
    threads.into_iter().filter_map(|t| t.join().ok()).collect()
}

pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Líneas `FORGE_AGENT\t<programa>\t<ruta>` entre el resto de la salida del shell
/// (que puede incluir mensajes del perfil del usuario).
fn parse_lookup(output: &str) -> HashMap<String, PathBuf> {
    output
        .lines()
        .filter_map(|l| {
            let mut parts = l.strip_prefix("FORGE_AGENT\t")?.splitn(2, '\t');
            let (program, path) = (parts.next()?, parts.next()?.trim());
            (path.starts_with('/')).then(|| (program.to_string(), PathBuf::from(path)))
        })
        .collect()
}

fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// Agente por defecto: el marcado en agents.toml o el primero instalado de los conocidos.
pub fn default_agent<'a>(
    agents: &'a [AgentSpec],
    detected: &HashMap<String, Detection>,
) -> Option<&'a AgentSpec> {
    let installed = |a: &&AgentSpec| detected.get(a.program()).is_some_and(|d| d.path.is_some());
    agents
        .iter()
        .filter(|a| a.enabled)
        .find(|a| a.default)
        .or_else(|| agents.iter().filter(|a| a.enabled).find(installed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_get_their_event_hooks() {
        let agents = builtins();
        let get = |id: &str| agents.iter().find(|a| a.id == id).unwrap();
        let claude = get("claude").with_events("claude --continue", "/A/forge", false);
        assert!(
            claude.starts_with("claude --settings '{\"hooks\":{"),
            "{claude}"
        );
        // Comillas anidadas escapadas para el shell ('\'').
        assert!(
            claude.contains(r"'\''/A/forge'\'' agent-event stop"),
            "{claude}"
        );
        assert!(claude.contains("agent-event waiting"));
        assert!(claude.ends_with(" --continue"));
        let codex = get("codex").with_events("codex", "/A/forge", false);
        assert_eq!(
            codex,
            r#"codex -c 'notify=["/A/forge","agent-event","stop"]'"#
        );
        assert_eq!(
            get("opencode").with_events("opencode", "/A/forge", false),
            "opencode"
        );
        // Con el mod (cargado por las terminales de Forge), Claude se abre tal cual.
        assert_eq!(
            get("claude").with_events("claude", "/A/forge", true),
            "claude"
        );
    }

    #[test]
    fn claude_opens_with_the_chosen_model() {
        let agents = builtins();
        let get = |id: &str| agents.iter().find(|a| a.id == id).unwrap();
        assert_eq!(
            get("claude").with_model("claude --continue", "sonnet"),
            "claude --model 'sonnet' --continue"
        );
        assert_eq!(
            get("claude").with_model("claude", "opusplan"),
            "claude --model 'opusplan'"
        );
        assert_eq!(
            get("claude").with_model("claude --model opus", "sonnet"),
            "claude --model opus"
        );
        assert_eq!(
            get("claude").with_model("claude", ""),
            "claude",
            "sin elección: su configuración"
        );
        assert_eq!(get("codex").with_model("codex", "sonnet"), "codex");
    }

    fn project(name: &str, agents_toml: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-agents-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        if let Some(t) = agents_toml {
            std::fs::write(dir.join(".forge/agents.toml"), t).unwrap();
        }
        dir
    }

    #[test]
    fn builtins_overrides_and_custom_agents() {
        let dir = project(
            "load",
            Some(
                "[[agents]]\nid = \"claude\"\ncommand = \"claude --model opus\"\ndefault = true\n\
                 [[agents]]\nid = \"codex\"\nenabled = false\n\
                 [[agents]]\nid = \"aider\"\nname = \"Aider\"\ncommand = \"aider --no-auto-commits\"\n\
                 [[reviewers]]\nid = \"security\"\nroute = \"powerful\"\n",
            ),
        );
        let agents = load(&dir).unwrap();
        let get = |id: &str| agents.iter().find(|a| a.id == id).unwrap();
        assert_eq!(get("claude").command, "claude --model opus");
        assert_eq!(get("claude").resume.as_deref(), Some("claude --continue"));
        assert!(get("claude").default && get("claude").configured);
        assert!(!get("codex").enabled);
        assert_eq!(
            (get("aider").name.as_str(), get("aider").program()),
            ("Aider", "aider")
        );
        assert!(!get("gemini").configured);

        let detected = HashMap::from([(
            "codex".to_string(),
            Detection {
                path: Some("/x".into()),
                version: None,
            },
        )]);
        assert_eq!(
            default_agent(&agents, &detected).map(|a| a.id.as_str()),
            Some("claude")
        );
        let plain = load(&project("plain", None)).unwrap();
        assert_eq!(
            default_agent(&plain, &detected).map(|a| a.id.as_str()),
            Some("codex"),
            "primero instalado"
        );
    }

    #[test]
    fn mcp_injection() {
        let agents = builtins();
        let get = |id: &str| agents.iter().find(|a| a.id == id).unwrap();
        let server = McpServer {
            name: "forge".into(),
            command: "/Apps/Forge/forge".into(),
            args: vec!["mcp".into(), "--project".into(), "/p/it's".into()],
        };
        let claude = get("claude").with_mcp("claude --continue", &server);
        assert!(
            claude.starts_with("claude --mcp-config '{\"mcpServers\""),
            "{claude}"
        );
        assert!(
            claude.ends_with(" --continue") && claude.contains(r"it'\''s"),
            "{claude}"
        );
        let codex = get("codex").with_mcp("codex resume --last", &server);
        assert!(
            codex.starts_with("codex -c 'mcp_servers.forge.command=\"/Apps/Forge/forge\"' -c "),
            "{codex}"
        );
        assert!(codex.ends_with(" resume --last"), "{codex}");
        assert_eq!(
            get("claude")
                .with_prompt("claude", "planifica #7")
                .as_deref(),
            Some("claude 'planifica #7'")
        );
        assert_eq!(
            get("opencode").with_prompt("opencode", "it's").as_deref(),
            Some(r"opencode --prompt 'it'\''s'")
        );
        assert_eq!(get("gemini").with_prompt("gemini", "x"), None);
        let opencode = get("opencode").with_mcp("opencode --continue", &server);
        assert!(
            opencode.starts_with("OPENCODE_CONFIG_CONTENT='{\"mcp\":{\"forge\":"),
            "{opencode}"
        );
        assert!(
            opencode.ends_with(" opencode --standalone --continue"),
            "{opencode}"
        );
    }

    #[test]
    fn config_errors() {
        let typo = project(
            "typo",
            Some("[[agents]]\nid = \"claude\"\ncomand = \"x\"\n"),
        );
        assert!(load(&typo).unwrap_err().contains("comand"));
        let custom = project("custom", Some("[[agents]]\nid = \"nuevo\"\n"));
        assert!(load(&custom).unwrap_err().contains("necesita `command`"));
        let two = project(
            "two",
            Some(
                "[[agents]]\nid = \"claude\"\ndefault = true\n[[agents]]\nid = \"codex\"\ndefault = true\n",
            ),
        );
        assert!(load(&two).unwrap_err().contains("solo un agente"));
    }

    #[test]
    fn parses_lookup_among_profile_noise() {
        let out = "Last login: hoy\nOS: macOS\n\nFORGE_AGENT\tclaude\t/Users/x/.local/bin/claude\n\nFORGE_AGENT\tgemini\t\n";
        let paths = parse_lookup(out);
        assert_eq!(
            paths.get("claude"),
            Some(&PathBuf::from("/Users/x/.local/bin/claude"))
        );
        assert!(!paths.contains_key("gemini"));
    }

    /// Detección real en esta máquina (sh siempre existe; un nombre inventado no).
    #[test]
    fn detects_real_programs() {
        let found = detect(&["sh".into(), "forge-no-existe-xyz".into()]);
        assert!(found["sh"].path.is_some());
        assert!(found["forge-no-existe-xyz"].path.is_none());
    }

    #[test]
    fn the_agents_template_loads_commented_and_uncommented() {
        let dir = std::env::temp_dir().join(format!("forge-agents-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        std::fs::write(dir.join(".forge/agents.toml"), template()).unwrap();
        assert_eq!(load(&dir).unwrap().len(), builtins().len());
        let open: String = template()
            .lines()
            .map(|l| {
                l.strip_prefix("# ")
                    .filter(|l| !l.contains(' ') || l.contains('=') || l.starts_with('['))
                    .unwrap_or(l)
            })
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(dir.join(".forge/agents.toml"), open).unwrap();
        let agents = load(&dir).unwrap();
        assert!(
            agents
                .iter()
                .any(|a| a.id == "mi-agente" && a.environment.contains_key("MI_VARIABLE"))
        );
        assert!(agents.iter().find(|a| a.id == "claude").unwrap().default);
        let _ = std::fs::remove_dir_all(dir);
    }
}
