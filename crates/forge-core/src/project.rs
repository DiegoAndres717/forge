// Configuración versionable del proyecto: `.forge/project.toml` y `.forge/layouts.toml`.
use crate::tr;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub workspace: WorkspaceSection,
    pub commands: Vec<SavedCommand>,
    pub layouts: HashMap<String, LayoutSpec>,
    pub processes: Vec<ProcessDef>,
    /// Variables para todos los paneles y procesos del proyecto.
    pub environment: HashMap<String, String>,
    pub memory: MemorySection,
}

/// `[memory]`: memoria del proyecto y si se comparte con los agentes (MCP).
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MemorySection {
    pub enabled: bool,
    /// Conectar la memoria a los agentes abiertos desde Forge.
    pub share_with_agents: bool,
    pub provider: MemoryProvider,
    /// Servidor MCP externo (p. ej. Engram) cuando `provider = "mcp"`.
    pub command: Option<String>,
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            enabled: true,
            share_with_agents: true,
            provider: MemoryProvider::Local,
            command: None,
        }
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MemoryProvider {
    #[default]
    Local,
    Mcp,
}

/// Proceso administrado (`[[processes]]`).
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessDef {
    pub id: String,
    pub name: Option<String>,
    pub command: String,
    pub working_directory: Option<PathBuf>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Cuándo se inicia: "manual" (por defecto), "on-workspace-open" o "never" (= manual).
    #[serde(default)]
    pub restart: StartPolicy,
    /// Reinicio automático al terminar: "never" (por defecto), "on-failure" o "always".
    #[serde(default)]
    pub auto_restart: AutoRestart,
    pub health_check: Option<HealthCheck>,
}

