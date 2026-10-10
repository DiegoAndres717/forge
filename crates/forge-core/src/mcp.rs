// Servidor MCP (stdio, JSON-RPC 2.0 por líneas): la memoria y el contexto del proyecto
// como herramientas para Claude Code, Codex, OpenCode u otro cliente MCP.
use crate::tr;
use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{Value, json};

use crate::ideas;
use crate::memory::{self, KINDS};
use crate::store::Store;

const PROTOCOL: &str = "2025-06-18";

fn tools() -> Value {
    let kinds: Vec<&str> = KINDS.iter().map(|(id, _)| *id).collect();
    json!([
        {
            "name": "memory_search",
            "description": tr!("Busca en la memoria del proyecto (decisiones, arquitectura, errores resueltos, comandos, convenciones). Úsala antes de decidir algo o al encontrar un error que quizá ya se resolvió."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": tr!("Palabras a buscar (sin tildes también vale)")},
                    "kind": {"type": "string", "enum": kinds},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50}
                },
                "required": ["query"]
            }
        },
        {
            "name": "memory_save",
            "description": tr!("Guarda algo que convenga recordar en el proyecto: una decisión y su porqué, un error resuelto y su causa, un comando útil, una convención. Para cosas por hacer usa los planes (plan_add)."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": kinds},
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "tags": {"type": "string", "description": tr!("Separadas por comas")},
                    "force": {"type": "boolean", "description": tr!("Guardar aunque ya exista una memoria parecida")}
                },
                "required": ["kind", "title", "body"]
            }
        },
        {
            "name": "memory_update",
            "description": tr!("Corrige o amplía una memoria existente (por id) en vez de guardar otra parecida. Solo cambia los campos que pases."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "kind": {"type": "string", "enum": kinds},
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "tags": {"type": "string", "description": tr!("Separadas por comas")}
                },
                "required": ["id"]
            }
        },
        {
            "name": "memory_list",
            "description": tr!("Lista las memorias más recientes del proyecto, opcionalmente de un tipo."),
            "inputSchema": {
                "type": "object",
                "properties": {"kind": {"type": "string", "enum": kinds}, "limit": {"type": "integer", "minimum": 1, "maximum": 50}}
            }
        },
        {
            "name": "memory_delete",
            "description": tr!("Borra una memoria que ya no es cierta (por id)."),
            "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}
        },
        {
            "name": "plans_list",
            "description": tr!("Planes y backlog del proyecto (o la lista general con scope=general): en progreso con sus fases (rama y tareas), el backlog y, si se pide, los completados. Consúltalo al empezar a trabajar o cuando el usuario pregunte qué falta o en qué va."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "include_done": {"type": "boolean", "description": tr!("Incluir los completados")},
                    "scope": {"type": "string", "enum": ["project", "general"]}
                }
            }
        },
        {
            "name": "plan_add",
            "description": tr!("Crea un plan por fases cuando el usuario cuente algo que quiere hacer (cada fase con su rama si la merece y sus tareas), o, sin fases, anota una idea en el backlog para no perderla. Una fase no siempre es una rama."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": tr!("El objetivo, en una línea")},
                    "note": {"type": "string", "description": tr!("Contexto o criterio de terminado (opcional)")},
                    "phases": {
                        "type": "array",
                        "description": tr!("Fases en orden (vacío: idea del backlog)"),
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": {"type": "string"},
                                "notes": {"type": "string"},
                                "branch": {"type": "string", "description": tr!("Rama de Git, p. ej. feat/export-csv (opcional)")},
                                "tasks": {"type": "array", "items": {"type": "string"}}
                            },
                            "required": ["title"]
                        }
                    },
                    "scope": {"type": "string", "enum": ["project", "general"], "description": tr!("general = no es de este proyecto")}
                },
                "required": ["title"]
            }
        },
        {
            "name": "plan_update",
            "description": tr!("Avanza un plan: al empezar una fase márcala in_progress (y crea su rama si la tiene), tacha tareas al terminarlas y marca la fase done al acabar; el plan se completa solo cuando terminan todas. También planifica una idea del backlog (phases), cambia el estado de una idea sin fases o corrige título y nota. Devuelve el plan actualizado."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "status": {"type": "string", "enum": ["backlog", "in_progress", "done"], "description": tr!("Estado del plan o idea (con fases se calcula solo)")},
                    "title": {"type": "string"},
                    "note": {"type": "string"},
                    "phases": {"type": "array", "description": tr!("Reemplaza todas las fases (mismo formato que en plan_add)"), "items": {"type": "object"}},
                    "phase": {"type": "integer", "description": tr!("Número de fase a cambiar (desde 1)")},
                    "phase_status": {"type": "string", "enum": ["backlog", "in_progress", "done"]},
                    "branch": {"type": "string", "description": tr!("Rama de la fase (vacío la quita)")},
                    "phase_notes": {"type": "string"},
                    "task": {"type": "integer", "description": tr!("Número de tarea de la fase (desde 1)")},
                    "task_done": {"type": "boolean", "description": tr!("Por defecto true: tachar la tarea")},
                    "add_task": {"type": "string", "description": tr!("Añade una tarea a la fase")},
                    "scope": {"type": "string", "enum": ["project", "general"]}
                },
                "required": ["id"]
            }
        },
        {
            "name": "plan_delete",
            "description": tr!("Borra un plan o idea descartada (por id). Si se hizo, mejor márcalo done."),
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "integer"}, "scope": {"type": "string", "enum": ["project", "general"]}},
                "required": ["id"]
            }
        },
        {
            "name": "project_context",
            "description": tr!("Resumen del proyecto: reglas y checks del Guard, procesos, últimas validaciones y decisiones recientes."),
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

pub struct Server<'a> {
    project: &'a Path,
    store: &'a Store,
    /// Cliente conectado (clientInfo.name): queda como autor de lo que guarde.
    client: String,
}

