// Fase 10: la memoria del proyecto como servidor MCP real (`forge mcp`), compartida
// entre clientes (un agente guarda, otro cliente la encuentra).
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

#[test]
fn mcp_server_shares_memory_between_clients() {
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
            "memory_list",
            "memory_delete",
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
}
