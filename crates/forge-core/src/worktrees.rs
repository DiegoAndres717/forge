// Worktrees del repositorio (los de .claude/worktrees, /tmp…): verlos con su estado y
// limpiar los que ya no hacen falta sin perder trabajo. Nunca `--force` ni `branch -D`.
use crate::tr;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    /// `None` con HEAD separado.
    pub branch: Option<String>,
    pub head: Option<String>,
    /// El worktree principal (el primero de la lista): nunca se toca.
    pub main: bool,
    /// Su carpeta ya no existe: solo quedan sus datos en `.git/worktrees`.
    pub prunable: bool,
    pub locked: bool,
    /// ¿Ya está su rama en la principal (merge, squash o rebase)? `None` si no se sabe.
    pub merged: Option<bool>,
    /// Cambios sin commit (también archivos nuevos sin seguimiento).
    pub dirty: bool,
    /// Commits que la rama remota no tiene; `None` sin rama remota.
    pub unpushed: Option<u32>,
    /// Su rama sigue una rama remota que existe (entonces la local no se borra).
    pub tracked: bool,
    /// Archivos ignorados que `git worktree remove` borraría y no se regeneran (`.env`…).
    pub ignored: Vec<String>,
    /// Bytes en disco (aproximado); `None` si no se pudo medir.
    pub size: Option<u64>,
}

/// Qué haría la limpieza con un worktree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// `git worktree prune` (la carpeta ya no está); y borra la rama si está fusionada.
    Prune {
        delete_branch: bool,
    },
    /// `git worktree remove` (sin --force); y borra la rama si está fusionada.
    Remove {
        delete_branch: bool,
    },
    Keep(Keep),
}

/// Por qué un worktree se queda.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Keep {
    Main,
    Current,
    InUse,
    Locked,
    Dirty,
    /// Tiene commits que no están ni en la principal ni en el remoto.
    Unsaved,
    /// Tiene archivos ignorados que no se regeneran (`.env`, bases de datos locales…).
    Ignored,
    /// HEAD separado: puede haber un rebase o un bisect a medias.
    Detached,
    /// Su carpeta está en un disco que no está montado: no se poda.
    Unmounted,
}

impl Keep {
    pub fn reason(self) -> String {
        match self {
            Keep::Main => tr!("es el worktree principal"),
            Keep::Current => tr!("es el worktree actual"),
            Keep::InUse => tr!("una terminal o un proceso trabaja en él"),
            Keep::Locked => tr!("está bloqueado (git worktree lock)"),
            Keep::Dirty => tr!("tiene cambios sin commit"),
            Keep::Unsaved => tr!("su rama no está fusionada ni subida"),
            Keep::Ignored => tr!("tiene archivos ignorados que no se pueden regenerar"),
            Keep::Detached => tr!("tiene HEAD separado (quizá un rebase a medias)"),
            Keep::Unmounted => tr!("su disco no está conectado"),
        }
        .to_string()
    }
}

impl Worktree {
    /// La rama, o «HEAD separado».
    pub fn label(&self) -> String {
        self.branch
            .clone()
            .unwrap_or_else(|| tr!("HEAD separado").to_string())
    }

    /// Su estado en pocas palabras: «fusionada · cambios sin commit · 2 sin subir».
    pub fn state(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut add = |s: &str| parts.push(s.to_string());
        if self.prunable {
            add(tr!("carpeta borrada"));
        }
        if self.locked {
            add(tr!("bloqueado (lock)"));
        }
        match self.merged {
            Some(true) => add(tr!("fusionada")),
            Some(false) => add(tr!("sin fusionar")),
            None => {}
        }
        if self.dirty {
            add(tr!("cambios sin commit"));
        }
        if !self.ignored.is_empty() {
            add(&tr!("ignorados: {p}", p = self.ignored.join(", ")));
        }
        match self.unpushed {
            Some(0) => add(tr!("subida")),
            Some(n) => add(&tr!("{n} sin subir", n = n)),
            None if !self.main && !self.prunable => add(tr!("sin rama remota")),
            None => {}
        }
        parts.join(" · ")
    }
}

/// Un worktree, lo que la limpieza hace (o haría) con él y, si se aplicó, cómo fue.
#[derive(Clone, Debug, PartialEq)]
pub struct Cleanup {
    pub worktree: Worktree,
    pub action: Action,
    pub result: Option<Result<(), String>>,
}

impl Cleanup {
    /// Qué se hace con él: «quitar y borrar la rama feat/x», «se queda: …».
    pub fn describe(&self) -> String {
        let branch = self.worktree.label();
        match self.action {
            Action::Prune {
                delete_branch: true,
            } => {
                tr!("podar y borrar la rama {b}", b = branch)
            }
            Action::Prune { .. } => tr!("podar (la carpeta ya no está)").to_string(),
            Action::Remove {
                delete_branch: true,
            } => {
                tr!("quitar y borrar la rama {b}", b = branch)
            }
            Action::Remove { .. } => tr!("quitar (la rama se queda)").to_string(),
            Action::Keep(keep) => tr!("se queda: {r}", r = keep.reason()),
        }
    }
}

/// Los worktrees del repositorio en `repo`, con su estado y lo que ocupan.
pub fn list(repo: &Path) -> Result<Vec<Worktree>, String> {
    list_with(repo, true)
}