impl<'a> Server<'a> {
    pub fn new(project: &'a Path, store: &'a Store) -> Self {
        Self {
            project,
            store,
            client: "agente".into(),
        }
    }

    /// Atiende un mensaje; `None` para notificaciones (no llevan respuesta).
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned()?;
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let result = match method {
            "initialize" => {
                if let Some(name) = params["clientInfo"]["name"].as_str() {
                    self.client = name.to_string();
                }
                let version = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "forge", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": tr!("Memoria compartida del proyecto (Forge). Consulta memory_search antes de decisiones importantes o al depurar; guarda con memory_save las decisiones, errores resueltos y convenciones que descubras. Lo que hay por hacer va en los planes (plans_list, plan_add, plan_update): cuando el usuario cuente algo que quiere hacer, crea un plan por fases (cada fase con su rama si la merece y sus tareas); lo que surja para después, al backlog (plan_add sin fases). Al empezar una fase márcala in_progress, tacha las tareas al terminarlas y marca la fase done al acabar.")
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => Ok(self.call(
                params["name"].as_str().unwrap_or_default(),
                &params["arguments"],
            )),
            _ => Err(
                json!({"code": -32601, "message": tr!("método no soportado: {method}", method = method)}),
            ),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        })
    }

    fn call(&self, tool: &str, args: &Value) -> Value {
        let text = |s: String, error: bool| json!({"content": [{"type": "text", "text": s}], "isError": error});
        let limit = args["limit"].as_u64().unwrap_or(10).clamp(1, 50) as usize;
        let kind = args["kind"].as_str();
        let list = (args["scope"].as_str() != Some("general")).then_some(self.project);
        let result = match tool {
            "memory_search" => self
                .store
                .search_memories(
                    self.project,
                    args["query"].as_str().unwrap_or_default(),
                    kind,
                    limit,
                )
                .map(|found| listing(&found, tr!("No hay memorias que coincidan."))),
            "memory_list" => self
                .store
                .list_memories(self.project, kind, limit)
                .map(|found| listing(&found, tr!("La memoria está vacía."))),
            "memory_save"
                if !args["force"].as_bool().unwrap_or(false)
                    && let Ok(Some(m)) = self.store.similar_memory(
                        self.project,
                        args["title"].as_str().unwrap_or_default(),
                    ) =>
            {
                Ok(tr!(
                    "No guardada: ya existe una memoria parecida.\n{p0}\nSi es lo mismo, actualízala con memory_update (id {id}); si es otra cosa, vuelve a llamar a memory_save con force=true.",
                    p0 = memory::render(&m),
                    id = m.id
                ))
            }
            "memory_save" => self
                .store
                .add_memory(
                    self.project,
                    kind.unwrap_or("note"),
                    args["title"].as_str().unwrap_or_default(),
                    args["body"].as_str().unwrap_or_default(),
                    args["tags"].as_str().unwrap_or_default(),
                    &self.client,
                )
                .map(|id| format!("Guardada como #{id}.")),
            "memory_update" => match args["id"].as_i64() {
                Some(id) => self
                    .store
                    .update_memory(
                        self.project,
                        id,
                        kind,
                        args["title"].as_str(),
                        args["body"].as_str(),
                        args["tags"].as_str(),
                    )
                    .map(|ok| {
                        if ok {
                            tr!("Memoria #{id} actualizada.", id = id)
                        } else {
                            tr!("No existe la memoria #{id}.", id = id)
                        }
                    }),
                None => Err(tr!("falta `id`").into()),
            },
            "memory_delete" => match args["id"].as_i64() {
                Some(id) => self.store.delete_memory(self.project, id).map(|ok| {
                    if ok {
                        tr!("Memoria #{id} borrada.", id = id)
                    } else {
                        tr!("No existe la memoria #{id}.", id = id)
                    }
                }),
                None => Err(tr!("falta `id`").into()),
            },
            // Los nombres anteriores (ideas_*) siguen valiendo para sesiones ya abiertas.
            "plans_list" | "ideas_list" => self
                .store
                .list_ideas(list, args["include_done"].as_bool().unwrap_or(false))
                .map(|ideas| {
                    if ideas.is_empty() {
                        tr!("No hay planes ni ideas en el backlog.").into()
                    } else {
                        ideas
                            .iter()
                            .map(ideas::render)
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                }),
            "plan_add" | "idea_add" => parse_phases(&args["phases"]).and_then(|phases| {
                let id = self.store.add_plan(
                    list,
                    args["title"].as_str().unwrap_or_default(),
                    args["note"].as_str().unwrap_or_default(),
                    &phases,
                    &self.client,
                )?;
                Ok(if phases.is_empty() {
                    tr!("Anotada en el backlog como #{id}.", id = id)
                } else {
                    tr!(
                        "Plan #{id} creado con {n} fases.",
                        id = id,
                        n = phases.len()
                    )
                })
            }),
            "plan_update" | "idea_update" => match args["id"].as_i64() {
                Some(id) => self.update_plan(list, id, args),
                None => Err(tr!("falta `id`").into()),
            },
            "plan_delete" | "idea_delete" => match args["id"].as_i64() {
                Some(id) => self.store.delete_idea(list, id).map(|ok| {
                    if ok {
                        tr!("Plan #{id} borrado.", id = id)
                    } else {
                        tr!("No existe el plan #{id} en esta lista.", id = id)
                    }
                }),
                None => Err(tr!("falta `id`").into()),
            },
            "project_context" => Ok(project_context(self.project, self.store)),
            _ => Err(format!("herramienta desconocida: {tool}")),
        };
        match result {
            Ok(s) => text(s, false),
            Err(e) => text(e, true),
        }
    }
}

