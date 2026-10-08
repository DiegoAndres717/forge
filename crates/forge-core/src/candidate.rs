// Candidato congelado: el árbol exacto que se va a commitear/subir, y una copia aislada
// de él para ejecutar los checks sin tocar el árbol de trabajo ni el index del usuario.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::guard::Stage;

/// Variables que git pone en los hooks y que no deben filtrarse a la copia aislada
/// (con GIT_INDEX_FILE heredado, `git worktree add` escribiría en el index del commit).
const HOOK_ENV: [&str; 3] = ["GIT_INDEX_FILE", "GIT_DIR", "GIT_WORK_TREE"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Identificador legible: `<proyecto>-<árbol corto>`.
    pub id: String,
    /// Árbol de Git con el contenido exacto evaluado.
    pub tree: String,
    pub head: String,
    /// sha256 del diff evaluado (las excepciones "este commit" se atan a él).
    pub diff_hash: String,
    /// Los checks corrieron en una copia aislada porque el árbol de trabajo difería.
    pub isolated: bool,
}

fn git(repo: &Path, args: &[&str], index: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(args).stdin(Stdio::null());
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("no se pudo ejecutar git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Como `git` pero sin las variables de los hooks.
fn git_clean(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(args).stdin(Stdio::null());
    for var in HOOK_ENV {
        cmd.env_remove(var);
    }
    // commit-tree necesita una identidad aunque el usuario no tenga una configurada.
    cmd.env("GIT_AUTHOR_NAME", "Forge")
        .env("GIT_AUTHOR_EMAIL", "forge@localhost");
    cmd.env("GIT_COMMITTER_NAME", "Forge")
        .env("GIT_COMMITTER_EMAIL", "forge@localhost");
    let out = cmd
        .output()
        .map_err(|e| format!("no se pudo ejecutar git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn temp_path(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("forge-{tag}-{}-{nanos}", std::process::id()))
}

/// Árbol con el contenido actual del árbol de trabajo (incluidos archivos nuevos no ignorados),
/// calculado con un index temporal: el index del usuario no se toca.
pub fn worktree_tree(repo: &Path) -> Result<String, String> {
    let index = temp_path("index");
    let result = (|| {
        match git(repo, &["rev-parse", "--verify", "--quiet", "HEAD"], None) {
            Ok(_) => git(repo, &["read-tree", "HEAD"], Some(&index))?,
            Err(_) => git(repo, &["read-tree", "--empty"], Some(&index))?,
        };
        git(repo, &["add", "-A"], Some(&index))?;
        git(repo, &["write-tree"], Some(&index))
    })();
    let _ = std::fs::remove_file(&index);
    result
}

/// Árbol del candidato de una etapa.
/// - commit: el index (lo preparado) o, si no había nada preparado, el árbol de trabajo.
/// - push/pr: el árbol del commit que se sube (HEAD o el extremo del rango del hook).
pub fn candidate_tree(
    repo: &Path,
    stage: Stage,
    unstaged: bool,
    push_range: Option<&str>,
) -> Result<String, String> {
    match stage {
        Stage::Commit if unstaged => worktree_tree(repo),
        // Respeta GIT_INDEX_FILE: en un hook es el index que se va a commitear.
        Stage::Commit => git(repo, &["write-tree"], None),
        _ => {
            let tip = push_range
                .and_then(|r| r.split("..").last())
                .unwrap_or("HEAD");
            git(repo, &["rev-parse", &format!("{tip}^{{tree}}")], None)
        }
    }
}

pub fn freeze(
    repo: &Path,
    name: &str,
    stage: Stage,
    unstaged: bool,
    push_range: Option<&str>,
    diff_hash: &str,
) -> Result<Candidate, String> {
    let tree = candidate_tree(repo, stage, unstaged, push_range)?;
    let head = git(repo, &["rev-parse", "--verify", "--quiet", "HEAD"], None).unwrap_or_default();
    let isolated = worktree_tree(repo)? != tree;
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Ok(Candidate {
        id: format!("{}-{}", slug.trim_matches('-'), &tree[..6.min(tree.len())]),
        tree,
        head,
        diff_hash: diff_hash.to_string(),
        isolated,
    })
}

/// Copia aislada del candidato (`git worktree` temporal). Se elimina al soltarla.
pub struct Snapshot {
    repo: PathBuf,
    pub path: PathBuf,
}

impl Snapshot {
    pub fn create(repo: &Path, tree: &str, head: &str) -> Result<Self, String> {
        // Commit suelto con el árbol del candidato: no mueve ramas ni toca el index.
        let mut args = vec!["commit-tree", tree, "-m", "forge: candidato"];
        if !head.is_empty() {
            args.extend(["-p", head]);
        }
        let commit = git_clean(repo, &args)?;
        let path = temp_path("snapshot");
        git_clean(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                "--quiet",
                &path.to_string_lossy(),
                &commit,
            ],
        )?;
        let snapshot = Self {
            repo: repo.to_path_buf(),
            path,
        };
        snapshot.link_ignored()?;
        Ok(snapshot)
    }

    /// Enlaza lo ignorado que los checks necesitan y no está en Git: dependencias
    /// (node_modules en cualquier nivel), `.env`, cachés de compilación de la raíz…
    // ponytail: se enlazan los ignorados de la raíz y todos los node_modules; otros
    // ecosistemas con dependencias anidadas fuera de Git necesitarán su propia regla.
    fn link_ignored(&self) -> Result<(), String> {
        let ignored = git(
            &self.repo,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
            ],
            None,
        )?;
        for entry in ignored.lines() {
            let rel = entry.trim_end_matches('/');
            let top_level = !rel.contains('/');
            let deps = rel.rsplit('/').next() == Some("node_modules");
            if rel.is_empty() || !(top_level || deps) {
                continue;
            }
            let target = self.path.join(rel);
            if target.exists() {
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::os::unix::fs::symlink(self.repo.join(rel), &target)
                .map_err(|e| format!("{}: {e}", target.display()))?;
        }
        Ok(())
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let path = self.path.to_string_lossy().into_owned();
        if git_clean(&self.repo, &["worktree", "remove", "--force", &path]).is_err() {
            let _ = std::fs::remove_dir_all(&self.path);
            let _ = git_clean(&self.repo, &["worktree", "prune"]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-cand-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |c: &str| {
            assert!(
                Command::new("sh")
                    .args(["-c", c])
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success(),
                "{c}"
            )
        };
        sh("git init -q -b main && git config user.email t@t && git config user.name t");
        sh(
            "printf 'node_modules/\\n.env\\n' > .gitignore && echo v0 > a.txt && git add . && git commit -qm inicio",
        );
        dir.canonicalize().unwrap()
    }

    #[test]
    fn snapshot_has_staged_content_not_unstaged() {
        let dir = repo("snap");
        std::fs::write(dir.join("a.txt"), "v1\n").unwrap();
        git(&dir, &["add", "a.txt"], None).unwrap();
        std::fs::write(dir.join("a.txt"), "v2 sin preparar\n").unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::write(dir.join(".env"), "X=1\n").unwrap();
        let index_before = git(&dir, &["write-tree"], None).unwrap();

        let c = freeze(&dir, "Mi App", Stage::Commit, false, None, "sha256:x").unwrap();
        assert!(c.isolated, "hay cambios sin preparar: debe aislarse");
        assert!(c.id.starts_with("mi-app-"));
        let snap = Snapshot::create(&dir, &c.tree, &c.head).unwrap();
        assert_eq!(
            std::fs::read_to_string(snap.path.join("a.txt")).unwrap(),
            "v1\n"
        );
        assert!(
            snap.path.join("node_modules/pkg").exists(),
            "dependencias enlazadas"
        );
        assert!(snap.path.join(".env").exists());
        let path = snap.path.clone();
        drop(snap);
        assert!(!path.exists(), "la copia se elimina");

        // El usuario no nota nada: su archivo, su index y sus ramas siguen igual.
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "v2 sin preparar\n"
        );
        assert_eq!(git(&dir, &["write-tree"], None).unwrap(), index_before);
        assert_eq!(
            git(&dir, &["worktree", "list"], None)
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn tree_changes_with_content_and_matches_when_clean() {
        let dir = repo("tree");
        std::fs::write(dir.join("a.txt"), "v1\n").unwrap();
        git(&dir, &["add", "a.txt"], None).unwrap();
        let c = freeze(&dir, "x", Stage::Commit, false, None, "").unwrap();
        assert!(
            !c.isolated,
            "árbol de trabajo = candidato: corre en su sitio"
        );
        std::fs::write(dir.join("nuevo.txt"), "x\n").unwrap();
        let wt = worktree_tree(&dir).unwrap();
        assert_ne!(wt, c.tree, "un archivo nuevo cambia el árbol de trabajo");
        assert_eq!(
            candidate_tree(&dir, Stage::Commit, false, None).unwrap(),
            c.tree,
            "pero no el candidato del commit"
        );
    }
}
