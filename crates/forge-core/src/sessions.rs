// Última sesión de un agente en una carpeta, leída de los archivos que guarda el propio
// agente: para reanudarla con un clic y mostrar de qué iba.
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, PartialEq)]
pub struct LastSession {
    pub id: String,
    /// Última escritura (segundos Unix).
    pub when: i64,
    /// Título que le puso el agente o lo último que se le pidió.
    pub topic: String,
}

/// Comando para reanudar justo esa sesión (`command` es el de abrir el agente).
pub fn resume_command(program: &str, command: &str, id: &str) -> Option<String> {
    match program {
        "claude" => Some(format!("{command} --resume {id}")),
        "codex" => Some(format!("{command} resume {id}")),
        _ => None,
    }
}

/// Última sesión interactiva del agente en `cwd` (las de modo no interactivo no cuentan).
/// `config`: carpeta de la cuenta; sin ella, la de siempre (o la de su variable de entorno).
pub fn last_session(program: &str, cwd: &Path, config: Option<&Path>) -> Option<LastSession> {
    let dir = config_dir(program, config)?;
    match program {
        "claude" => claude(&dir, cwd),
        "codex" => codex(&dir, cwd),
        _ => None,
    }
}

/// Carpeta de configuración del agente: la de la cuenta o, sin ella, la de siempre (o la
/// de su variable de entorno).
fn config_dir(program: &str, account: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = account {
        return Some(dir.to_path_buf());
    }
    let (var, default) = match program {
        "claude" => ("CLAUDE_CONFIG_DIR", ".claude"),
        "codex" => ("CODEX_HOME", ".codex"),
        _ => return None,
    };
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(std::env::var_os(var).map_or_else(|| home.join(default), PathBuf::from))
}