impl Server<'_> {
    /// plan_update: fases nuevas, cambio de una fase y/o estado, título y nota; devuelve el
    /// plan como queda.
    fn update_plan(&self, list: Option<&Path>, id: i64, args: &Value) -> Result<String, String> {
        let missing = || tr!("No existe el plan #{id} en esta lista.", id = id);
        if !args["phases"].is_null() {
            let phases = parse_phases(&args["phases"])?;
            if !self.store.set_phases(list, id, &phases, &self.client)? {
                return Ok(missing());
            }
        }
        if let Some(n) = args["phase"].as_u64() {
            let task = args["task"].as_u64().map(|t| {
                (
                    t.saturating_sub(1) as usize,
                    args["task_done"].as_bool().unwrap_or(true),
                )
            });
            let change = ideas::PhaseChange {
                status: args["phase_status"].as_str(),
                branch: args["branch"].as_str(),
                notes: args["phase_notes"].as_str(),
                task,
                add_task: args["add_task"].as_str(),
            };
            let index = (n as usize)
                .checked_sub(1)
                .ok_or_else(|| tr!("las fases se numeran desde 1").to_string())?;
            if !self
                .store
                .update_phase(list, id, index, change, &self.client)?
            {
                return Ok(missing());
            }
        }
        let (status, title, note) = (
            args["status"].as_str(),
            args["title"].as_str(),
            args["note"].as_str(),
        );
        if (status.is_some() || title.is_some() || note.is_some())
            && !self
                .store
                .update_idea(list, id, status, title, note, &self.client)?
        {
            return Ok(missing());
        }
        match self.store.get_idea(list, id)? {
            Some(plan) => Ok(ideas::render(&plan)),
            None => Ok(missing()),
        }
    }
}