/// Carpetas (o archivos) ignorados que se regeneran solos: perderlos no pierde trabajo.
const REBUILDABLE: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    ".parcel-cache",
    "coverage",
    "__pycache__",
    ".pytest_cache",
    ".venv",
    "venv",
    ".gradle",
    "DerivedData",
    ".DS_Store",
];

/// `sizes`: medir lo que ocupa cada uno (recorre todas sus carpetas: la limpieza no lo usa).
fn list_with(repo: &Path, sizes: bool) -> Result<Vec<Worktree>, String> {
    let out = git(repo, &["worktree", "list", "--porcelain", "-z"])?;
    let base = crate::git::main_branch(repo);
    let mut list = parse(&out);
    for wt in &mut list {
        if wt.main {
            continue; // nunca se toca; medirlo sería medir el repositorio entero
        }
        if let Some(branch) = &wt.branch {
            if let Some(base) = &base {
                wt.merged = crate::git::branch_merged_into(repo, base, branch);
            }
            let local = format!("refs/heads/{branch}");
            let upstream = git(repo, &["for-each-ref", "--format=%(upstream)", &local]);
            wt.tracked = upstream.is_ok_and(|u| {
                !u.trim().is_empty() && crate::git::resolve_commit(repo, u.trim()).is_some()
            });
        }
        if wt.prunable {
            continue;
        }
        match crate::git::status(&wt.path) {
            Ok(status) => {
                wt.dirty = !status.files.is_empty();
                // Con la rama remota borrada (`[gone]`) no queda nada subido que valga.
                wt.unpushed = status
                    .upstream
                    .filter(|u| crate::git::resolve_commit(&wt.path, u).is_some())
                    .map(|_| status.ahead);
            }
            // Si no se puede mirar, se trata como si tuviera cambios.
            Err(_) => wt.dirty = true,
        }
        wt.ignored = match git(&wt.path, &["status", "--porcelain", "--ignored", "-z"]) {
            Ok(out) => unrecoverable(&out),
            Err(e) => vec![e],
        };
        if sizes {
            wt.size = dir_size(&wt.path);
        }
    }
    Ok(list)
}

/// Lo ignorado de `git status --ignored -z` que no se regenera solo.
fn unrecoverable(status: &str) -> Vec<String> {
    status
        .split('\0')
        .filter_map(|e| e.strip_prefix("!! "))
        .filter(|p| {
            !Path::new(p)
                .components()
                .any(|c| REBUILDABLE.iter().any(|r| c.as_os_str() == *r))
        })
        .map(str::to_string)
        .collect()
}

/// ¿Está la carpeta en un disco (/Volumes/x, /mnt/x, /media/u/x, C:\) que no está montado?
fn volume_missing(path: &Path) -> bool {
    let parts: Vec<_> = path.components().collect();
    let depth = if cfg!(windows) {
        2
    } else {
        match parts.get(1).and_then(|c| c.as_os_str().to_str()) {
            Some("Volumes" | "mnt") => 3,
            Some("media") => 4,
            _ => return false,
        }
    };
    parts.len() >= depth && !parts[..depth].iter().collect::<PathBuf>().exists()
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| tr!("no se pudo ejecutar git: {e}", e = e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `git worktree list --porcelain -z`: campos separados por NUL (una ruta puede llevar un
/// salto de línea), un bloque por worktree y el principal primero.
fn parse(out: &str) -> Vec<Worktree> {
    let mut list: Vec<Worktree> = Vec::new();
    for line in out.split('\0') {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            list.push(Worktree {
                path: PathBuf::from(value),
                main: list.is_empty(),
                ..Default::default()
            });
            continue;
        }
        let Some(wt) = list.last_mut() else { continue };
        match key {
            "HEAD" => wt.head = Some(value.to_string()),
            "branch" => wt.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).into()),
            "prunable" => wt.prunable = true,
            "locked" => wt.locked = true,
            _ => {}
        }
    }
    list
}

/// Carpetas de trabajo de los procesos del usuario (lo que haya en marcha en un worktree
/// lo protege). Best-effort: `/proc` en Linux, `lsof` en macOS; en Windows, ninguna.
pub fn process_cwds() -> Vec<PathBuf> {
    if cfg!(target_os = "linux") {
        let Ok(procs) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        procs
            .flatten()
            .filter_map(|p| std::fs::read_link(p.path().join("cwd")).ok())
            .collect()
    } else if cfg!(target_os = "macos") {
        // Sale con error si algún proceso no se deja leer, pero la lista vale igual.
        std::process::Command::new("lsof")
            .args(["-d", "cwd", "-Fn"])
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| l.strip_prefix('n'))
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// Bytes de los archivos bajo `dir`, sin seguir enlaces; lo que no se puede leer no cuenta.
fn dir_size(dir: &Path) -> Option<u64> {
    std::fs::read_dir(dir).ok()?;
    let (mut total, mut pending) = (0, vec![dir.to_path_buf()]);
    while let Some(d) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            // `DirEntry::metadata` no sigue enlaces simbólicos.
            match entry.metadata() {
                Ok(m) if m.is_dir() => pending.push(entry.path()),
                Ok(m) if m.is_file() => total += m.len(),
                _ => {}
            }
        }
    }
    Some(total)
}

