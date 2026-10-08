// Comandos peligrosos (§15.2): `rm -rf`, `git reset --hard`, `git push --force`,
// `git clean -f`, `docker system prune`, `DROP DATABASE`.
//
// En las terminales de Forge, `rm`, `git`, `docker`, `psql`… son enlaces al binario de
// Forge que van primero en el PATH (lo añade un ZDOTDIR propio al final del arranque de
// zsh, después de la configuración del usuario). Si el comando es inofensivo se ejecuta el
// programa real al instante; si es peligroso, se pide autorización a la app (un archivo en
// `approvals/` que la ventana muestra como diálogo) y se espera la respuesta. Así también
// se protege lo que ejecutan los agentes, no solo lo que escribe el usuario.
use crate::tr;
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Programas interceptados.
pub const SHIMMED: [&str; 7] = ["rm", "git", "docker", "psql", "mysql", "mariadb", "dropdb"];

/// Lo que espera un comando a que el usuario responda (menos que el límite de 2 min de
/// las herramientas de shell de los agentes, para que vean el rechazo y no un tiempo agotado).
const WAIT: Duration = Duration::from_secs(110);

/// Si el comando necesita autorización, el motivo.
pub fn classify(
    program: &str,
    args: &[String],
    cwd: &Path,
    project: Option<&Path>,
) -> Option<String> {
    match program {
        "rm" => rm(args, cwd, project),
        "git" => git(args),
        "docker" => {
            let pos: Vec<&str> = args
                .iter()
                .map(String::as_str)
                .filter(|a| !a.starts_with('-'))
                .collect();
            pos.windows(2)
                .any(|w| w == ["system", "prune"])
                .then(|| tr!("borra contenedores, imágenes y redes de Docker sin uso").into())
        }
        "dropdb" => Some(tr!("borra una base de datos").into()),
        "psql" | "mysql" | "mariadb" => args
            .iter()
            .any(|a| {
                let a = a.to_lowercase();
                let words: Vec<&str> = a.split_whitespace().collect();
                words.windows(2).any(|w| w == ["drop", "database"])
            })
            .then(|| "ejecuta DROP DATABASE".into()),
        _ => None,
    }
}

/// Opciones cortas agrupadas (`-rf`) y largas (`--force`).
fn has_flag(args: &[String], short: char, long: &str) -> bool {
    args.iter()
        .take_while(|a| *a != "--")
        .any(|a| a == long || (a.starts_with('-') && !a.starts_with("--") && a.contains(short)))
}

fn rm(args: &[String], cwd: &Path, project: Option<&Path>) -> Option<String> {
    let recursive = has_flag(args, 'r', "--recursive") || has_flag(args, 'R', "--recursive");
    if !(recursive && has_flag(args, 'f', "--force")) {
        return None;
    }
    let mut after_dashes = false;
    let targets: Vec<&String> = args
        .iter()
        .filter(|a| {
            if *a == "--" {
                after_dashes = true;
                return false;
            }
            after_dashes || !a.starts_with('-')
        })
        .collect();
    // Dentro del proyecto (dist, node_modules, .next…) es rutina: no se pregunta.
    let inside = |t: &String| {
        let path = normalize(&cwd.join(t));
        project.is_some_and(|root| {
            path.starts_with(root)
                && path != root
                && !path.components().any(|c| c.as_os_str() == ".git")
        })
    };
    let outside: Vec<&String> = targets.into_iter().filter(|t| !inside(t)).collect();
    if outside.is_empty() {
        return None;
    }
    let names: Vec<&str> = outside.iter().take(3).map(|s| s.as_str()).collect();
    Some(tr!(
        "borra en forma recursiva fuera de la carpeta del proyecto: {p0}",
        p0 = names.join(", ")
    ))
}

/// `a/./b/../c` → `a/c` sin tocar el disco (el destino puede no existir).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

