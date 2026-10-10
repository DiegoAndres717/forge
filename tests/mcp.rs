// Fase 10: la memoria del proyecto como servidor MCP real (`forge mcp`), compartida
// entre clientes (un agente guarda, otro cliente la encuentra).
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

#[test]
fn mcp_server_shares_memory_between_clients() {
    // SAFETY: todos los tests fijan el mismo valor; los textos esperados están en español.
    unsafe { std::env::set_var("FORGE_LANG", "es") };
    let base = std::env::temp_dir().join(format!("forge-it-mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("proyecto");
    std::fs::create_dir_all(project.join(".forge")).unwrap();
    std::fs::write(
        project.join(".forge/project.toml"),
        "[project]\nname = \"Bovinapp\"\n",
    )
    .unwrap();
    let db = base.join("forge.db");
    let project_arg = project.to_string_lossy().into_owned();

    let mut child = Command::new(FORGE)
        .args(["mcp", "--project", &project_arg])
        .env("FORGE_DB", &db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut request = |message: Value| -> Option<Value> {
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
        message.get("id")?;
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        Some(serde_json::from_str(&line).unwrap())
    };

    let init = request(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "2"}}}))
    .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "forge");
    assert!(request(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());

    let tools = request(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "memory_search",
            "memory_save",
            "memory_update",
            "memory_list",
            "memory_delete",
            "plans_list",
            "plan_add",
            "plan_update",
            "plan_delete",
            "project_context"
        ]
    );

    let saved = request(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "memory_save",
        "arguments": {"kind": "decision", "title": "Vacunación por lotes", "body": "Se registran por lote para no duplicar animales.", "tags": "vacunas"}}}))
    .unwrap();
    assert_eq!(saved["result"]["isError"], false, "{saved}");

    let found = request(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "memory_search", "arguments": {"query": "vacunacion"}}})).unwrap();
    let text = found["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        text.contains("Vacunación por lotes") && text.contains("claude-code"),
        "{text}"
    );

    let context = request(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "project_context", "arguments": {}}})).unwrap();
    let context = context["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        context.contains("Proyecto Bovinapp") && context.contains("Vacunación por lotes"),
        "{context}"
    );
    // Planes: el agente anota en el backlog, crea un plan por fases y lo avanza.
    let mut call = |id: u64, name: &str, arguments: Value| -> String {
        let r = request(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})).unwrap();
        assert_eq!(r["result"]["isError"], false, "{r}");
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        call(6, "plan_add", json!({"title": "Exportar vacunas a Excel"})),
        "Anotada en el backlog como #1."
    );
    // El nombre anterior sigue valiendo (sesiones ya abiertas).
    call(
        7,
        "idea_add",
        json!({"title": "App para talleres", "scope": "general"}),
    );
    assert_eq!(
        call(
            8,
            "plan_add",
            json!({"title": "Vacunas por lote", "phases": [
                {"title": "Modelo y migración", "branch": "feat/vacunas-lote", "tasks": ["Tabla lotes", "Migración"]},
                {"title": "Pantalla de registro"}
            ]})
        ),
        "Plan #3 creado con 2 fases."
    );
    let started = call(
        9,
        "plan_update",
        json!({"id": 3, "phase": 1, "phase_status": "in_progress"}),
    );
    assert!(started.starts_with("#3 ◐ Vacunas por lote"), "{started}");
    assert!(
        started.contains("1. ◐ Modelo y migración [feat/vacunas-lote]"),
        "{started}"
    );
    assert!(call(10, "project_context", json!({})).contains("fase 1 de 2: Modelo y migración"));
    let ticked = call(
        11,
        "plan_update",
        json!({"id": 3, "phase": 1, "task": 1, "phase_status": "done"}),
    );
    assert!(
        ticked.contains("1.1 [x] Tabla lotes") && ticked.contains("1.2 [ ] Migración"),
        "{ticked}"
    );
    let done = call(
        12,
        "plan_update",
        json!({"id": 3, "phase": 2, "phase_status": "done"}),
    );
    assert!(
        done.starts_with("#3 ✓"),
        "todas las fases completadas: {done}"
    );
    let list = call(13, "plans_list", json!({}));
    assert!(
        list.contains("#1 ○ Exportar vacunas a Excel") && !list.contains("#3"),
        "{list}"
    );
    assert!(call(14, "plans_list", json!({"include_done": true})).contains("#3 ✓"));
    assert!(call(15, "plans_list", json!({"scope": "general"})).contains("App para talleres"));

    drop(stdin);
    assert!(
        child.wait().unwrap().success(),
        "el servidor termina al cerrarse stdin"
    );

    // Otro cliente (la CLI) ve lo que guardó el agente.
    let cli = Command::new(FORGE)
        .args(["memory", "search", "lotes", "--project", &project_arg])
        .env("FORGE_DB", &db)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&cli.stdout);
    assert!(
        out.contains("Vacunación por lotes") && out.contains("guardada por claude-code"),
        "{out}"
    );
    let add = Command::new(FORGE)
        .args([
            "memory",
            "add",
            "command",
            "Levantar la base",
            "docker compose up postgres",
            "--project",
            &project_arg,
        ])
        .env("FORGE_DB", &db)
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );

    // La CLI ve los planes del agente, con sus fases (y la lista general aparte).
    let ideas = |extra: &[&str]| {
        let mut args = vec!["ideas", "list", "--project", &project_arg];
        args.extend(extra);
        let out = Command::new(FORGE)
            .args(&args)
            .env("FORGE_DB", &db)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let all = ideas(&["--all"]);
    assert!(all.contains("#1 ○ Exportar vacunas a Excel"), "{all}");
    assert!(
        all.contains("#3 ✓ Vacunas por lote") && all.contains("[feat/vacunas-lote]"),
        "{all}"
    );
    assert!(ideas(&["--general"]).contains("App para talleres"));

    // `forge plans`: ver un plan, tachar una tarea, reabrir una fase.
    let plans = |args: &[&str]| {
        let mut all = vec!["plans"];
        all.extend(args);
        all.extend(["--project", project_arg.as_str()]);
        let out = Command::new(FORGE)
            .args(&all)
            .env("FORGE_DB", &db)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(plans(&["show", "3"]).contains("1.2 [ ] Migración"));
    assert!(plans(&["task", "3", "1.2"]).contains("1.2 [x] Migración"));
    let reopened = plans(&["phase", "3", "2", "backlog"]);
    assert!(
        reopened.starts_with("#3 ◐"),
        "una fase reabierta: en progreso de nuevo\n{reopened}"
    );
    // Recordatorio por mensaje (hook del mod): el plan en curso y la memoria.
    let reminder = plans(&["reminder"]);
    assert!(
        reminder.contains("Plan #3")
            && reminder.contains("plan_update")
            && reminder.contains("memory_save"),
        "{reminder}"
    );
    assert!(plans(&["add", "Modo oscuro"]).contains("backlog"));
    assert!(plans(&[]).contains("Modo oscuro"));
}