/// Qué hacer con `wt`. `current`: carpetas desde las que se pide (nunca se tocan sus
/// worktrees); `in_use`: carpetas de terminales y procesos abiertos.
pub fn decide(wt: &Worktree, current: &[PathBuf], in_use: &[PathBuf]) -> Action {
    let root = crate::canonical(&wt.path);
    // El principal contiene a los anidados (.claude/worktrees): ya se descartó arriba.
    let inside = |dirs: &[PathBuf]| dirs.iter().any(|d| crate::canonical(d).starts_with(&root));
    let merged = wt.branch.is_some() && wt.merged == Some(true);
    // Como `branch -d`, y además nunca una que aún sigue a una rama remota viva.
    let delete_branch = merged && !wt.tracked;
    match () {
        _ if wt.main => Action::Keep(Keep::Main),
        _ if inside(current) => Action::Keep(Keep::Current),
        // Git no poda ni quita los bloqueados sin --force.
        _ if wt.locked => Action::Keep(Keep::Locked),
        // Un disco desconectado también deja la carpeta «borrada».
        _ if wt.prunable && volume_missing(&wt.path) => Action::Keep(Keep::Unmounted),
        _ if wt.prunable => Action::Prune { delete_branch },
        _ if inside(in_use) => Action::Keep(Keep::InUse),
        _ if wt.dirty => Action::Keep(Keep::Dirty),
        _ if wt.branch.is_none() => Action::Keep(Keep::Detached),
        _ if !wt.ignored.is_empty() => Action::Keep(Keep::Ignored),
        _ if merged || wt.unpushed == Some(0) => Action::Remove { delete_branch },
        _ => Action::Keep(Keep::Unsaved),
    }
}

/// La limpieza: sin `apply` solo dice lo que haría (simulación).
pub fn clean(
    repo: &Path,
    current: &[PathBuf],
    in_use: &[PathBuf],
    apply: bool,
) -> Result<Vec<Cleanup>, String> {
    let mut plan = plan(repo, current, in_use)?;
    if apply {
        self::apply(repo, &mut plan);
    }
    Ok(plan)
}

/// Lo que haría la limpieza con cada worktree, sin tocar nada.
pub fn plan(repo: &Path, current: &[PathBuf], in_use: &[PathBuf]) -> Result<Vec<Cleanup>, String> {
    Ok(list_with(repo, false)?
        .into_iter()
        .map(|worktree| Cleanup {
            action: decide(&worktree, current, in_use),
            worktree,
            result: None,
        })
        .collect())
}

/// Ejecuta `plan` (de `plan`) y deja en cada uno cómo fue.
pub fn apply(repo: &Path, plan: &mut [Cleanup]) {
    // `prune` solo quita lo que git ve como prunable (nunca los bloqueados).
    let pruned = plan
        .iter()
        .any(|c| matches!(c.action, Action::Prune { .. }))
        .then(|| git(repo, &["worktree", "prune"]).map(drop));
    for c in plan.iter_mut() {
        let (removed, delete_branch) = match c.action {
            Action::Prune { delete_branch } => (pruned.clone().unwrap_or(Ok(())), delete_branch),
            Action::Remove { delete_branch } => {
                // Sin --force: git se niega si aparece algún cambio desde que se miró.
                let path = c.worktree.path.to_string_lossy();
                (
                    git(repo, &["worktree", "remove", &path]).map(drop),
                    delete_branch,
                )
            }
            Action::Keep(_) => continue,
        };
        if removed.is_ok() {
            forget(&c.worktree.path);
        }
        c.result = Some(removed.and_then(|()| {
            match (&c.worktree.branch, &c.worktree.head) {
                // Como `branch -d`, pero vale con squash (ya comprobado): solo si la rama
                // sigue en el commit que se miró.
                (Some(branch), Some(head)) if delete_branch => git(
                    repo,
                    &["update-ref", "-d", &format!("refs/heads/{branch}"), head],
                )
                .map(drop),
                _ => Ok(()),
            }
        }));
    }
}

// ------------------------------------------------------------------ worktrees de Forge
//
// Varios agentes en paralelo creaban worktrees y clones sueltos (en /tmp, .claude/worktrees,
// carpetas hermanas) que quedaban como basura. `forge worktree new` los pone todos en un
// mismo sitio, `<datos de Forge>/worktrees/<proyecto>/<rama>`, y apunta quién lo pidió en
// `<rama>.json` al lado (fuera del worktree: dentro lo dejaría con cambios). Guard frena
// los `git worktree add` y `git clone` a mano del propio proyecto.

/// Con `FORGE_ALLOW_WORKTREE=1` un agente puede crear a mano un worktree o un clon del
/// proyecto (casos legítimos: probar un clon limpio, un bisect…).
pub const ALLOW: &str = "FORGE_ALLOW_WORKTREE";

/// Un worktree creado con `forge worktree new` y quién lo pidió (lo que hubiera en el
/// entorno; vacío si no se sabe).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    pub path: PathBuf,
    pub branch: String,
    /// Raíz del worktree principal del repositorio.
    pub project: PathBuf,
    /// El agente (`FORGE_ORIGIN`).
    #[serde(default)]
    pub origin: Option<String>,
    /// La terminal (`FORGE_SESSION_ID`: «pid de la app:terminal»).
    #[serde(default)]
    pub session: Option<String>,
    /// La fase del plan cuya rama es esta («plan #194, fase 2»).
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub created_at: i64,
}

