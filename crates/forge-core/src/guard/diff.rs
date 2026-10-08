// Patrones de rutas estilo gitignore y diff de Git por etapa.
use super::*;

// ---------------------------------------------------------------- patrones de rutas

/// Glob estilo gitignore: sin `/` compara el nombre del archivo en cualquier carpeta;
/// `*` y `?` dentro de un segmento, `**` cualquier número de carpetas, `dir/` = `dir/**`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches("./");
    let pattern = match pattern.strip_suffix('/') {
        Some(dir) => format!("{dir}/**"),
        None => pattern.to_string(),
    };
    if !pattern.contains('/') {
        return segment_match(
            pattern.as_bytes(),
            path.rsplit('/').next().unwrap_or(path).as_bytes(),
        );
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    segments_match(&p, &s)
}

pub(crate) fn segments_match(p: &[&str], s: &[&str]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(&"**") => (0..=s.len()).any(|i| segments_match(&p[1..], &s[i..])),
        Some(seg) => {
            !s.is_empty()
                && segment_match(seg.as_bytes(), s[0].as_bytes())
                && segments_match(&p[1..], &s[1..])
        }
    }
}

pub(crate) fn segment_match(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            segment_match(&p[1..], s) || (!s.is_empty() && segment_match(p, &s[1..]))
        }
        (Some(b'?'), Some(_)) => segment_match(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => segment_match(&p[1..], &s[1..]),
        _ => false,
    }
}

// ---------------------------------------------------------------- diff

#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
    /// El archivo ya no existe en el árbol (se está borrando).
    pub removed: bool,
}

#[derive(Debug, Default)]
pub struct Diff {
    pub files: Vec<FileChange>,
    /// Qué se evaluó, para mostrarlo ("cambios preparados", "commits sin subir vs origin/main"...).
    pub description: String,
    /// Comando para ver este mismo diff en un terminal.
    pub command: String,
    /// Líneas añadidas (ruta, número de línea, texto) para buscar secretos.
    pub added_lines: Vec<(String, usize, String)>,
    /// En la etapa commit: no había nada preparado y se evaluó todo el árbol de trabajo.
    pub unstaged: bool,
    /// Identidad del contenido evaluado (sha256 del patch). Cambia si cambia cualquier línea.
    pub candidate: String,
    /// Argumentos de `git diff` que producen este diff (para pedir el mismo a un modelo).
    pub range: Vec<String>,
    pub branch: String,
}

pub(crate) const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub(crate) fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("no se pudo ejecutar git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub fn repo_root(path: &Path) -> Result<PathBuf, String> {
    git(path, &["rev-parse", "--show-toplevel"])
        .map(|s| PathBuf::from(s.trim()))
        .map_err(|_| "no es un repositorio Git".to_string())
}