fn git(args: &[String]) -> Option<String> {
    // Subcomando: primer argumento que no es opción global (`-C dir`, `-c k=v`…).
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        i += if matches!(args[i].as_str(), "-C" | "-c") {
            2
        } else {
            1
        };
    }
    let (sub, rest) = (args.get(i)?.as_str(), &args[(i + 1).min(args.len())..]);
    match sub {
        "reset" if rest.iter().any(|a| a == "--hard") => {
            Some(tr!("descarta todos los cambios sin commit (git reset --hard)").into())
        }
        "push"
            if has_flag(rest, 'f', "--force")
                || rest
                    .iter()
                    .any(|a| a.starts_with("--force") || a.starts_with('+')) =>
        {
            Some(tr!("reescribe la historia del remoto (git push --force)").into())
        }
        "clean" if has_flag(rest, 'f', "--force") && !has_flag(rest, 'n', "--dry-run") => {
            Some(tr!("borra los archivos sin seguimiento (git clean)").into())
        }
        _ => None,
    }
}

// ------------------------------------------------------------------ autorizaciones

/// Solicitud pendiente (un JSON en `approvals/`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Request {
    pub id: String,
    pub command: String,
    pub reason: String,
    pub cwd: PathBuf,
    pub project: Option<PathBuf>,
    /// Agente o panel que lo ejecuta (p. ej. "Claude Code"), si se sabe.
    pub origin: Option<String>,
    pub pid: u32,
}