impl Record {
    /// «Claude Code · plan #194, fase 2», o vacío si no se sabe quién lo pidió.
    pub fn requester(&self) -> String {
        [&self.origin, &self.phase]
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// `<datos de Forge>/worktrees`, junto a la base de datos (`FORGE_DB` la mueve).
pub fn root() -> Option<PathBuf> {
    Some(
        crate::store::Store::default_path()?
            .parent()?
            .join("worktrees"),
    )
}

/// `feat/x` → `feat-x`: un nombre de carpeta válido en cualquier sistema.
fn folder_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Raíz del worktree principal (la carpeta del proyecto, aunque se pida desde otro worktree).
pub fn main_root(repo: &Path) -> Result<PathBuf, String> {
    let out = git(repo, &["worktree", "list", "--porcelain", "-z"])?;
    parse(&out)
        .into_iter()
        .next()
        .map(|w| w.path)
        .ok_or_else(|| tr!("no es un repositorio Git").to_string())
}

/// Carpeta de los worktrees de `project` (raíz del principal) dentro de `root`.
fn project_dir(project: &Path, root: &Path) -> PathBuf {
    let name = project.file_name().unwrap_or_default().to_string_lossy();
    root.join(folder_name(&name))
}

/// El registro de un worktree, al lado de su carpeta (`feat-x` → `feat-x.json`).
fn record_file(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy();
    Some(path.with_file_name(format!("{name}.json")))
}

fn read_record(file: &Path) -> Option<Record> {
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

/// Crea el worktree de `branch` en `root/<proyecto>/<rama>` (y la rama, desde `from` o la
/// principal, si no existe) y lo registra. Devuelve su carpeta.
pub fn create(
    repo: &Path,
    branch: &str,
    from: Option<&str>,
    root: &Path,
    who: Record,
) -> Result<PathBuf, String> {
    if branch.starts_with('-') || git(repo, &["check-ref-format", "--branch", branch]).is_err() {
        return Err(tr!("nombre de rama no válido: {b}", b = branch).to_string());
    }
    let project = main_root(repo)?;
    let path = project_dir(&project, root).join(folder_name(branch));
    if path.exists() {
        // Un agente que lo vuelve a pedir: el mismo, sin error.
        let same = record_file(&path)
            .and_then(|f| read_record(&f))
            .is_some_and(|r| r.branch == branch);
        return if same {
            Ok(path)
        } else {
            Err(tr!("ya existe {p}", p = path.display()).to_string())
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let target = path.to_string_lossy().into_owned();
    let exists = crate::git::resolve_commit(&project, branch).is_some();
    let args: Vec<String> = match from {
        // La rama ya existe (local o en origin): git la saca tal cual.
        None if exists => vec!["worktree".into(), "add".into(), target, branch.into()],
        _ => {
            let base = match from {
                Some(f) => f.to_string(),
                None => crate::git::main_branch(&project)
                    .ok_or_else(|| tr!("no se encontró la rama principal").to_string())?,
            };
            // Sin seguir a la base (origin/main): la limpieza la tomaría por ya subida.
            [
                "worktree",
                "add",
                "--no-track",
                "-b",
                branch,
                &target,
                &base,
            ]
            .map(String::from)
            .to_vec()
        }
    };
    let out = std::process::Command::new("git")
        .args(&args)
        .current_dir(&project)
        // Los enlaces de Guard dejan pasar este: lo crea Forge, en su sitio.
        .env(ALLOW, "1")
        .output()
        .map_err(|e| tr!("no se pudo ejecutar git: {e}", e = e).to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let record = Record {
        path: path.clone(),
        branch: branch.to_string(),
        project,
        created_at: crate::store::now(),
        ..who
    };
    if let (Some(file), Ok(json)) = (record_file(&path), serde_json::to_string_pretty(&record)) {
        std::fs::write(file, json).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

/// Los worktrees de `repo` creados con `forge worktree new`.
pub fn records(repo: &Path, root: &Path) -> Vec<Record> {
    let Ok(project) = main_root(repo) else {
        return Vec::new();
    };
    let project = crate::canonical(&project);
    let Ok(entries) = std::fs::read_dir(project_dir(&project, root)) else {
        return Vec::new();
    };
    let mut list: Vec<Record> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| read_record(&p))
        .filter(|r| crate::canonical(&r.project) == project)
        .collect();
    list.sort_by(|a, b| a.path.cmp(&b.path));
    list
}

/// Borra el registro de un worktree ya quitado: solo si de verdad es el de Forge para esa
/// carpeta (al lado de un worktree cualquiera puede haber un `.json` del usuario).
fn forget(path: &Path) {
    if let Some(file) = record_file(path)
        && read_record(&file).is_some_and(|r| same_path(&r.path, path))
    {
        let _ = std::fs::remove_file(file);
    }
}

/// ¿La misma carpeta? También si ya no existe (se compara la carpeta que la contiene):
/// `/var` y `/private/var` en macOS, `C:\` y `\\?\C:\` en Windows.
fn same_path(a: &Path, b: &Path) -> bool {
    let key = |p: &Path| {
        p.parent()
            .map(crate::canonical)
            .unwrap_or_default()
            .join(p.file_name().unwrap_or_default())
    };
    key(a) == key(b)
}

/// El registro de `wt`, si lo tiene.
pub fn record_of<'a>(wt: &Worktree, records: &'a [Record]) -> Option<&'a Record> {
    records.iter().find(|r| same_path(&r.path, &wt.path))
}

/// Lo que Forge propone limpiar: worktrees registrados que la limpieza quitaría y cuya rama
/// ya está fusionada o cuya terminal (`open`: las sesiones abiertas) ya se cerró.
pub fn offers(plan: &[Cleanup], records: &[Record], open: &[String]) -> Vec<Cleanup> {
    plan.iter()
        .filter(|c| !matches!(c.action, Action::Keep(_)))
        .filter(|c| {
            record_of(&c.worktree, records).is_some_and(|r| {
                c.worktree.merged == Some(true)
                    || r.session.as_ref().is_some_and(|s| !open.contains(s))
            })
        })
        .cloned()
        .collect()
}

/// La carpeta `.git` común del repositorio en `dir` (la misma desde cualquier worktree).
fn common_dir(dir: &Path) -> Option<PathBuf> {
    let out = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    Some(crate::canonical(Path::new(out.trim())))
}

/// ¿`dir` es el mismo repositorio que `project`? La misma carpeta `.git` o el mismo remoto.
fn same_repo(dir: &Path, project: &Path) -> bool {
    if let (Some(a), Some(b)) = (common_dir(dir), common_dir(project))
        && a == b
    {
        return true;
    }
    matches!((crate::git::remote(dir), crate::git::remote(project)), (Some(a), Some(b)) if a == b)
}

/// Opciones de `git clone` que toman su valor en el argumento siguiente.
const CLONE_VALUES: &[&str] = &[
    "-b",
    "--branch",
    "-o",
    "--origin",
    "-u",
    "--upload-pack",
    "-c",
    "--config",
    "-j",
    "--jobs",
    "--reference",
    "--reference-if-able",
    "--separate-git-dir",
    "--depth",
    "--shallow-since",
    "--shallow-exclude",
    "--template",
    "--filter",
    "--bundle-uri",
    "--server-option",
    "--ref-format",
];

/// El origen de `git clone <opciones> <origen> [carpeta]`.
fn clone_source(rest: &[String]) -> Option<&str> {
    let mut i = 0;
    while let Some(a) = rest.get(i) {
        if a == "--" {
            return rest.get(i + 1).map(String::as_str);
        }
        if !a.starts_with('-') {
            return Some(a);
        }
        i += if CLONE_VALUES.contains(&a.as_str()) {
            2
        } else {
            1
        };
    }
    None
}

/// ¿Clonar `source` (desde `cwd`) copia el repositorio de `project`?
fn same_source(source: &str, cwd: &Path, project: &Path) -> bool {
    let local = cwd.join(source.strip_prefix("file://").unwrap_or(source));
    if local.exists() {
        return same_repo(&local, project);
    }
    crate::git::remote(project).is_some_and(|r| crate::git::normalize_remote(source) == r)
}

/// Si `git <args>` (desde `cwd`) crea un worktree o un clon del mismo repositorio que
/// `project`, el aviso para el agente (en inglés: es para él).
pub fn stray_copy(args: &[String], cwd: &Path, project: &Path) -> Option<String> {
    // Opciones globales antes del subcomando (`-C dir` cambia de carpeta, como en git).
    let mut dir = cwd.to_path_buf();
    let mut i = 0;
    while let Some(a) = args.get(i).filter(|a| a.starts_with('-')) {
        match a.as_str() {
            "-C" => {
                dir = dir.join(args.get(i + 1)?);
                i += 2;
            }
            "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env" => i += 2,
            _ => i += 1,
        }
    }
    let (sub, rest) = (args.get(i)?.as_str(), &args[i + 1..]);
    let what = match sub {
        "worktree"
            if rest.iter().find(|a| !a.starts_with('-'))? == "add" && same_repo(&dir, project) =>
        {
            "a worktree"
        }
        "clone" if same_source(clone_source(rest)?, &dir, project) => "a clone",
        _ => return None,
    };
    Some(format!(
        "Forge: don't create {what} of this project by hand: copies made in /tmp, \
         .claude/worktrees or sibling folders are left behind as clutter. Run \
         `forge worktree new <branch> [--from <base>]` instead: it creates the worktree in \
         Forge's worktrees folder, registers it and prints its path (cd there). When you \
         finish, remove it with `git worktree remove <path>` or let Forge offer the cleanup. \
         Only if a manual copy is truly needed, rerun the command with {ALLOW}=1 in front."
    ))
}

/// Como `stray_copy`, en una línea de shell (para los hooks previos de los agentes).
// ponytail: palabras sueltas, sin un parser de shell (como `danger::dangerous_in_command`);
// el `git` puede ir tras `bash -lc '…'`, `sudo` o `env`. Los enlaces de Guard son el respaldo.
pub fn stray_copy_in_command(command: &str, cwd: &Path, project: &Path) -> Option<String> {
    if command.contains(&format!("{ALLOW}=1")) {
        return None;
    }
    let mut cwd = cwd.to_path_buf();
    command
        .replace("&&", ";")
        .replace("||", ";")
        .split([';', '|', '\n'])
        .find_map(|segment| {
            let words: Vec<String> = segment
                .split_whitespace()
                .map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_string())
                .collect();
            if words.first().is_some_and(|w| w == "cd") {
                if let Some(dir) = words.get(1) {
                    cwd = cwd.join(dir);
                }
                return None;
            }
            let at = words
                .iter()
                .position(|w| crate::danger::shim_name(w).as_deref() == Some("git"))?;
            stray_copy(&words[at + 1..], &cwd, project)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_repo;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    /// Plan #194, fase 2: el worktree va a `<raíz>/<proyecto>/<rama saneada>`, con su rama
    /// nueva desde la principal, y queda registrado con quién lo pidió.
    #[test]
    fn a_new_worktree_goes_to_forges_folder_and_is_registered() {
        let dir = test_repo("wt-new", "true");
        let root = dir.join("forge-data/worktrees");
        let who = Record {
            origin: Some("Codex".into()),
            session: Some("1:7".into()),
            ..Default::default()
        };
        let path = create(&dir, "feat/new-thing", None, &root, who).unwrap();
        let name = dir.file_name().unwrap();
        assert_eq!(path, root.join(name).join("feat-new-thing"));
        assert!(path.join("a").is_file(), "es un checkout del repositorio");
        let list = list(&dir).unwrap();
        let wt = list.iter().find(|w| !w.main).unwrap();
        assert_eq!(wt.branch.as_deref(), Some("feat/new-thing"));
        assert_eq!(crate::canonical(&wt.path), crate::canonical(&path));
        // Sin seguir a la principal: si no, la limpieza la tomaría por subida.
        assert!(wt.unpushed.is_none(), "{wt:?}");
        let records = records(&dir, &root);
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].branch, "feat/new-thing");
        assert_eq!(records[0].origin.as_deref(), Some("Codex"));
        assert_eq!(records[0].requester(), "Codex");
        // Otra vez la misma rama: la misma carpeta, sin error (un agente que repite).
        let again = create(&dir, "feat/new-thing", None, &root, Record::default()).unwrap();
        assert_eq!(again, path);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_new_worktree_can_start_from_another_base_or_reuse_a_branch() {
        let dir = test_repo(
            "wt-from",
            "git switch -qc base && echo b > b && git add b && git commit -qm b \
             && git switch -q main && git branch ya-existe",
        );
        let root = dir.join("forge-data/worktrees");
        let from = create(&dir, "feat/x", Some("base"), &root, Record::default()).unwrap();
        assert!(from.join("b").is_file(), "sale de --from");
        let reused = create(&dir, "ya-existe", None, &root, Record::default()).unwrap();
        assert!(reused.join("a").is_file() && !reused.join("b").exists());
        assert!(create(&dir, "-mal", None, &root, Record::default()).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn worktree_add_and_clones_of_the_same_repo_are_stray_copies() {
        let dir = test_repo(
            "wt-stray",
            "git remote add origin git@github.com:Acme/Repo.git && mkdir sub",
        );
        let other = test_repo("wt-stray-other", "true");
        let tmp = std::env::temp_dir();
        let stray = |cmd: &str, cwd: &Path| stray_copy(&words(cmd), cwd, &dir);
        let msg = stray("worktree add ../x", &dir).expect("worktree add");
        assert!(msg.contains("forge worktree new"), "{msg}");
        assert!(msg.contains(ALLOW), "{msg}");
        assert!(stray("worktree add -b feat/y ../y", &dir.join("sub")).is_some());
        assert!(stray(&format!("-C {} worktree add x", dir.display()), &tmp).is_some());
        // Clones del mismo repositorio: por carpeta o por el mismo remoto en otra forma.
        assert!(stray(&format!("clone {} /tmp/c", dir.display()), &tmp).is_some());
        assert!(stray("clone --depth 1 . ../c", &dir).is_some());
        assert!(stray("clone https://github.com/acme/repo /tmp/c", &tmp).is_some());
        // Lo demás pasa: listar, quitar, podar, clonar otro repo, un worktree de otro repo.
        for ok in [
            "worktree list",
            "worktree remove ../x",
            "worktree prune",
            "status",
        ] {
            assert_eq!(stray(ok, &dir), None, "{ok}");
        }
        assert_eq!(stray(&format!("clone {} c", other.display()), &tmp), None);
        assert_eq!(stray("clone https://github.com/acme/otro c", &tmp), None);
        assert_eq!(stray("worktree add ../x", &other), None);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(other).unwrap();
    }

    #[test]
    fn shell_lines_are_checked_with_their_cd_and_the_escape() {
        let dir = test_repo("wt-line", "true");
        let tmp = std::env::temp_dir();
        let line = |cmd: &str, cwd: &Path| stray_copy_in_command(cmd, cwd, &dir);
        assert!(line("git status && git worktree add ../x", &dir).is_some());
        assert!(line(&format!("cd {} && git worktree add x", dir.display()), &tmp).is_some());
        assert!(line("bash -lc 'git worktree add ../x'", &dir).is_some());
        assert_eq!(line("git worktree list", &dir), None);
        assert_eq!(line("cd /tmp && git worktree add x", &dir), None);
        assert_eq!(
            line(&format!("{ALLOW}=1 git worktree add ../x"), &dir),
            None
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forge_offers_to_clean_registered_worktrees_once_merged_or_orphaned() {
        let wt = |path: &str, merged| Worktree {
            path: PathBuf::from(path),
            branch: Some(path.into()),
            merged: Some(merged),
            ..Default::default()
        };
        let cleanup = |w: Worktree, action| Cleanup {
            worktree: w,
            action,
            result: None,
        };
        let remove = Action::Remove {
            delete_branch: false,
        };
        let plan = [
            cleanup(wt("/w/merged", true), remove),
            cleanup(wt("/w/closed", false), remove),
            cleanup(wt("/w/open", false), remove),
            cleanup(wt("/w/dirty", true), Action::Keep(Keep::Dirty)),
            cleanup(wt("/w/unregistered", true), remove),
        ];
        let record = |path: &str, session: &str| Record {
            path: PathBuf::from(path),
            session: Some(session.into()),
            ..Default::default()
        };
        let records = [
            record("/w/merged", "1:1"),
            record("/w/closed", "1:2"),
            record("/w/open", "1:3"),
            record("/w/dirty", "1:4"),
        ];
        let open = ["1:1".to_string(), "1:3".to_string()];
        let offered: Vec<_> = offers(&plan, &records, &open)
            .into_iter()
            .map(|c| c.worktree.path)
            .collect();
        assert_eq!(
            offered,
            [PathBuf::from("/w/merged"), PathBuf::from("/w/closed")]
        );
    }

    #[test]
    fn cleaning_a_registered_worktree_forgets_its_record() {
        let dir = test_repo("wt-forget", "true");
        let root = dir.join("forge-data/worktrees");
        let path = create(&dir, "feat/gone", None, &root, Record::default()).unwrap();
        std::fs::remove_dir_all(&path).unwrap();
        let plan = clean(&dir, &[], &[], true).unwrap();
        assert!(
            plan.iter()
                .any(|c| matches!(c.action, Action::Prune { .. }))
        );
        assert!(records(&dir, &root).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
    use std::process::Command;

    fn find<'a>(list: &'a [Worktree], tail: &str) -> &'a Worktree {
        list.iter()
            .find(|w| w.path.ends_with(tail))
            .unwrap_or_else(|| panic!("falta {tail} en {list:?}"))
    }

    fn action(plan: &[Cleanup], tail: &str) -> Action {
        plan.iter()
            .find(|c| c.worktree.path.ends_with(tail))
            .unwrap_or_else(|| panic!("falta {tail} en {plan:?}"))
            .action
    }

    fn porcelain(dir: &Path) -> String {
        let out = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Rama `feat/done` fusionada con squash (como GitHub) y su worktree en `wt/done`.
    const SQUASHED: &str = "echo wt/ >> .git/info/exclude \
        && git switch -qc feat/done && echo c > c && git add c && git commit -qm c \
        && git switch -q main && git merge -q --squash feat/done && git commit -qm 'c (#1)' \
        && git worktree add -q wt/done feat/done";

    #[test]
    fn a_worktree_with_uncommitted_changes_is_never_touched() {
        let dir = test_repo(
            "wt-dirty",
            &format!("{SQUASHED} && echo nuevo > wt/done/nuevo.txt"),
        );
        let list = list(&dir).unwrap();
        let done = find(&list, "wt/done");
        assert!(done.dirty, "{done:?}");
        assert_eq!(decide(done, &[], &[]), Action::Keep(Keep::Dirty));
        let plan = clean(&dir, &[], &[], true).unwrap();
        assert_eq!(action(&plan, "wt/done"), Action::Keep(Keep::Dirty));
        assert!(dir.join("wt/done/nuevo.txt").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_worktree_whose_folder_is_gone_is_pruned_and_its_unmerged_branch_kept() {
        let dir = test_repo(
            "wt-prune",
            "echo wt/ >> .git/info/exclude && git worktree add -q -b feat/gone wt/gone \
             && echo g > wt/gone/g && git -C wt/gone add g && git -C wt/gone commit -qm g \
             && rm -rf wt/gone",
        );
        let gone = find(&list(&dir).unwrap(), "wt/gone").clone();
        assert!(gone.prunable, "{gone:?}");
        assert_eq!(
            decide(&gone, &[], &[]),
            Action::Prune {
                delete_branch: false
            }
        );
        let plan = clean(&dir, &[], &[], true).unwrap();
        assert_eq!(action(&plan, "wt/gone"), decide(&gone, &[], &[]));
        assert!(!porcelain(&dir).contains("wt/gone"), "{}", porcelain(&dir));
        assert!(crate::git::resolve_commit(&dir, "feat/gone").is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_squash_merged_worktree_is_removed_with_its_branch() {
        let dir = test_repo("wt-squash", SQUASHED);
        let list = list(&dir).unwrap();
        let done = find(&list, "wt/done");
        assert_eq!(done.merged, Some(true), "{done:?}");
        assert!(!done.dirty && done.size.is_some_and(|s| s > 0), "{done:?}");
        let plan = clean(&dir, &[], &[], true).unwrap();
        assert_eq!(
            action(&plan, "wt/done"),
            Action::Remove {
                delete_branch: true
            }
        );
        let done = plan.iter().find(|c| c.worktree.path.ends_with("wt/done"));
        assert_eq!(done.unwrap().result, Some(Ok(())));
        assert!(!dir.join("wt/done").exists());
        assert_eq!(crate::git::resolve_commit(&dir, "feat/done"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_main_the_current_and_worktrees_in_use_are_never_removed() {
        let dir = test_repo(
            "wt-current",
            &format!(
                "{SQUASHED} && mkdir wt/done/sub && git switch -qc feat/other && echo o > o \
                 && git add o && git commit -qm o && git switch -q main \
                 && git merge -q --squash feat/other && git commit -qm 'o (#2)' \
                 && git worktree add -q wt/other feat/other"
            ),
        );
        let current = [dir.join("wt/done/sub")];
        let in_use = [dir.join("wt/other")];
        let plan = clean(&dir, &current, &in_use, true).unwrap();
        assert!(
            plan.iter()
                .any(|c| c.worktree.main && c.action == Action::Keep(Keep::Main)),
            "{plan:?}"
        );
        assert_eq!(action(&plan, "wt/done"), Action::Keep(Keep::Current));
        assert_eq!(action(&plan, "wt/other"), Action::Keep(Keep::InUse));
        assert!(dir.join("wt/done").is_dir() && dir.join("wt/other").is_dir());
        assert!(dir.join(".git").is_dir());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_simulation_changes_nothing() {
        let dir = test_repo(
            "wt-dry",
            &format!("{SQUASHED} && git worktree add -q -b feat/gone wt/gone && rm -rf wt/gone"),
        );
        let before = porcelain(&dir);
        let plan = clean(&dir, &[], &[], false).unwrap();
        assert!(matches!(action(&plan, "wt/gone"), Action::Prune { .. }));
        assert!(matches!(action(&plan, "wt/done"), Action::Remove { .. }));
        assert!(plan.iter().all(|c| c.result.is_none()));
        assert_eq!(porcelain(&dir), before);
        assert!(dir.join("wt/done/c").is_file());
        assert!(crate::git::resolve_commit(&dir, "feat/done").is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `git worktree remove` sin --force borra lo ignorado: un `.env` no se recupera, un
    /// `target/` sí.
    #[test]
    fn ignored_files_that_cannot_be_rebuilt_keep_the_worktree() {
        let dir = test_repo(
            "wt-ignored",
            &format!(
                "{SQUASHED} && printf '.env\\ntarget/\\n' >> .git/info/exclude \
                 && echo SECRET=1 > wt/done/.env && git switch -qc feat/other \
                 && echo o > o && git add o && git commit -qm o && git switch -q main \
                 && git merge -q --squash feat/other && git commit -qm 'o (#2)' \
                 && git worktree add -q wt/other feat/other \
                 && mkdir wt/other/target && echo bin > wt/other/target/app"
            ),
        );
        let plan = clean(&dir, &[], &[], true).unwrap();
        assert_eq!(action(&plan, "wt/done"), Action::Keep(Keep::Ignored));
        assert!(dir.join("wt/done/.env").is_file());
        assert!(matches!(action(&plan, "wt/other"), Action::Remove { .. }));
        assert!(!dir.join("wt/other").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Con HEAD separado puede haber un rebase o un bisect a medias: no se quita.
    #[test]
    fn detached_worktrees_are_kept_even_if_a_remote_has_their_commit() {
        let dir = test_repo(
            "wt-detached",
            "echo wt/ >> .git/info/exclude && git update-ref refs/remotes/origin/main main \
             && git worktree add -q --detach wt/det main",
        );
        let plan = clean(&dir, &[], &[], true).unwrap();
        assert_eq!(action(&plan, "wt/det"), Action::Keep(Keep::Detached));
        assert!(dir.join("wt/det").is_dir());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Fusionada pero con su rama remota viva (una `develop`, o GitHub sin borrar ramas):
    /// el worktree se quita, la rama local se queda.
    #[test]
    fn a_merged_branch_with_a_live_remote_branch_is_not_deleted() {
        let dir = test_repo(
            "wt-upstream",
            &format!(
                "{SQUASHED} && git remote add origin /no/existe \
                 && git update-ref refs/remotes/origin/feat/done feat/done \
                 && git branch -q --set-upstream-to=origin/feat/done feat/done"
            ),
        );
        let plan = clean(&dir, &[], &[], true).unwrap();
        assert_eq!(
            action(&plan, "wt/done"),
            Action::Remove {
                delete_branch: false
            }
        );
        assert!(!dir.join("wt/done").exists());
        assert!(crate::git::resolve_commit(&dir, "refs/heads/feat/done").is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn a_missing_external_disk_is_told_apart_from_a_deleted_folder() {
        assert!(volume_missing(Path::new("/Volumes/forge-no-existe/wt")));
        assert!(volume_missing(Path::new("/media/ana/forge-no-existe/wt")));
        assert!(!volume_missing(Path::new("/tmp/forge-no-existe/wt")));
    }

    #[test]
    fn porcelain_with_nul_separators_keeps_odd_paths_whole() {
        let out = "worktree /r\0HEAD abc\0branch refs/heads/main\0\0\
                   worktree /t/con\nsalto\0HEAD def\0detached\0locked razón\0\0";
        let list = parse(out);
        assert_eq!(list.len(), 2);
        assert!(list[0].main && list[0].branch.as_deref() == Some("main"));
        assert_eq!(list[1].path, PathBuf::from("/t/con\nsalto"));
        assert!(list[1].locked && list[1].branch.is_none() && !list[1].main);
    }
}