/// Fases que manda un agente: `[{title, notes?, branch?, tasks?: [texto]}]` (o vacío).
fn parse_phases(value: &Value) -> Result<Vec<ideas::Phase>, String> {
    let Some(items) = value.as_array() else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .map(|item| {
            let title = item["title"].as_str().unwrap_or_default();
            let mut phase = ideas::Phase::new(title);
            phase.notes = item["notes"].as_str().unwrap_or_default().to_string();
            phase.branch = item["branch"].as_str().map(str::to_string);
            if let Some(status) = item["status"].as_str() {
                phase.status = status.to_string();
            }
            phase.tasks = item["tasks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| {
                    let text = t.as_str().or_else(|| t["text"].as_str())?;
                    Some(ideas::Task {
                        text: text.to_string(),
                        done: t["done"].as_bool().unwrap_or(false),
                    })
                })
                .collect();
            Ok(phase)
        })
        .collect()
}

fn listing(found: &[memory::Memory], empty: &str) -> String {
    if found.is_empty() {
        return empty.into();
    }
    found
        .iter()
        .map(memory::render)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Resumen del proyecto para dar contexto a un agente.
pub fn project_context(project: &Path, store: &Store) -> String {
    let mut out = Vec::new();
    let name = crate::project::Project::load(project)
        .map(|p| p.name())
        .unwrap_or_else(|_| crate::project::folder_name(project));
    let branch =
        crate::project::git_branch(project).map_or(String::new(), |b| tr!(" · rama {b}", b = b));
    out.push(tr!(
        "Proyecto {name} ({p0}){branch}",
        p0 = project.display(),
        name = name,
        branch = branch
    ));

    if let Ok(Some(rules)) = crate::guard::Rules::load(project) {
        out.push("Project Guard:".into());
        for stage in crate::guard::Stage::ALL {
            let (checks, _) = rules.checks_for(stage);
            let list: Vec<String> = checks
                .iter()
                .map(|c| format!("{} (`{}`)", c.id, c.command))
                .collect();
            if !list.is_empty() {
                out.push(format!("  {}: {}", stage.label(), list.join(", ")));
            }
        }
        out.push(tr!(
            "  máximo {p0} líneas por cambio",
            p0 = rules.quality.max_changed_lines
        ));
    }
    if let Ok(p) = crate::project::Project::load(project)
        && !p.config.processes.is_empty()
    {
        out.push(tr!("Procesos:").into());
        out.extend(
            p.config
                .processes
                .iter()
                .map(|d| format!("  {} — `{}`", d.label(), d.command)),
        );
    }
    if let Ok(history) = store.validations(project, 5)
        && !history.is_empty()
    {
        out.push(tr!("Últimas validaciones:").into());
        out.extend(history.iter().map(|(id, r)| {
            format!(
                "  #{id} {} · {} · {}",
                r.stage.label(),
                crate::evidence::verdict_label(r.verdict, r.stage),
                crate::store::ago_precise(r.finished_at)
            )
        }));
    }
    let important: Vec<String> = store
        .list_memories(project, None, 40)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| matches!(m.kind.as_str(), "decision" | "architecture" | "convention"))
        .take(12)
        .map(|m| format!("  #{} [{}] {}", m.id, memory::kind_label(&m.kind), m.title))
        .collect();
    if !important.is_empty() {
        out.push(
            tr!("Memoria (decisiones, arquitectura, convenciones; detalle con memory_search):")
                .into(),
        );
        out.extend(important);
    }
    let open = store.list_ideas(Some(project), false).unwrap_or_default();
    let (doing, backlog): (Vec<_>, Vec<_>) = open.iter().partition(|i| i.status == "doing");
    if !doing.is_empty() {
        out.push(tr!("Planes en progreso (plans_list para el detalle):").into());
        out.extend(doing.iter().take(8).map(|plan| match plan.current_phase() {
            Some((n, phase)) => format!(
                "  #{} {} — {}",
                plan.id,
                plan.title,
                tr!(
                    "fase {n} de {total}: {phase}",
                    n = n + 1,
                    total = plan.phases.len(),
                    phase = phase.title
                )
            ),
            None => format!("  #{} {}", plan.id, plan.title),
        }));
    }
    if !backlog.is_empty() {
        out.push(tr!("Backlog:").into());
        out.extend(
            backlog
                .iter()
                .take(10)
                .map(|i| format!("  #{} {}", i.id, i.title)),
        );
    }
    out.join("\n")
}