fn alive(pid: u32) -> bool {
    // SAFETY: kill con señal 0 solo comprueba que el proceso existe.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Solicitudes cuyo proceso sigue esperando.
pub fn pending(dir: &Path) -> Vec<Request> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Request> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .filter(|r: &Request| {
            let answered = ["allow", "deny"]
                .iter()
                .any(|x| dir.join(format!("{}.{x}", r.id)).exists());
            !answered && alive(r.pid)
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn answer(dir: &Path, id: &str, allow: bool) -> std::io::Result<()> {
    let ext = if allow { "allow" } else { "deny" };
    std::fs::write(dir.join(format!("{id}.{ext}")), b"")
}

/// Pide autorización y espera la respuesta. `Ok(true)` = permitido.
fn ask(dir: &Path, request: &Request, app_pid: Option<u32>) -> Result<bool, String> {
    if !app_pid.is_some_and(alive) {
        return ask_tty(request);
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{}.json", request.id));
    let tmp = dir.join(format!("{}.tmp", request.id));
    std::fs::write(
        &tmp,
        serde_json::to_vec(request).map_err(|e| e.to_string())?,
    )
    .and_then(|_| std::fs::rename(&tmp, &file))
    .map_err(|e| e.to_string())?;
    eprintln!(
        "{}",
        tr!(
            "Forge: «{p0}» {p1} — esperando autorización en Forge…",
            p0 = request.command,
            p1 = request.reason
        )
    );
    let start = Instant::now();
    let decision = loop {
        if dir.join(format!("{}.allow", request.id)).exists() {
            break Ok(true);
        }
        if dir.join(format!("{}.deny", request.id)).exists() {
            break Ok(false);
        }
        if start.elapsed() > WAIT || !app_pid.is_some_and(alive) {
            break Err(tr!("sin respuesta de Forge").to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    for ext in ["json", "allow", "deny"] {
        let _ = std::fs::remove_file(dir.join(format!("{}.{ext}", request.id)));
    }
    decision
}

/// Sin la app abierta: se pregunta en la terminal, si la hay; si no, se rechaza.
fn ask_tty(request: &Request) -> Result<bool, String> {
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| tr!("Forge no está abierto para autorizarlo").to_string())?;
    write!(
        tty,
        "{}",
        tr!(
            "Forge: «{p0}» {p1}.\n¿Ejecutar? [s/N] ",
            p0 = request.command,
            p1 = request.reason
        )
    )
    .map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(tty), &mut line)
        .map_err(|e| e.to_string())?;
    Ok(matches!(
        line.trim().to_lowercase().as_str(),
        "s" | "si" | "sí" | "y" | "yes"
    ))
}

/// El programa real: el primero del PATH que no es un enlace de Forge.
fn real_program(name: &str, shims: Option<&Path>) -> Option<PathBuf> {
    let shims = shims.and_then(|s| s.canonicalize().ok());
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|dir| dir.canonicalize().ok() != shims || shims.is_none())
        .map(|dir| dir.join(name))
        .find(|p| {
            p.is_file()
                && std::fs::metadata(p).is_ok_and(|m| {
                    std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0
                })
                && p.canonicalize().ok()
                    != std::env::current_exe()
                        .ok()
                        .and_then(|e| e.canonicalize().ok())
        })
}

/// Punto de entrada cuando Forge se ejecuta como `rm`, `git`… (enlace en `shims/`).
pub fn shim_main(name: &str, args: Vec<String>) -> ! {
    let shims = std::env::var_os("FORGE_SHIMS").map(PathBuf::from);
    let Some(real) = real_program(name, shims.as_deref()) else {
        eprintln!(
            "{}",
            tr!("Forge: no se encontró `{name}` en el PATH", name = name)
        );
        std::process::exit(127);
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let project = std::env::var_os("FORGE_PROJECT").map(PathBuf::from);
    let dangerous = classify(name, &args, &cwd, project.as_deref()).is_some();
    // Solo en este camino poco frecuente se lee el idioma (`git status` no paga la base) y
    // se vuelve a clasificar para que el motivo salga en ese idioma.
    if dangerous {
        crate::i18n::init_from_store();
    }
    if let Some(reason) = dangerous
        .then(|| classify(name, &args, &cwd, project.as_deref()))
        .flatten()
    {
        let mut command = vec![name.to_string()];
        command.extend(args.iter().cloned());
        let request = Request {
            id: format!(
                "{}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis()),
                std::process::id()
            ),
            command: command.join(" "),
            reason,
            cwd,
            project,
            origin: std::env::var("FORGE_ORIGIN").ok(),
            pid: std::process::id(),
        };
        let dir = std::env::var_os("FORGE_APPROVALS").map(PathBuf::from);
        let app = std::env::var("FORGE_APP_PID")
            .ok()
            .and_then(|p| p.parse().ok());
        let decision = match &dir {
            Some(dir) => ask(dir, &request, app),
            None => ask_tty(&request),
        };
        match decision {
            Ok(true) => {}
            Ok(false) => {
                eprintln!(
                    "{}",
                    tr!("Forge: comando rechazado: {p0}", p0 = request.command)
                );
                std::process::exit(126);
            }
            Err(e) => {
                eprintln!(
                    "{}",
                    tr!(
                        "Forge: comando bloqueado ({e}): {p0}",
                        p0 = request.command,
                        e = e
                    )
                );
                std::process::exit(126);
            }
        }
    }
    let err = std::process::Command::new(&real)
        .arg0(name)
        .args(&args)
        .exec();
    eprintln!(
        "{}",
        tr!(
            "Forge: no se pudo ejecutar {p0}: {err}",
            p0 = real.display(),
            err = err
        )
    );
    std::process::exit(126);
}

// ------------------------------------------------------------------ instalación

/// Arranque de zsh de Forge: carga los archivos del usuario y, al final, pone los enlaces
/// primero en el PATH (después de path_helper, nvm, brew… que lo reordenan).
const ZSH_FILES: [(&str, &str); 4] = [
    (".zshenv", ""),
    (".zprofile", ""),
    (".zshrc", "forge_path"),
    (".zlogin", "forge_path"),
];

fn zsh_file(name: &str, tail: &str) -> String {
    let mut s = format!(
        "# Generado por Forge: carga el {name} del usuario.\n\
         FORGE_ZDOTDIR=$ZDOTDIR\n\
         ZDOTDIR=${{FORGE_USER_ZDOTDIR:-$HOME}}\n\
         # /etc/zshrc arma el historial con nuestro ZDOTDIR: que sea el del usuario.\n\
         [[ $HISTFILE == $FORGE_ZDOTDIR/.zsh_history ]] && HISTFILE=$ZDOTDIR/.zsh_history\n\
         [[ -f $ZDOTDIR/{name} ]] && source $ZDOTDIR/{name}\n\
         FORGE_USER_ZDOTDIR=$ZDOTDIR\n\
         ZDOTDIR=$FORGE_ZDOTDIR\n"
    );
    if tail == "forge_path" {
        s += "path=($FORGE_SHIMS ${path:#$FORGE_SHIMS})\n";
    }
    if name == ".zlogin" {
        // Último archivo de un shell de login: los zsh hijos usan la configuración del usuario.
        s += "ZDOTDIR=$FORGE_USER_ZDOTDIR\n";
    }
    s
}

/// Crea `shims/` (enlaces a `exe`) y `zsh/` dentro de `base` y devuelve las variables para
/// las terminales de Forge.
pub fn install(base: &Path, exe: &Path) -> std::io::Result<HashMap<String, String>> {
    let shims = base.join("shims");
    let zsh = base.join("zsh");
    let approvals = base.join("approvals");
    let events = base.join("events");
    for dir in [&shims, &zsh, &approvals, &events] {
        std::fs::create_dir_all(dir)?;
    }
    for name in SHIMMED {
        let link = shims.join(name);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(exe, &link)?;
    }
    for (name, tail) in ZSH_FILES {
        std::fs::write(zsh.join(name), zsh_file(name, tail))?;
    }
    let mut env = HashMap::from([
        ("FORGE_SHIMS".into(), shims.to_string_lossy().into_owned()),
        (
            "FORGE_APPROVALS".into(),
            approvals.to_string_lossy().into_owned(),
        ),
        ("FORGE_EVENTS".into(), events.to_string_lossy().into_owned()),
        ("FORGE_APP_PID".into(), std::process::id().to_string()),
    ]);
    let shell = std::env::var("SHELL").unwrap_or_default();
    if shell.ends_with("zsh") {
        if let Some(user) = std::env::var_os("ZDOTDIR") {
            env.insert(
                "FORGE_USER_ZDOTDIR".into(),
                user.to_string_lossy().into_owned(),
            );
        }
        env.insert("ZDOTDIR".into(), zsh.to_string_lossy().into_owned());
    } else {
        // ponytail: solo zsh reordena el PATH al final del arranque; en bash/fish los
        // enlaces van delante del PATH heredado (su perfil puede volver a reordenarlo).
        let path = std::env::var("PATH").unwrap_or_default();
        env.insert("PATH".into(), format!("{}:{path}", shims.display()));
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn classifies_the_plan_commands() {
        let project = Path::new("/p/bovinapp");
        let c = |p: &str, a: &str| classify(p, &args(a), project, Some(project)).is_some();
        // rm -rf: dentro del proyecto es rutina; fuera, la raíz o .git piden autorización.
        assert!(!c("rm", "-rf dist node_modules"));
        assert!(!c("rm", "-r web/.next"));
        assert!(!c("rm", "-f a.txt"));
        assert!(c("rm", "-rf ."));
        assert!(c("rm", "-rf ../otro"));
        assert!(c("rm", "-fr /"));
        assert!(c("rm", "--recursive --force /Users/diego"));
        assert!(c("rm", "-rf .git"));
        assert!(c("rm", "-r -f -- -raro ../x"));
        assert!(
            classify("rm", &args("-rf dist"), project, None).is_some(),
            "sin proyecto, siempre"
        );

        assert!(c("git", "reset --hard HEAD~1"));
        assert!(!c("git", "reset --soft HEAD~1"));
        assert!(c("git", "push --force"));
        assert!(c("git", "push -uf origin main"));
        assert!(c("git", "push --force-with-lease"));
        assert!(c("git", "push origin +main"));
        assert!(!c("git", "push -u origin main"));
        assert!(c("git", "-C web clean -fd"));
        assert!(c("git", "clean -xdf"));
        assert!(!c("git", "clean -nd"));
        assert!(!c("git", "status"));
        assert!(!c("git", "-c core.pager=cat log --format=%s"));

        assert!(c("docker", "system prune -af"));
        assert!(!c("docker", "ps -a"));
        assert!(
            classify(
                "psql",
                &["-c".into(), "DROP DATABASE bovinapp;".into()],
                project,
                None
            )
            .is_some()
        );
        assert!(classify("psql", &["-c".into(), "select 1".into()], project, None).is_none());
        assert!(c("dropdb", "bovinapp"));
    }

    #[test]
    fn approvals_round_trip() {
        let dir = std::env::temp_dir().join(format!("forge-approvals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let request = Request {
            id: "1-1".into(),
            command: "git push --force".into(),
            reason: "reescribe".into(),
            cwd: "/p".into(),
            project: None,
            origin: Some("Claude Code".into()),
            pid: std::process::id(),
        };
        let asker = {
            let (dir, request) = (dir.clone(), request.clone());
            std::thread::spawn(move || ask(&dir, &request, Some(std::process::id())))
        };
        let start = Instant::now();
        while pending(&dir).is_empty() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(pending(&dir), vec![request]);
        answer(&dir, "1-1", false).unwrap();
        assert_eq!(asker.join().unwrap(), Ok(false));
        assert!(pending(&dir).is_empty());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "no deja archivos"
        );
    }
}