pub(crate) fn has_head(repo: &Path) -> bool {
    git(repo, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok()
}

/// Rama principal del remoto (origin/HEAD), o main/master locales.
pub(crate) fn default_branch(repo: &Path) -> Result<String, String> {
    if let Ok(r) = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        return Ok(r.trim().to_string());
    }
    for candidate in ["origin/main", "origin/master", "main", "master"] {
        if git(repo, &["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return Ok(candidate.to_string());
        }
    }
    Err("no se encontró la rama base (configura `base` en [pull_request])".into())
}

/// `push_range`: rango exacto que se va a subir (lo da el hook pre-push).
pub fn collect_diff(
    repo: &Path,
    stage: Stage,
    rules: &Rules,
    push_range: Option<&str>,
) -> Result<Diff, String> {
    let mut diff = Diff::default();
    let range: Vec<String> = match stage {
        Stage::Commit => {
            let staged = git(
                repo,
                &["diff", "--cached", "--numstat", "-z", "--no-renames"],
            )?;
            if !staged.is_empty() {
                diff.description = "cambios preparados (git add)".into();
                diff.command = "git diff --cached".into();
                vec!["--cached".into()]
            } else {
                diff.unstaged = true;
                diff.description = "nada preparado: se evalúan todos los cambios sin commit".into();
                diff.command = "git status --short && git diff HEAD".into();
                let base = if has_head(repo) { "HEAD" } else { EMPTY_TREE };
                vec![base.into()]
            }
        }
        Stage::Push if push_range.is_some() => {
            let range = push_range.unwrap_or_default();
            diff.description = format!("commits a subir ({range})");
            diff.command = format!("git log --oneline {range} && git diff {range}");
            vec![range.to_string()]
        }
        Stage::Push | Stage::PullRequest => {
            if !has_head(repo) {
                return Err("el repositorio no tiene commits todavía".into());
            }
            let base = match (stage, &rules.pull_request.base) {
                (Stage::PullRequest, Some(base)) => base.clone(),
                (Stage::Push, _) => match git(
                    repo,
                    &[
                        "rev-parse",
                        "--abbrev-ref",
                        "--symbolic-full-name",
                        "@{upstream}",
                    ],
                ) {
                    Ok(upstream) => upstream.trim().to_string(),
                    Err(_) => default_branch(repo)?,
                },
                _ => default_branch(repo)?,
            };
            diff.description = match stage {
                Stage::Push => format!("commits sin subir (vs {base})"),
                _ => format!("cambios de la rama vs {base}"),
            };
            diff.command = format!("git diff {base}...HEAD");
            vec![format!("{base}...HEAD")]
        }
    };
    diff.range = range.clone();
    let range: Vec<&str> = range.iter().map(String::as_str).collect();

    let numstat = git(
        repo,
        &[&["diff", "--numstat", "-z", "--no-renames"], &range[..]].concat(),
    )?;
    diff.files = parse_numstat(&numstat);
    let patch = git(
        repo,
        &[
            &["diff", "-U0", "--no-color", "--no-renames", "--no-ext-diff"],
            &range[..],
        ]
        .concat(),
    )?;
    diff.added_lines = parse_added_lines(&patch);
    let mut hasher = Sha256::new();
    hasher.update(format!("{stage:?}\0").as_bytes());
    hasher.update(patch.as_bytes());

    // Archivos nuevos sin seguimiento (solo al evaluar el árbol de trabajo).
    if diff.unstaged {
        let untracked = git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for path in untracked.split('\0').filter(|p| !p.is_empty()) {
            let (lines, binary) = read_text(&repo.join(path));
            hasher.update(format!("\0nuevo:{path}\0{}", lines.join("\n")).as_bytes());
            diff.files.push(FileChange {
                path: path.into(),
                added: lines.len(),
                deleted: 0,
                binary,
                removed: false,
            });
            diff.added_lines.extend(
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, l)| (path.to_string(), i + 1, l)),
            );
        }
    }
    for f in &mut diff.files {
        f.removed = !repo.join(&f.path).exists();
    }
    diff.candidate = format!("sha256:{}", hex(&hasher.finalize()));
    diff.branch = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|b| b.trim().to_string())
        .unwrap_or_default();
    Ok(diff)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Líneas de un archivo de texto (vacío si es binario o mayor de 1 MB).
pub(crate) fn read_text(path: &Path) -> (Vec<String>, bool) {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() <= 1 << 20 && !bytes.contains(&0) => (
            String::from_utf8_lossy(&bytes)
                .lines()
                .map(String::from)
                .collect(),
            false,
        ),
        Ok(_) => (Vec::new(), true),
        Err(_) => (Vec::new(), false),
    }
}

/// `git diff --numstat -z`: "añadidas\tborradas\truta\0" ("-" en binarios).
pub(crate) fn parse_numstat(text: &str) -> Vec<FileChange> {
    text.split('\0')
        .filter_map(|record| {
            let mut parts = record.splitn(3, '\t');
            let (added, deleted, path) = (parts.next()?, parts.next()?, parts.next()?);
            let binary = added == "-";
            Some(FileChange {
                path: path.trim_start_matches('\n').to_string(),
                added: added.parse().unwrap_or(0),
                deleted: deleted.parse().unwrap_or(0),
                binary,
                removed: false,
            })
        })
        .collect()
}

/// Líneas añadidas de un patch unificado con su ruta y número de línea nuevo.
pub(crate) fn parse_added_lines(patch: &str) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let mut file: Option<String> = None;
    let mut line_no = 0;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            file = path.strip_prefix("b/").map(String::from);
        } else if let Some(hunk) = line.strip_prefix("@@ ") {
            // @@ -a,b +c,d @@
            line_no = hunk
                .split_whitespace()
                .find_map(|p| p.strip_prefix('+'))
                .and_then(|p| p.split(',').next()?.parse().ok())
                .unwrap_or(0);
        } else if let (Some(text), Some(f)) = (line.strip_prefix('+'), &file) {
            out.push((f.clone(), line_no, text.to_string()));
            line_no += 1;
        } else if line.starts_with(' ') {
            line_no += 1;
        }
    }
    out
}