/// Bucle del servidor: un mensaje JSON por línea en stdin, respuestas por stdout.
pub fn serve(project: &Path, store: &Store) -> std::io::Result<()> {
    let mut server = Server::new(project, store);
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => server.handle(&message),
            Err(e) => Some(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}}),
            ),
        };
        if let Some(response) = response {
            let mut out = stdout.lock();
            writeln!(out, "{response}")?;
            out.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_and_tools() {
        let store = Store::in_memory().unwrap();
        let project = Path::new("/p/x");
        let mut server = Server::new(project, &store);
        let init = server
            .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "clientInfo": {"name": "claude-code"}}}))
            .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(
            server
                .handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .is_none()
        );
        let list = server
            .handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 10);

        let call = |server: &mut Server, id: i64, name: &str, args: Value| {
            server.handle(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})).unwrap()
        };
        let saved = call(
            &mut server,
            3,
            "memory_save",
            json!({"kind": "decision", "title": "Pagos con Wompi", "body": "Se usa Wompi por soporte de PSE."}),
        );
        assert_eq!(saved["result"]["isError"], false);
        // Algo parecido no se duplica: se propone actualizar la existente.
        let dup = call(
            &mut server,
            31,
            "memory_save",
            json!({"kind": "decision", "title": "Pagos con Wompi y PSE", "body": "otra vez"}),
        );
        let dup_text = dup["result"]["content"][0]["text"].as_str().unwrap();
        assert!(dup_text.contains("memory_update"), "{dup_text}");
        let up = call(
            &mut server,
            32,
            "memory_update",
            json!({"id": 1, "body": "Se usa Wompi por PSE y Nequi."}),
        );
        assert_eq!(up["result"]["isError"], false);
        let forced = call(
            &mut server,
            33,
            "memory_save",
            json!({"kind": "note", "title": "Pagos con Wompi", "body": "x", "force": true}),
        );
        assert!(
            forced["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("#2")
        );
        let found = call(&mut server, 4, "memory_search", json!({"query": "nequi"}));
        let text = found["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Pagos con Wompi") && text.contains("claude-code"),
            "{text}"
        );
        let bad = call(
            &mut server,
            5,
            "memory_save",
            json!({"kind": "nada", "title": "x", "body": "y"}),
        );
        assert_eq!(bad["result"]["isError"], true);
        let unknown = server
            .handle(&json!({"jsonrpc": "2.0", "id": 6, "method": "resources/list"}))
            .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        let ctx = call(&mut server, 7, "project_context", json!({}));
        assert!(
            ctx["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Pagos con Wompi")
        );
    }
}