/// Pasa la conversación `id` de una cuenta a otra (carpetas `from` → `to`; `None` = la
/// principal) para reanudarla allí con `resume_command`.
pub fn copy_session(
    program: &str,
    cwd: &Path,
    id: &str,
    from: Option<&Path>,
    to: Option<&Path>,
) -> Result<(), String> {
    let missing = || crate::tr!("No encuentro esa conversación.").to_string();
    let (from, to) = (
        config_dir(program, from).ok_or_else(missing)?,
        config_dir(program, to).ok_or_else(missing)?,
    );
    let copy = |rel: &Path| -> Result<(), String> {
        let target = to.join(rel);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::copy(from.join(rel), target)
            .map(drop)
            .map_err(|e| e.to_string())
    };
    match program {
        "claude" => {
            let rel = Path::new("projects")
                .join(claude_dir_name(cwd))
                .join(format!("{id}.jsonl"));
            if !from.join(&rel).is_file() {
                return Err(missing());
            }
            copy(&rel)
        }
        "codex" => {
            // sessions/AAAA/MM/DD/rollout-…-<id>.jsonl, y su nombre en session_index.jsonl.
            let file = codex_file(&from, id).ok_or_else(missing)?;
            let rel = file.strip_prefix(&from).map_err(|e| e.to_string())?;
            copy(rel)?;
            let lines: String = std::fs::read_to_string(from.join("session_index.jsonl"))
                .unwrap_or_default()
                .lines()
                .filter(|l| l.contains(&format!("\"{id}\"")))
                .map(|l| format!("{l}\n"))
                .collect();
            if !lines.is_empty() {
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(to.join("session_index.jsonl"))
                    .and_then(|mut f| f.write_all(lines.as_bytes()))
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        _ => Err(missing()),
    }
}

/// Uso del plan de una cuenta de Codex (`account`: su carpeta; `None` = la principal),
/// sacado de su sesión más reciente: Codex guarda allí sus ventanas de uso.
pub fn codex_limits(account: Option<&Path>) -> Option<crate::accounts::AccountLimits> {
    let home = config_dir("codex", account)?;
    let newest = |dir: &Path| {
        let mut v = subdirs(dir);
        v.sort();
        v.pop()
    };
    let day = newest(&newest(&newest(&home.join("sessions"))?)?)?;
    let mut files: Vec<(i64, PathBuf)> = std::fs::read_dir(&day)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .map(|p| (mtime(&p), p))
        .collect();
    // Los nombres llevan la fecha: desempatan si se escribieron en el mismo segundo. La
    // más reciente con medidas: una sesión abierta y cerrada sin conversar no tiene.
    files.sort();
    let (at, limits) = files.iter().rev().find_map(|(at, file)| {
        let text = tail(file, 256 * 1024);
        let found = json_lines(&text).rev().find(|e| {
            e["payload"]["type"] == "token_count" && e["payload"]["rate_limits"].is_object()
        })?;
        Some((*at, found))
    })?;
    let windows = ["primary", "secondary"]
        .iter()
        .filter_map(|k| {
            let w = &limits["payload"]["rate_limits"][k];
            let percent = w["used_percent"].as_f64()?;
            let kind = match w["window_minutes"].as_i64() {
                Some(300) => "five_hour".to_string(),
                Some(10_080) => "seven_day".to_string(),
                Some(m) if m >= 1_440 => format!("{}d", m / 1_440),
                Some(m) => format!("{}h", m / 60),
                None => k.to_string(),
            };
            Some(crate::accounts::UsageWindow {
                kind,
                percent_used: percent,
                resets_at: w["resets_at"].as_i64().map(|t| t.to_string()),
            })
        })
        .collect::<Vec<_>>();
    (!windows.is_empty()).then_some(crate::accounts::AccountLimits { at, windows })
}

/// El archivo de una sesión de Codex por su id (`sessions/AAAA/MM/DD/rollout-…-<id>.jsonl`).
fn codex_file(home: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("{id}.jsonl");
    subdirs(&home.join("sessions"))
        .iter()
        .flat_map(|y| subdirs(y))
        .flat_map(|m| subdirs(&m))
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .find(|p| p.to_string_lossy().ends_with(&suffix))
}

/// Lo que se saca de una sesión para pasársela a otro agente.
#[derive(Debug, Default, PartialEq)]
struct Digest {
    topic: String,
    /// Los mensajes del usuario, en orden.
    asks: Vec<String>,
    last_answer: String,
    /// Archivos que el agente editó, sin repetir, en el orden en que los tocó.
    files: Vec<String>,
    last_error: String,
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
}

/// Lo que el usuario escribió de verdad: sin las etiquetas que añade el agente (comandos,
/// recordatorios, avisos de tareas, contexto del entorno) ni las líneas automáticas de las
/// imágenes; lo pegado se queda, sin las etiquetas que lo envuelven. Vacío si no queda nada.
fn user_text(text: &str) -> String {
    const SYSTEM: [&str; 9] = [
        "<command-",
        "<local-command",
        "<system-reminder",
        "<user-prompt-submit-hook",
        "<task-notification",
        "<environment_context",
        "<user_instructions",
        "<bash-",
        "# AGENTS.md",
    ];
    let t = text.trim();
    if SYSTEM.iter().any(|tag| t.starts_with(tag)) {
        return String::new();
    }
    t.lines()
        .filter(|l| !l.trim_start().starts_with("[Image"))
        .filter(|l| {
            let l = l.trim();
            !(l.starts_with("<pasted_content") || l.starts_with("</pasted_content"))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn digest_claude(lines: impl Iterator<Item = serde_json::Value>) -> Digest {
    let mut d = Digest::default();
    for e in lines {
        if e["type"] == "ai-title" {
            d.topic = e["aiTitle"].as_str().unwrap_or_default().to_string();
            continue;
        }
        let content = &e["message"]["content"];
        match e["type"].as_str() {
            Some("user") => {
                if let Some(text) = content.as_str().map(user_text)
                    && !text.is_empty()
                {
                    d.asks.push(text);
                }
                for item in content.as_array().into_iter().flatten() {
                    match item["type"].as_str() {
                        Some("text") => {
                            let text = user_text(item["text"].as_str().unwrap_or_default());
                            if !text.is_empty() {
                                d.asks.push(text);
                            }
                        }
                        Some("tool_result") if item["is_error"] == true => {
                            let text = item["content"]
                                .as_str()
                                .map(str::to_string)
                                .or_else(|| item["content"][0]["text"].as_str().map(str::to_string))
                                .unwrap_or_default();
                            d.last_error = text;
                        }
                        _ => {}
                    }
                }
            }
            Some("assistant") => {
                for item in content.as_array().into_iter().flatten() {
                    match item["type"].as_str() {
                        Some("text") => {
                            d.last_answer = item["text"].as_str().unwrap_or_default().to_string()
                        }
                        Some("tool_use")
                            if matches!(
                                item["name"].as_str(),
                                Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
                            ) =>
                        {
                            let input = &item["input"];
                            if let Some(f) = input["file_path"]
                                .as_str()
                                .or(input["notebook_path"].as_str())
                                && !d.files.iter().any(|x| x == f)
                            {
                                d.files.push(f.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    d
}

fn digest_codex(lines: impl Iterator<Item = serde_json::Value>) -> Digest {
    let mut d = Digest::default();
    for e in lines {
        let p = &e["payload"];
        if e["type"] != "response_item" {
            continue;
        }
        match p["type"].as_str() {
            Some("message") => {
                let text: String = p["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let asked = user_text(&text);
                match p["role"].as_str() {
                    Some("user") if !asked.is_empty() => d.asks.push(asked),
                    Some("assistant") if !text.trim().is_empty() => d.last_answer = text,
                    _ => {}
                }
            }
            Some("function_call" | "custom_tool_call") => {
                let args = p["arguments"]
                    .as_str()
                    .or(p["input"].as_str())
                    .unwrap_or_default();
                for line in args.split(['\n', '"']) {
                    for prefix in ["*** Update File: ", "*** Add File: ", "*** Delete File: "] {
                        if let Some(f) = line.trim().strip_prefix(prefix)
                            && !d.files.iter().any(|x| x == f.trim())
                        {
                            d.files.push(f.trim().to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    d
}

/// Resumen para seguir en otro agente la última conversación de `program` en `cwd`
/// (`config`: la carpeta de su cuenta). Corto a propósito (~3 KB): el otro agente lee
/// los planes y Forge Memory por MCP para el resto. `None` si no hay sesión.
pub fn handoff_brief(program: &str, cwd: &Path, config: Option<&Path>) -> Option<String> {
    let (name, lines) = digest_last(program, cwd, config)?;
    if lines.is_empty() {
        return None;
    }
    let mut out = vec![crate::tr!(
        "Sigo una conversación que empezó en {name}. Resumen del traspaso:",
        name = name
    )];
    out.extend(lines);
    out.push(crate::tr!("Antes de seguir, revisa los planes en curso (plans_list) y Forge Memory (memory_search), y pregúntame si algo no está claro.").into());
    Some(out.join("\n"))
}

/// La última conversación del agente en el proyecto como nota de Forge Memory (se guarda
/// al cerrar su terminal, sin depender de que el agente guarde nada).
pub fn session_summary(program: &str, cwd: &Path, config: Option<&Path>) -> Option<String> {
    let (name, lines) = digest_last(program, cwd, config)?;
    if lines.is_empty() {
        return None;
    }
    let mut out = vec![crate::tr!("Última sesión de {name}:", name = name)];
    out.extend(lines);
    Some(out.join("\n"))
}

/// Tema, objetivo, últimas peticiones, última respuesta, archivos y último error de la
/// última sesión del agente en `cwd`.
fn digest_last(
    program: &str,
    cwd: &Path,
    config: Option<&Path>,
) -> Option<(&'static str, Vec<String>)> {
    let session = last_session(program, cwd, config)?;
    let dir = config_dir(program, config)?;
    let file = match program {
        "claude" => dir
            .join("projects")
            .join(claude_dir_name(cwd))
            .join(format!("{}.jsonl", session.id)),
        "codex" => codex_file(&dir, &session.id)?,
        _ => return None,
    };
    // El principio (el objetivo) y lo último; una sesión puede pesar decenas de MB.
    let head = {
        let mut buf = vec![0; 256 * 1024];
        let n = std::fs::File::open(&file).ok()?.read(&mut buf).ok()?;
        String::from_utf8_lossy(&buf[..n]).into_owned()
    };
    let recent = tail(&file, 1024 * 1024);
    let digest = |text: &str| match program {
        "claude" => digest_claude(json_lines(text)),
        _ => digest_codex(json_lines(text)),
    };
    let start = digest(&head);
    let end = digest(&recent);
    let name = match program {
        "claude" => "Claude Code",
        _ => "Codex",
    };
    let mut out = Vec::new();
    let topic = if end.topic.is_empty() {
        session.topic.clone()
    } else {
        end.topic.clone()
    };
    if !topic.is_empty() {
        out.push(crate::tr!("Tema: {p0}", p0 = topic));
    }
    if let Some(goal) = start.asks.first() {
        out.push(crate::tr!(
            "Objetivo inicial: {p0}",
            p0 = clip(&one_line(goal), 400)
        ));
    }
    let recent_asks: Vec<&String> = end.asks.iter().rev().take(3).collect();
    if !recent_asks.is_empty() {
        out.push(crate::tr!("Últimas peticiones:").into());
        for ask in recent_asks.into_iter().rev() {
            out.push(format!("- {}", clip(&one_line(ask), 300)));
        }
    }
    if !end.last_answer.is_empty() {
        out.push(crate::tr!(
            "Última respuesta de {name}: {p0}",
            name = name,
            p0 = clip(&end.last_answer, 800)
        ));
    }
    let files: Vec<&String> = end.files.iter().rev().take(15).collect();
    if !files.is_empty() {
        let list: Vec<&str> = files.into_iter().rev().map(String::as_str).collect();
        out.push(crate::tr!("Archivos tocados: {p0}", p0 = list.join(", ")));
    }
    if !end.last_error.is_empty() {
        out.push(crate::tr!(
            "Último error: {p0}",
            p0 = clip(&one_line(&end.last_error), 300)
        ));
    }
    Some((name, out))
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// Claude Code guarda las sesiones de cada carpeta en `projects/<ruta con lo no
/// alfanumérico como '-'>`.
fn claude_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn mtime(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// Final del archivo (las sesiones pueden pesar decenas de MB; lo útil se repite al final).
fn tail(path: &Path, bytes: u64) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(bytes)));
    let mut buf = Vec::new();
    let _ = file.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn json_lines(text: &str) -> impl DoubleEndedIterator<Item = serde_json::Value> + '_ {
    text.lines().filter_map(|l| serde_json::from_str(l).ok())
}

fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(80) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line,
    }
}

/// Claude Code: `<config>/projects/<cwd con lo no alfanumérico como '-'>/<id>.jsonl`.
fn claude(config: &Path, cwd: &Path) -> Option<LastSession> {
    let mut files: Vec<(i64, PathBuf)> =
        std::fs::read_dir(config.join("projects").join(claude_dir_name(cwd)))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .map(|p| (mtime(&p), p))
            .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().take(5).find_map(|(when, path)| {
        let text = tail(&path, 256 * 1024);
        let entries: Vec<serde_json::Value> = json_lines(&text).collect();
        // `claude -p` (revisores, scripts) se marca como sdk-cli.
        if entries.iter().any(|e| e["entrypoint"] == "sdk-cli") {
            return None;
        }
        let field = |kind: &str, key: &str| {
            entries
                .iter()
                .rev()
                .find(|e| e["type"] == kind)
                .and_then(|e| e[key].as_str().map(one_line))
        };
        Some(LastSession {
            id: path.file_stem()?.to_string_lossy().into_owned(),
            when,
            topic: field("ai-title", "aiTitle")
                .or_else(|| field("last-prompt", "lastPrompt"))
                .unwrap_or_default(),
        })
    })
}

/// Codex: `<home>/sessions/AAAA/MM/DD/rollout-….jsonl`, con la carpeta en la primera línea;
/// el nombre de cada hilo, en `session_index.jsonl`.
fn codex(home: &Path, cwd: &Path) -> Option<LastSession> {
    // ponytail: solo los 14 días con sesiones más recientes; ampliar si alguien lo echa en falta.
    let mut days = Vec::new();
    let sorted = |dir: &Path| {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|r| r.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        v.sort();
        v.reverse();
        v
    };
    'out: for year in sorted(&home.join("sessions")) {
        for month in sorted(&year) {
            for day in sorted(&month) {
                days.push(day);
                if days.len() >= 14 {
                    break 'out;
                }
            }
        }
    }
    let found = days.iter().find_map(|day| {
        let mut files: Vec<(i64, PathBuf)> = std::fs::read_dir(day)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .map(|p| (mtime(&p), p))
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.0));
        files.into_iter().find_map(|(when, path)| {
            let mut first = String::new();
            let mut reader = std::io::BufReader::new(std::fs::File::open(&path).ok()?);
            std::io::BufRead::read_line(&mut reader, &mut first).ok()?;
            // Una sesión abierta y cerrada sin conversar (solo la cabecera) no cuenta.
            let mut rest = vec![0; 256 * 1024];
            let n = reader.read(&mut rest).ok()?;
            if !String::from_utf8_lossy(&rest[..n]).contains("\"response_item\"") {
                return None;
            }
            let meta: serde_json::Value = serde_json::from_str(&first).ok()?;
            let p = &meta["payload"];
            if p["cwd"].as_str() != Some(&*cwd.to_string_lossy()) || p["source"] == "exec" {
                return None;
            }
            Some((p["id"].as_str()?.to_string(), when))
        })
    });
    let (id, when) = found?;
    let index = tail(&home.join("session_index.jsonl"), 512 * 1024);
    let topic = json_lines(&index)
        .rev()
        .find(|e| e["id"] == id.as_str())
        .and_then(|e| e["thread_name"].as_str().map(one_line))
        .unwrap_or_default();
    Some(LastSession { id, when, topic })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_last_interactive_session() {
        let tmp = std::env::temp_dir().join(format!("forge-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let cwd = Path::new("/Users/a/mi proyecto");

        // Claude: la más nueva es de `claude -p`, así que vale la anterior.
        let dir = tmp.join("claude/projects/-Users-a-mi-proyecto");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("old.jsonl"),
            "{\"type\":\"user\",\"entrypoint\":\"cli\"}\n{\"type\":\"last-prompt\",\"lastPrompt\":\"arregla el login\"}\n{\"type\":\"ai-title\",\"aiTitle\":\"Login con Google\"}\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(
            dir.join("print.jsonl"),
            "{\"type\":\"user\",\"entrypoint\":\"sdk-cli\"}\n",
        )
        .unwrap();
        let s = claude(&tmp.join("claude"), cwd).unwrap();
        assert_eq!(
            (s.id.as_str(), s.topic.as_str()),
            ("old", "Login con Google")
        );
        assert!(claude(&tmp.join("claude"), Path::new("/otro")).is_none());

        // Codex: la de esta carpeta (no la de `codex exec` ni la de otra carpeta).
        let day = tmp.join("codex/sessions/2026/10/09");
        std::fs::create_dir_all(&day).unwrap();
        let meta = |id: &str, cwd: &str, source: &str| {
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"{cwd}\",\"source\":\"{source}\"}}}}\n"
            )
        };
        let said = "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"Migra la base\"}]}}\n";
        std::fs::write(
            day.join("rollout-a.jsonl"),
            meta("a", "/Users/a/mi proyecto", "cli") + said,
        )
        .unwrap();
        // Más reciente pero sin conversar (Codex abierto y cerrado): no cuenta.
        let empty = day.join("rollout-d.jsonl");
        std::fs::write(&empty, meta("d", "/Users/a/mi proyecto", "cli")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&empty)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
            .unwrap();
        std::fs::write(
            day.join("rollout-b.jsonl"),
            meta("b", "/Users/a/mi proyecto", "exec"),
        )
        .unwrap();
        std::fs::write(day.join("rollout-c.jsonl"), meta("c", "/otro", "cli")).unwrap();
        std::fs::write(
            tmp.join("codex/session_index.jsonl"),
            "{\"id\":\"a\",\"thread_name\":\"viejo\"}\n{\"id\":\"a\",\"thread_name\":\"Migrar a PG17\"}\n",
        )
        .unwrap();
        let s = codex(&tmp.join("codex"), cwd).unwrap();
        assert_eq!((s.id.as_str(), s.topic.as_str()), ("a", "Migrar a PG17"));

        assert_eq!(
            resume_command("claude", "claude --model opus", "x").as_deref(),
            Some("claude --model opus --resume x")
        );
        assert_eq!(resume_command("opencode", "opencode", "x"), None);

        // Resumen de traspaso de una sesión de Claude.
        std::fs::write(
            dir.join("brief.jsonl"),
            [
                r#"{"type":"user","entrypoint":"cli","message":{"content":"Quiero recibos por email tras el pago"}}"#,
                r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"src/receipt.ts","content":"x"}},{"type":"text","text":"Listo el modelo de recibo."}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":"tsc: Type error in receipt.ts"}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"text","text":"ahora el envío con Resend\n[Image: original 100x100]"}]}}"#,
                r#"{"type":"ai-title","aiTitle":"Recibos por email"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let brief = handoff_brief("claude", cwd, Some(&tmp.join("claude"))).unwrap();
        for part in [
            "Claude Code",
            "Recibos por email",
            "Quiero recibos por email",
            "ahora el envío con Resend",
            "Listo el modelo de recibo",
            "src/receipt.ts",
            "Type error",
            "plans_list",
        ] {
            assert!(brief.contains(part), "falta «{part}» en:\n{brief}");
        }
        assert!(
            !brief.contains("/clear"),
            "las etiquetas de sistema no son peticiones"
        );
        // La misma sesión como nota de memoria: sin el saludo ni la petición del traspaso.
        let note = session_summary("claude", cwd, Some(&tmp.join("claude"))).unwrap();
        assert!(note.starts_with("Última sesión de Claude Code:"));
        assert!(note.contains("ahora el envío con Resend") && note.contains("src/receipt.ts"));
        assert!(!note.contains("traspaso") && !note.contains("plans_list"));

        // Pasar la conversación a otra cuenta: la encuentra allí.
        let other = tmp.join("claude-2");
        copy_session(
            "claude",
            cwd,
            "old",
            Some(&tmp.join("claude")),
            Some(&other),
        )
        .unwrap();
        assert_eq!(claude(&other, cwd).unwrap().topic, "Login con Google");
        assert!(
            copy_session(
                "claude",
                cwd,
                "nada",
                Some(&tmp.join("claude")),
                Some(&other)
            )
            .is_err()
        );
        let other = tmp.join("codex-2");
        copy_session("codex", cwd, "a", Some(&tmp.join("codex")), Some(&other)).unwrap();
        let s = codex(&other, cwd).unwrap();
        assert_eq!((s.id.as_str(), s.topic.as_str()), ("a", "Migrar a PG17"));

        // Uso del plan de Codex: la última medida de su sesión más reciente.
        assert!(
            codex_limits(Some(&tmp.join("codex"))).is_none(),
            "sin medidas"
        );
        let rate = |p: f64| {
            format!(
                "{{\"payload\":{{\"type\":\"token_count\",\"rate_limits\":{{\"primary\":{{\"used_percent\":{p},\"window_minutes\":300,\"resets_at\":1}},\"secondary\":{{\"used_percent\":7.0,\"window_minutes\":43200}}}}}}}}\n"
            )
        };
        std::fs::write(
            day.join("rollout-z.jsonl"),
            meta("z", "/x", "cli") + said + &rate(10.0) + &rate(64.0),
        )
        .unwrap();
        let l = codex_limits(Some(&tmp.join("codex"))).unwrap();
        let got: Vec<(&str, f64)> = l
            .windows
            .iter()
            .map(|w| (w.kind.as_str(), w.percent_used))
            .collect();
        assert_eq!(got, [("five_hour", 64.0), ("30d", 7.0)]);
        let _ = std::fs::remove_dir_all(tmp);
    }
}
