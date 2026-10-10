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
            let suffix = format!("{id}.jsonl");
            let file = subdirs(&from.join("sessions"))
                .iter()
                .flat_map(|y| subdirs(y))
                .flat_map(|m| subdirs(&m))
                .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
                .map(|e| e.path())
                .find(|p| p.to_string_lossy().ends_with(&suffix))
                .ok_or_else(missing)?;
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
            std::io::BufRead::read_line(
                &mut std::io::BufReader::new(std::fs::File::open(&path).ok()?),
                &mut first,
            )
            .ok()?;
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
        std::fs::write(
            day.join("rollout-a.jsonl"),
            meta("a", "/Users/a/mi proyecto", "cli"),
        )
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
        let _ = std::fs::remove_dir_all(tmp);
    }
}
