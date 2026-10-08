// Servidor MCP (stdio, JSON-RPC 2.0 por líneas): la memoria y el contexto del proyecto
// como herramientas para Claude Code, Codex, OpenCode u otro cliente MCP.
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
            "description": "Busca en la memoria del proyecto (decisiones, arquitectura, errores resueltos, comandos, convenciones). Úsala antes de decidir algo o al encontrar un error que quizá ya se resolvió.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Palabras a buscar (sin tildes también vale)"},
                    "kind": {"type": "string", "enum": kinds},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50}
                },
                "required": ["query"]
            }
        },
        {
            "name": "memory_save",
            "description": "Guarda algo que convenga recordar en el proyecto: una decisión y su porqué, un error resuelto y su causa, un comando útil, una convención. Para cosas por hacer usa idea_add.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": kinds},
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "tags": {"type": "string", "description": "Separadas por comas"}
                },
                "required": ["kind", "title", "body"]
            }
        },
        {
            "name": "memory_list",
            "description": "Lista las memorias más recientes del proyecto, opcionalmente de un tipo.",
            "inputSchema": {
                "type": "object",
                "properties": {"kind": {"type": "string", "enum": kinds}, "limit": {"type": "integer", "minimum": 1, "maximum": 50}}
            }
        },
        {
            "name": "memory_delete",
            "description": "Borra una memoria que ya no es cierta (por id).",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}
        },
        {
            "name": "ideas_list",
            "description": "Lista de ideas y pendientes del proyecto (o la lista general con scope=general): en curso, pendientes y, si se pide, las hechas. Consúltala al empezar a trabajar o cuando el usuario pregunte qué falta.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "include_done": {"type": "boolean", "description": "Incluir las ya hechas"},
                    "scope": {"type": "string", "enum": ["project", "general"]}
                }
            }
        },
        {
            "name": "idea_add",
            "description": "Anota una idea o pendiente para no perderla (propia o del usuario). Úsala cuando surja algo para hacer después, en vez de dejarlo solo en la conversación.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Qué hacer, en una línea"},
                    "note": {"type": "string", "description": "Detalle o criterio de terminado (opcional)"},
                    "scope": {"type": "string", "enum": ["project", "general"], "description": "general = no es de este proyecto"}
                },
                "required": ["title"]
            }
        },
        {
            "name": "idea_update",
            "description": "Cambia una idea: márcala doing al empezarla y done al terminarla (queda tachada), o corrige título y nota.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "status": {"type": "string", "enum": ["pending", "doing", "done"]},
                    "title": {"type": "string"},
                    "note": {"type": "string"},
                    "scope": {"type": "string", "enum": ["project", "general"]}
                },
                "required": ["id"]
            }
        },
        {
            "name": "idea_delete",
            "description": "Borra una idea descartada (por id). Si se hizo, mejor márcala done.",
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "integer"}, "scope": {"type": "string", "enum": ["project", "general"]}},
                "required": ["id"]
            }
        },
        {
            "name": "project_context",
            "description": "Resumen del proyecto: reglas y checks del Guard, procesos, últimas validaciones y decisiones recientes.",
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
                    "instructions": "Memoria compartida del proyecto (Forge). Consulta memory_search antes de decisiones importantes o al depurar; guarda con memory_save las decisiones, errores resueltos y convenciones que descubras. Las cosas por hacer van en la lista de ideas (ideas_list, idea_add, idea_update): márcalas doing al empezar y done al terminar."
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => Ok(self.call(
                params["name"].as_str().unwrap_or_default(),
                &params["arguments"],
            )),
            _ => Err(json!({"code": -32601, "message": format!("método no soportado: {method}")})),
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
                .map(|found| listing(&found, "No hay memorias que coincidan.")),
            "memory_list" => self
                .store
                .list_memories(self.project, kind, limit)
                .map(|found| listing(&found, "La memoria está vacía.")),
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
            "memory_delete" => match args["id"].as_i64() {
                Some(id) => self.store.delete_memory(self.project, id).map(|ok| {
                    if ok {
                        format!("Memoria #{id} borrada.")
                    } else {
                        format!("No existe la memoria #{id}.")
                    }
                }),
                None => Err("falta `id`".into()),
            },
            "ideas_list" => self
                .store
                .list_ideas(list, args["include_done"].as_bool().unwrap_or(false))
                .map(|ideas| {
                    if ideas.is_empty() {
                        "No hay ideas pendientes.".into()
                    } else {
                        ideas
                            .iter()
                            .map(ideas::render)
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                }),
            "idea_add" => self
                .store
                .add_idea(
                    list,
                    args["title"].as_str().unwrap_or_default(),
                    args["note"].as_str().unwrap_or_default(),
                    &self.client,
                )
                .map(|id| format!("Anotada como idea #{id}.")),
            "idea_update" => match args["id"].as_i64() {
                Some(id) => self
                    .store
                    .update_idea(
                        list,
                        id,
                        args["status"].as_str(),
                        args["title"].as_str(),
                        args["note"].as_str(),
                        &self.client,
                    )
                    .map(|ok| {
                        if ok {
                            format!("Idea #{id} actualizada.")
                        } else {
                            format!("No existe la idea #{id} en esta lista.")
                        }
                    }),
                None => Err("falta `id`".into()),
            },
            "idea_delete" => match args["id"].as_i64() {
                Some(id) => self.store.delete_idea(list, id).map(|ok| {
                    if ok {
                        format!("Idea #{id} borrada.")
                    } else {
                        format!("No existe la idea #{id} en esta lista.")
                    }
                }),
                None => Err("falta `id`".into()),
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
        crate::project::git_branch(project).map_or(String::new(), |b| format!(" · rama {b}"));
    out.push(format!("Proyecto {name} ({}){branch}", project.display()));

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
        out.push(format!(
            "  máximo {} líneas por cambio",
            rules.quality.max_changed_lines
        ));
    }
    if let Ok(p) = crate::project::Project::load(project)
        && !p.config.processes.is_empty()
    {
        out.push("Procesos:".into());
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
        out.push("Últimas validaciones:".into());
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
            "Memoria (decisiones, arquitectura, convenciones; detalle con memory_search):".into(),
        );
        out.extend(important);
    }
    let open = store.list_ideas(Some(project), false).unwrap_or_default();
    if !open.is_empty() {
        out.push("Ideas pendientes (ideas_list para el detalle):".into());
        out.extend(
            open.iter()
                .take(12)
                .map(|i| format!("  {}", ideas::render(i).lines().next().unwrap_or_default())),
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
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 5);

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
        let found = call(&mut server, 4, "memory_search", json!({"query": "wompi"}));
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
