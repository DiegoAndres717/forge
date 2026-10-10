// Git para el panel de la app: estado (rama, cambios), ramas, historial y acciones
// sencillas (preparar, quitar, descartar, cambiar de rama). Todo con la CLI de git.
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// `None` con HEAD separado.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    pub path: String,
    /// Columna del índice de `git status --porcelain` (' ' = sin cambios preparados).
    pub staged: char,
    /// Columna del árbol de trabajo ('?' = sin seguimiento).
    pub unstaged: char,
}

impl FileChange {
    pub fn is_staged(&self) -> bool {
        !matches!(self.staged, ' ' | '?')
    }

    pub fn is_unstaged(&self) -> bool {
        self.unstaged != ' '
    }

    pub fn untracked(&self) -> bool {
        self.unstaged == '?'
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub author: String,
    pub when: i64,
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_OPTIONAL_LOCKS", "0") // leer el estado no bloquea a otros git
        .output()
        .map_err(|e| crate::tr!("no se pudo ejecutar git: {e}", e = e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Identidad estable de un repositorio: el remoto (origin o el primero) normalizado, más la
/// subcarpeta si el proyecto es parte del repo. `None` si no es un repo o no tiene remoto.
pub fn remote_identity(repo: &Path) -> Option<String> {
    let remotes = git(repo, &["config", "--get-regexp", r"^remote\..*\.url$"]).ok()?;
    let urls: Vec<(&str, &str)> = remotes.lines().filter_map(|l| l.split_once(' ')).collect();
    let url = urls
        .iter()
        .find(|(k, _)| *k == "remote.origin.url")
        .or(urls.first())?
        .1;
    let prefix = git(repo, &["rev-parse", "--show-prefix"]).unwrap_or_default();
    let prefix = prefix.trim().trim_end_matches('/');
    let base = normalize_remote(url);
    Some(if prefix.is_empty() {
        base
    } else {
        format!("{base}/{prefix}")
    })
}

/// `git@github.com:Org/Repo.git`, `https://user@github.com/org/repo` y
/// `ssh://git@github.com:22/org/repo.git` → `github.com/org/repo`.
pub fn normalize_remote(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (scheme, rest) = match url.split_once("://") {
        Some((_, rest)) => (true, rest),
        None => (false, url),
    };
    // Usuario (`git@`), solo si va antes de la ruta.
    let rest = match rest.find('@') {
        Some(at) if at < rest.find('/').unwrap_or(usize::MAX) => &rest[at + 1..],
        _ => rest,
    };
    let split = if scheme {
        rest.split_once('/')
            .map(|(host, path)| (host.split(':').next().unwrap_or(host), path))
    } else {
        // Forma scp (`host:ruta`); una ruta local se queda tal cual.
        rest.split_once(':').filter(|(host, _)| !host.contains('/'))
    };
    match split {
        Some((host, path)) => format!("{host}/{}", path.trim_start_matches('/')).to_lowercase(),
        None => rest.to_lowercase(),
    }
}

pub fn status(repo: &Path) -> Result<Status, String> {
    let out = git(
        repo,
        &[
            "status",
            "--porcelain=v1",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )?;
    Ok(parse_status(&out))
}

fn parse_status(out: &str) -> Status {
    let mut status = Status::default();
    let mut entries = out.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        if let Some(head) = entry.strip_prefix("## ") {
            parse_branch(head, &mut status);
            continue;
        }
        let mut chars = entry.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else {
            continue;
        };
        let path = entry.get(3..).unwrap_or_default().to_string();
        // Renombrados/copiados: la ruta de origen viene en la entrada siguiente.
        if matches!(x, 'R' | 'C') {
            entries.next();
        }
        status.files.push(FileChange {
            path,
            staged: x,
            unstaged: y,
        });
    }
    status
}

/// `main...origin/main [ahead 2, behind 1]`, `HEAD (no branch)`, `No commits yet on main`.
fn parse_branch(head: &str, status: &mut Status) {
    let head = head.strip_prefix("No commits yet on ").unwrap_or(head);
    let (names, counts) = head.split_once(" [").unwrap_or((head, ""));
    let (local, upstream) = names.split_once("...").unwrap_or((names, ""));
    if !local.starts_with("HEAD (") {
        status.branch = Some(local.to_string());
    }
    status.upstream = (!upstream.is_empty()).then(|| upstream.to_string());
    for part in counts.trim_end_matches(']').split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            status.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            status.behind = n.parse().unwrap_or(0);
        }
    }
}

pub fn branches(repo: &Path) -> Result<Vec<String>, String> {
    let out = git(
        repo,
        &[
            "branch",
            "--format=%(refname:short)",
            "--sort=-committerdate",
        ],
    )?;
    Ok(out.lines().map(str::to_string).collect())
}

pub fn log(repo: &Path, limit: usize) -> Result<Vec<Commit>, String> {
    let n = format!("-{limit}");
    let out = match git(repo, &["log", &n, "--format=%h%x1f%s%x1f%an%x1f%at"]) {
        Err(e) if e.contains("does not have any commits") => return Ok(Vec::new()),
        other => other?,
    };
    Ok(out
        .lines()
        .filter_map(|l| {
            let mut p = l.split('\x1f');
            Some(Commit {
                hash: p.next()?.to_string(),
                subject: p.next()?.to_string(),
                author: p.next()?.to_string(),
                when: p.next()?.parse().ok()?,
            })
        })
        .collect())
}

pub fn stage(repo: &Path, paths: &[&str]) -> Result<(), String> {
    let mut args = vec!["add", "-A", "--"];
    args.extend(paths);
    git(repo, &args).map(drop)
}

/// Quita del índice (sin tocar el archivo). Sin commits todavía, `reset` no sirve.
pub fn unstage(repo: &Path, paths: &[&str]) -> Result<(), String> {
    let has_head = git(repo, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    let mut args = if has_head {
        vec!["reset", "-q", "HEAD", "--"]
    } else {
        vec!["rm", "--cached", "-r", "-q", "--"]
    };
    args.extend(paths);
    git(repo, &args).map(drop)
}

/// Descarta los cambios de un archivo (irreversible: la app pide confirmación). Los
/// archivos sin seguimiento se borran.
pub fn discard(repo: &Path, file: &FileChange) -> Result<(), String> {
    if file.untracked() {
        return std::fs::remove_file(repo.join(&file.path)).map_err(|e| e.to_string());
    }
    git(
        repo,
        &[
            "restore",
            "--staged",
            "--worktree",
            "--source=HEAD",
            "--",
            &file.path,
        ],
    )
    .map(drop)
}

pub fn switch(repo: &Path, branch: &str) -> Result<(), String> {
    git(repo, &["switch", branch]).map(drop)
}

pub fn create_branch(repo: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return Err(crate::tr!("nombre de rama no válido").to_string());
    }
    git(repo, &["switch", "-c", name]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_normalize_to_the_same_identity() {
        for url in [
            "git@github.com:Org/Repo.git",
            "https://github.com/org/repo",
            "https://user@github.com/org/repo.git/",
            "ssh://git@github.com:22/org/repo.git",
        ] {
            assert_eq!(normalize_remote(url), "github.com/org/repo", "{url}");
        }
        assert_eq!(normalize_remote("/srv/git/repo.git"), "/srv/git/repo");
    }

    #[test]
    fn parses_porcelain_status() {
        let out = "## main...origin/main [ahead 2, behind 1]\0M  src/a.rs\0 M b.rs\0?? new file.txt\0R  new.rs\0old.rs\0";
        let s = parse_status(out);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        let paths: Vec<&str> = s.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/a.rs", "b.rs", "new file.txt", "new.rs"]);
        assert!(s.files[0].is_staged() && !s.files[0].is_unstaged());
        assert!(!s.files[1].is_staged() && s.files[1].is_unstaged());
        assert!(s.files[2].untracked() && !s.files[2].is_staged());
        assert_eq!(
            parse_status("## No commits yet on main\0")
                .branch
                .as_deref(),
            Some("main")
        );
        assert_eq!(parse_status("## HEAD (no branch)\0").branch, None);
    }

    #[test]
    fn stage_unstage_discard_and_branches_on_a_real_repo() {
        let dir = std::env::temp_dir().join(format!("forge-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |c: &str| {
            assert!(
                Command::new("sh")
                    .args(["-c", c])
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        sh("git init -q -b main && git config user.email t@t && git config user.name t");
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        stage(&dir, &["a.txt"]).unwrap();
        assert!(status(&dir).unwrap().files[0].is_staged());
        unstage(&dir, &["a.txt"]).unwrap(); // sin commits todavía
        assert!(status(&dir).unwrap().files[0].untracked());
        sh("git add -A && git commit -qm inicio");
        std::fs::write(dir.join("a.txt"), "2\n").unwrap();
        let file = status(&dir).unwrap().files.remove(0);
        discard(&dir, &file).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "1\n");
        create_branch(&dir, "feat/x").unwrap();
        assert_eq!(status(&dir).unwrap().branch.as_deref(), Some("feat/x"));
        assert!(branches(&dir).unwrap().contains(&"main".to_string()));
        switch(&dir, "main").unwrap();
        assert_eq!(log(&dir, 5).unwrap()[0].subject, "inicio");
        assert!(create_branch(&dir, "con espacio").is_err());
    }
}