impl ProcessDef {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StartPolicy {
    #[default]
    Manual,
    OnWorkspaceOpen,
    Never,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AutoRestart {
    #[default]
    Never,
    OnFailure,
    Always,
}

/// Comprobación de salud: exactamente una de `port`, `url` o `command`.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    pub port: Option<u16>,
    pub url: Option<String>,
    pub command: Option<String>,
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_interval() -> u64 {
    5
}

fn default_timeout() -> u64 {
    2
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSection {
    pub name: Option<String>,
    /// Raíz de trabajo relativa a la carpeta del proyecto (por defecto ".").
    pub root: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceSection {
    pub default_layout: Option<String>,
    pub restore_panels: bool,
    pub restore_processes: bool,
}

impl Default for WorkspaceSection {
    fn default() -> Self {
        Self {
            default_layout: None,
            restore_panels: true,
            restore_processes: true,
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SavedCommand {
    pub name: String,
    pub command: String,
    pub working_directory: Option<PathBuf>,
}

/// Layout predefinido: una división o un panel (con comando opcional).
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum LayoutSpec {
    Split {
        split: SplitSpec,
        #[serde(default = "half")]
        ratio: f32,
        first: Box<LayoutSpec>,
        second: Box<LayoutSpec>,
    },
    Panel {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        working_directory: Option<PathBuf>,
    },
}

fn half() -> f32 {
    0.5
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SplitSpec {
    /// Lado a lado.
    Row,
    /// Uno encima del otro.
    Column,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct LayoutsFile {
    #[serde(default)]
    layouts: HashMap<String, LayoutSpec>,
}

pub struct Project {
    /// Carpeta del proyecto (la que contiene `.forge/`).
    pub path: PathBuf,
    pub config: ProjectConfig,
    /// Agentes disponibles (conocidos + `.forge/agents.toml`).
    pub agents: Vec<crate::agents::AgentSpec>,
}

impl Project {
    /// Carga el proyecto; una configuración inválida devuelve un error legible.
    pub fn load(path: &Path) -> Result<Self, String> {
        let dir = path.join(".forge");
        let mut config: ProjectConfig = read_toml(&dir.join("project.toml"))?.unwrap_or_default();
        if let Some(file) = read_toml::<LayoutsFile>(&dir.join("layouts.toml"))? {
            config.layouts.extend(file.layouts);
        }
        if let Some(name) = &config.workspace.default_layout
            && !config.layouts.contains_key(name)
        {
            return Err(tr!(
                "{p0}: default_layout \"{name}\" no está definido",
                p0 = dir.display(),
                name = name
            ));
        }
        if config.memory.provider == MemoryProvider::Mcp
            && config
                .memory
                .command
                .as_deref()
                .is_none_or(|c| c.trim().is_empty())
        {
            return Err(tr!(
                "{p0}: [memory] provider = \"mcp\" necesita `command` (p. ej. \"engram mcp\")",
                p0 = dir.join("project.toml").display()
            ));
        }
        validate_processes(&config.processes)
            .map_err(|e| format!("{}: {e}", dir.join("project.toml").display()))?;
        let agents = crate::agents::load(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            config,
            agents,
        })
    }

    pub fn name(&self) -> String {
        self.config
            .project
            .name
            .clone()
            .unwrap_or_else(|| folder_name(&self.path))
    }

    /// Directorio de trabajo base para paneles y comandos.
    pub fn root(&self) -> PathBuf {
        match &self.config.project.root {
            Some(root) => self.path.join(root),
            None => self.path.clone(),
        }
    }

    pub fn has_config(&self) -> bool {
        self.path.join(".forge/project.toml").exists()
    }

    pub fn default_layout(&self) -> Option<&LayoutSpec> {
        self.config
            .layouts
            .get(self.config.workspace.default_layout.as_ref()?)
    }

    /// Crea `.forge/project.toml` con comandos detectados en package.json.
    pub fn write_template(&self) -> Result<(), String> {
        let dir = self.path.join(".forge");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = dir.join("project.toml");
        if file.exists() {
            return Err(tr!("{p0} ya existe", p0 = file.display()));
        }
        std::fs::write(&file, template(&self.name(), &detect_commands(&self.path)))
            .map_err(|e| format!("{}: {e}", file.display()))
    }
}

/// Cómo comprueba Forge que un proceso funciona.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum HealthKind {
    #[default]
    None,
    Port,
    Url,
    Command,
}

/// Ajustes → Procesos: un proceso tal como se edita en el formulario.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ProcessForm {
    pub id: String,
    pub name: String,
    pub command: String,
    /// Relativa a la carpeta del proyecto ("" = la raíz).
    pub working_directory: String,
    pub start_on_open: bool,
    pub auto_restart: AutoRestart,
    pub health: HealthKind,
    pub port: u16,
    pub url: String,
    pub check_command: String,
    /// Lo que el formulario no edita, para no perderlo al guardar.
    pub environment: HashMap<String, String>,
    pub interval_seconds: u64,
    pub timeout_seconds: u64,
}

/// Un comando guardado tal como se edita en el formulario.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CommandForm {
    pub name: String,
    pub command: String,
    pub working_directory: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ProjectForm {
    pub processes: Vec<ProcessForm>,
    pub commands: Vec<CommandForm>,
}

fn dir_text(d: &Option<PathBuf>) -> String {
    d.as_ref()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl ProjectForm {
    /// Lo de `.forge/project.toml`; sin archivo, los scripts detectados (como la plantilla).
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = path.join(".forge/project.toml");
        let config: ProjectConfig = match read_toml(&file)? {
            Some(c) => c,
            None => ProjectConfig {
                commands: detect_commands(path),
                ..Default::default()
            },
        };
        Ok(Self {
            processes: config
                .processes
                .iter()
                .map(|p| {
                    let h = p.health_check.as_ref();
                    ProcessForm {
                        id: p.id.clone(),
                        name: p.name.clone().unwrap_or_default(),
                        command: p.command.clone(),
                        working_directory: dir_text(&p.working_directory),
                        start_on_open: p.restart == StartPolicy::OnWorkspaceOpen,
                        auto_restart: p.auto_restart,
                        health: match h {
                            Some(h) if h.port.is_some() => HealthKind::Port,
                            Some(h) if h.url.is_some() => HealthKind::Url,
                            Some(h) if h.command.is_some() => HealthKind::Command,
                            _ => HealthKind::None,
                        },
                        port: h.and_then(|h| h.port).unwrap_or_default(),
                        url: h.and_then(|h| h.url.clone()).unwrap_or_default(),
                        check_command: h.and_then(|h| h.command.clone()).unwrap_or_default(),
                        environment: p.environment.clone(),
                        interval_seconds: h.map_or(default_interval(), |h| h.interval_seconds),
                        timeout_seconds: h.map_or(default_timeout(), |h| h.timeout_seconds),
                    }
                })
                .collect(),
            commands: config
                .commands
                .iter()
                .map(|c| CommandForm {
                    name: c.name.clone(),
                    command: c.command.clone(),
                    working_directory: dir_text(&c.working_directory),
                })
                .collect(),
        })
    }

    /// Guarda procesos y comandos en `.forge/project.toml` (lo crea si no existe). El resto
    /// del archivo (distribuciones, entorno, comentarios) se conserva; lo inválido no se escribe.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};
        let file = path.join(".forge/project.toml");
        let text =
            std::fs::read_to_string(&file).unwrap_or_else(|_| template(&folder_name(path), &[]));
        let mut doc: DocumentMut = text
            .parse()
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let mut processes = ArrayOfTables::new();
        let mut seen = std::collections::HashSet::new();
        for p in &self.processes {
            let id = p.id.trim();
            if id.is_empty() || id.contains(char::is_whitespace) {
                return Err(tr!("Cada proceso necesita un id sin espacios.").into());
            }
            if !seen.insert(id) {
                return Err(tr!("Hay dos procesos con el id «{id}».", id = id));
            }
            if p.command.trim().is_empty() {
                return Err(tr!("El proceso «{id}» necesita un comando.", id = id));
            }
            let mut t = Table::new();
            t["id"] = value(id);
            if !p.name.trim().is_empty() {
                t["name"] = value(p.name.trim());
            }
            t["command"] = value(p.command.trim());
            if !p.working_directory.trim().is_empty() {
                t["working_directory"] = value(p.working_directory.trim());
            }
            if p.start_on_open {
                t["restart"] = value("on-workspace-open");
            }
            match p.auto_restart {
                AutoRestart::Never => {}
                AutoRestart::OnFailure => t["auto_restart"] = value("on-failure"),
                AutoRestart::Always => t["auto_restart"] = value("always"),
            }
            if !p.environment.is_empty() {
                let mut env = Table::new();
                let mut vars: Vec<_> = p.environment.iter().collect();
                vars.sort();
                for (k, v) in vars {
                    env[k.as_str()] = value(v.as_str());
                }
                t["environment"] = Item::Table(env);
            }
            let mut h = Table::new();
            match p.health {
                HealthKind::None => {}
                HealthKind::Port if p.port == 0 => {
                    return Err(tr!("Falta el puerto de «{id}».", id = id));
                }
                HealthKind::Port => h["port"] = value(i64::from(p.port)),
                HealthKind::Url if !p.url.trim().starts_with("http://") => {
                    return Err(tr!(
                        "La URL de «{id}» debe empezar por http:// (para https usa el puerto).",
                        id = id
                    ));
                }
                HealthKind::Url => h["url"] = value(p.url.trim()),
                HealthKind::Command if p.check_command.trim().is_empty() => {
                    return Err(tr!("Falta el comando de comprobación de «{id}».", id = id));
                }
                HealthKind::Command => h["command"] = value(p.check_command.trim()),
            }
            if p.health != HealthKind::None {
                if p.interval_seconds != default_interval() {
                    h["interval_seconds"] = value(p.interval_seconds as i64);
                }
                if p.timeout_seconds != default_timeout() {
                    h["timeout_seconds"] = value(p.timeout_seconds as i64);
                }
                t["health_check"] = Item::Table(h);
            }
            processes.push(t);
        }
        let mut commands = ArrayOfTables::new();
        for c in &self.commands {
            if c.name.trim().is_empty() || c.command.trim().is_empty() {
                return Err(tr!("Cada comando necesita un nombre y el comando.").into());
            }
            let mut t = Table::new();
            t["name"] = value(c.name.trim());
            t["command"] = value(c.command.trim());
            if !c.working_directory.trim().is_empty() {
                t["working_directory"] = value(c.working_directory.trim());
            }
            commands.push(t);
        }
        for (key, list) in [("processes", processes), ("commands", commands)] {
            if list.is_empty() {
                doc.remove(key);
            } else {
                doc[key] = Item::ArrayOfTables(list);
            }
        }
        let out = doc.to_string();
        let config: ProjectConfig = toml::from_str(&out).map_err(|e| e.to_string())?;
        validate_processes(&config.processes)?;
        std::fs::create_dir_all(path.join(".forge")).map_err(|e| e.to_string())?;
        std::fs::write(&file, out).map_err(|e| format!("{}: {e}", file.display()))
    }
}

fn validate_processes(processes: &[ProcessDef]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for p in processes {
        if !seen.insert(&p.id) {
            return Err(tr!("el proceso \"{p0}\" está repetido", p0 = p.id));
        }
        if let Some(h) = &p.health_check {
            let kinds = [h.port.is_some(), h.url.is_some(), h.command.is_some()];
            if kinds.iter().filter(|k| **k).count() != 1 {
                return Err(tr!(
                    "health_check de \"{p0}\": usa exactamente uno de port, url o command",
                    p0 = p.id
                ));
            }
            if h.url.as_ref().is_some_and(|u| !u.starts_with("http://")) {
                return Err(tr!(
                    "health_check de \"{p0}\": url debe empezar por http:// (para https usa port o command)",
                    p0 = p.id
                ));
            }
        }
    }
    Ok(())
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(text) => toml::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
    }
}

pub fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Scripts habituales de package.json, con el gestor de paquetes que use el proyecto.
pub fn detect_commands(path: &Path) -> Vec<SavedCommand> {
    #[derive(Deserialize)]
    struct Package {
        #[serde(default)]
        scripts: HashMap<String, String>,
    }
    let Ok(text) = std::fs::read_to_string(path.join("package.json")) else {
        return vec![];
    };
    let Ok(package) = serde_json::from_str::<Package>(&text) else {
        return vec![];
    };
    let runner = if path.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if path.join("yarn.lock").exists() {
        "yarn"
    } else if path.join("bun.lockb").exists() || path.join("bun.lock").exists() {
        "bun run"
    } else {
        "npm run"
    };
    ["dev", "start", "test", "lint", "typecheck", "build"]
        .into_iter()
        .filter(|s| package.scripts.contains_key(*s))
        .map(|s| SavedCommand {
            name: s.to_string(),
            command: format!("{runner} {s}"),
            working_directory: None,
        })
        .collect()
}

fn template(name: &str, commands: &[SavedCommand]) -> String {
    let mut out = format!(
        "[project]\nname = {name:?}\n\n[workspace]\nrestore_panels = true\n# default_layout = \"development\"\n\n\
         # Comandos guardados: aparecen en la barra lateral y se abren en un panel nuevo.\n"
    );
    for c in commands {
        out += &format!(
            "[[commands]]\nname = {:?}\ncommand = {:?}\n\n",
            c.name, c.command
        );
    }
    if commands.is_empty() {
        out += "# [[commands]]\n# name = \"Dev server\"\n# command = \"npm run dev\"\n\n";
    }
    out += "# Procesos administrados: panel PROCESOS de la barra lateral (iniciar, detener, logs, puertos).\n\
            # [[processes]]\n\
            # id = \"frontend\"\n\
            # name = \"Frontend\"\n\
            # command = \"npm run dev\"\n\
            # restart = \"on-workspace-open\"   # manual | on-workspace-open | never\n\
            # auto_restart = \"on-failure\"     # never | on-failure | always\n\
            # health_check = { url = \"http://localhost:5173\" }   # o { port = 5432 } o { command = \"pg_isready\" }\n\n\
            # Variables de entorno para todos los paneles y procesos.\n\
            # [environment]\n\
            # NODE_ENV = \"development\"\n\n";
    out += "# Layout predefinido (se usa al abrir sin sesión guardada o con \"Restablecer layout\").\n\
            # [layouts.development]\n\
            # split = \"row\"\n\
            # first = { name = \"Claude Code\", command = \"claude\" }\n\
            # second = { split = \"column\", first = { command = \"npm run dev\" }, second = {} }\n";
    out
}

/// Rama actual si `path` está dentro de un repositorio Git (o el commit corto si HEAD está suelto).
pub fn git_branch(path: &Path) -> Option<String> {
    let dot_git = path
        .ancestors()
        .map(|p| p.join(".git"))
        .find(|p| p.exists())?;
    // En worktrees y submódulos `.git` es un archivo "gitdir: <ruta>".
    let git_dir = if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let dir = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
        if dir.is_absolute() {
            dir
        } else {
            dot_git.parent()?.join(dir)
        }
    } else {
        dot_git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => branch.to_string(),
        None => head.chars().take(7).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_project_and_layouts() {
        let dir = temp_dir("parse");
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        std::fs::write(
            dir.join(".forge/project.toml"),
            r#"
            [project]
            name = "Bovinapp"
            [workspace]
            default_layout = "dev"
            [[commands]]
            name = "Dev"
            command = "npm run dev"
            [[processes]]
            id = "frontend"
            command = "npm run dev"
            restart = "on-workspace-open"
            auto_restart = "on-failure"
            health_check = { url = "http://localhost:5173" }
            [environment]
            NODE_ENV = "development"
            "#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".forge/layouts.toml"),
            r#"
            [layouts.dev]
            split = "row"
            first = { name = "Claude Code", command = "claude" }
            second = { split = "column", ratio = 0.3, first = { command = "npm run dev" }, second = {} }
            "#,
        )
        .unwrap();

        let p = Project::load(&dir).unwrap();
        assert_eq!(p.name(), "Bovinapp");
        assert_eq!(p.config.commands[0].command, "npm run dev");
        let frontend = &p.config.processes[0];
        assert_eq!(frontend.restart, StartPolicy::OnWorkspaceOpen);
        assert_eq!(frontend.auto_restart, AutoRestart::OnFailure);
        assert_eq!(
            frontend.health_check.as_ref().map(|h| h.interval_seconds),
            Some(5)
        );
        assert_eq!(p.config.environment["NODE_ENV"], "development");
        let Some(LayoutSpec::Split {
            split: SplitSpec::Row,
            second,
            ..
        }) = p.default_layout()
        else {
            panic!("layout inesperado");
        };
        assert!(matches!(**second, LayoutSpec::Split { ratio, .. } if ratio == 0.3));
    }

    #[test]
    fn reports_errors_clearly() {
        let dir = temp_dir("errors");
        std::fs::create_dir_all(dir.join(".forge")).unwrap();
        std::fs::write(
            dir.join(".forge/project.toml"),
            "[workspace]\ndefault_layout = \"nada\"",
        )
        .unwrap();
        assert!(
            Project::load(&dir)
                .err()
                .unwrap()
                .contains("\"nada\" no está definido")
        );
        let process =
            |extra: &str| format!("[[processes]]\nid = \"db\"\ncommand = \"x\"\n{extra}\n");
        std::fs::write(dir.join(".forge/project.toml"), process("") + &process("")).unwrap();
        assert!(Project::load(&dir).err().unwrap().contains("repetido"));
        std::fs::write(
            dir.join(".forge/project.toml"),
            process("health_check = { port = 1, command = \"y\" }"),
        )
        .unwrap();
        assert!(
            Project::load(&dir)
                .err()
                .unwrap()
                .contains("exactamente uno")
        );
        std::fs::write(
            dir.join(".forge/project.toml"),
            process("health_check = { url = \"https://x\" }"),
        )
        .unwrap();
        assert!(Project::load(&dir).err().unwrap().contains("http://"));
        std::fs::write(dir.join(".forge/project.toml"), "[project]\nnmae = \"x\"").unwrap();
        assert!(Project::load(&dir).err().unwrap().contains("nmae"));
    }

    #[test]
    fn template_detects_scripts_and_round_trips() {
        let dir = temp_dir("template");
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"dev":"vite","test":"vitest","x":"y"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        let p = Project::load(&dir).unwrap();
        assert!(!p.has_config());
        p.write_template().unwrap();

        let p = Project::load(&dir).unwrap();
        let commands: Vec<&str> = p
            .config
            .commands
            .iter()
            .map(|c| c.command.as_str())
            .collect();
        assert_eq!(commands, ["pnpm dev", "pnpm test"]);
        assert!(p.write_template().is_err());
    }

    #[test]
    fn git_branch_and_worktree() {
        let dir = temp_dir("git");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        assert_eq!(git_branch(&dir.join("src/deep")).as_deref(), Some("main"));

        let wt = temp_dir("worktree");
        std::fs::create_dir_all(dir.join(".git/worktrees/wt")).unwrap();
        std::fs::write(dir.join(".git/worktrees/wt/HEAD"), "0123456789abcdef\n").unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", dir.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        assert_eq!(git_branch(&wt).as_deref(), Some("0123456"));
    }

    #[test]
    fn the_processes_form_keeps_the_rest_of_the_file() {
        let dir = std::env::temp_dir().join(format!("forge-project-form-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();

        // Sin archivo: los scripts detectados.
        let mut form = ProjectForm::load(&dir).unwrap();
        assert!(form.processes.is_empty());
        assert_eq!(form.commands.len(), 1);
        form.processes.push(ProcessForm {
            id: "web".into(),
            name: "Web".into(),
            command: "npm run dev".into(),
            working_directory: "apps/web".into(),
            start_on_open: true,
            auto_restart: AutoRestart::OnFailure,
            health: HealthKind::Port,
            port: 5173,
            ..ProcessForm::default()
        });
        form.save(&dir).unwrap();
        let project = Project::load(&dir).unwrap();
        let p = &project.config.processes[0];
        assert_eq!(
            (p.restart, p.auto_restart),
            (StartPolicy::OnWorkspaceOpen, AutoRestart::OnFailure)
        );
        assert_eq!(p.health_check.as_ref().unwrap().port, Some(5173));
        assert_eq!(p.working_directory.as_deref(), Some(Path::new("apps/web")));
        assert_eq!(ProjectForm::load(&dir).unwrap().processes, form.processes);

        // Lo que el formulario no toca (comentarios, entorno, distribuciones) sigue.
        let path = dir.join(".forge/project.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            format!("# mi nota\n{text}\n[environment]\nAPI_URL = \"http://localhost:3000\"\n"),
        )
        .unwrap();
        form.commands.clear();
        form.save(&dir).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# mi nota") && text.contains("API_URL"),
            "{text}"
        );
        assert!(Project::load(&dir).unwrap().config.commands.is_empty());

        // Lo inválido no se escribe.
        let mut bad = form.clone();
        bad.processes[0].health = HealthKind::Url;
        bad.processes[0].url = "https://x".into();
        assert!(bad.save(&dir).is_err());
        let mut bad = form.clone();
        bad.processes.push(ProcessForm {
            id: "web".into(),
            command: "x".into(),
            ..ProcessForm::default()
        });
        assert!(bad.save(&dir).is_err());
        assert_eq!(ProjectForm::load(&dir).unwrap(), form);
        let _ = std::fs::remove_dir_all(dir);
    }
}
