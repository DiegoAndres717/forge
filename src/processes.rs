// Procesos administrados: iniciar/detener/reiniciar, reinicio automático, puertos y health checks.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::terminal::Terminal;
use forge_core::project::{AutoRestart, HealthCheck, ProcessDef};

const STOP_GRACE: Duration = Duration::from_secs(5);
const POLL_EVERY: Duration = Duration::from_secs(2);
const MAX_ATTEMPTS: u32 = 10;
/// Tras este tiempo estable, el contador de reintentos vuelve a cero.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// Margen de arranque en el que un health check fallido se muestra como "iniciando".
const STARTUP_GRACE: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum Status {
    Stopped,
    Running {
        since: Instant,
    },
    Stopping {
        deadline: Instant,
        then_start: bool,
    },
    /// Espera antes de un reinicio automático.
    Waiting {
        until: Instant,
    },
    Exited {
        code: u32,
    },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Health {
    Unknown,
    Healthy,
    Unhealthy(String),
}

pub struct Managed {
    pub def: ProcessDef,
    /// Ejecución actual o la última (sus logs siguen visibles tras terminar).
    pub terminal: Option<Terminal>,
    pub status: Status,
    attempts: u32,
    pub ports: Vec<u16>,
    pub health: Health,
    next_health: Instant,
}

/// Color del indicador de estado.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    Ok,
    Busy,
    Bad,
    Idle,
}

impl Managed {
    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            Status::Running { .. } | Status::Stopping { .. } | Status::Waiting { .. }
        )
    }

    /// Texto y color del estado, como en el panel PROCESOS.
    pub fn describe(&self) -> (String, Tone) {
        match &self.status {
            Status::Stopped => ("detenido".into(), Tone::Idle),
            Status::Running { since } => match &self.health {
                Health::Healthy => ("healthy".into(), Tone::Ok),
                Health::Unhealthy(_) if since.elapsed() < STARTUP_GRACE => {
                    ("iniciando…".into(), Tone::Busy)
                }
                Health::Unhealthy(why) => (format!("unhealthy: {why}"), Tone::Bad),
                Health::Unknown => ("ejecutándose".into(), Tone::Ok),
            },
            Status::Stopping { .. } => ("deteniendo…".into(), Tone::Busy),
            Status::Waiting { until } => {
                let secs = until.saturating_duration_since(Instant::now()).as_secs() + 1;
                (
                    format!("reinicio en {secs} s (intento {})", self.attempts),
                    Tone::Busy,
                )
            }
            Status::Exited { code: 0 } => ("terminó (código 0)".into(), Tone::Idle),
            Status::Exited { code } => (format!("falló (código {code})"), Tone::Bad),
            Status::Failed(why) => (format!("no se pudo iniciar: {why}"), Tone::Bad),
        }
    }

    pub fn urls(&self) -> Vec<String> {
        let mut urls = self
            .terminal
            .as_ref()
            .map(Terminal::urls)
            .unwrap_or_default();
        if urls.is_empty() {
            urls = self
                .ports
                .iter()
                .map(|p| format!("http://localhost:{p}"))
                .collect();
        }
        urls
    }
}

/// Resultado del sondeo en segundo plano para un proceso.
struct Probe {
    id: String,
    ports: Vec<u16>,
    health: Option<Health>,
}

pub struct Processes {
    pub list: Vec<Managed>,
    root: PathBuf,
    env: HashMap<String, String>,
    poll: Option<Receiver<Vec<Probe>>>,
    last_poll: Instant,
}

impl Processes {
    pub fn new(defs: &[ProcessDef], root: PathBuf, env: HashMap<String, String>) -> Self {
        let mut processes = Self {
            list: Vec::new(),
            root,
            env,
            poll: None,
            last_poll: Instant::now(),
        };
        processes.sync(defs);
        processes
    }

    /// Aplica una configuración recargada: conserva los procesos que siguen definidos
    /// (aunque cambie su comando, que se usará en el próximo inicio), añade los nuevos
    /// y detiene los eliminados.
    pub fn sync(&mut self, defs: &[ProcessDef]) {
        let mut old: HashMap<String, Managed> =
            self.list.drain(..).map(|m| (m.def.id.clone(), m)).collect();
        self.list = defs
            .iter()
            .map(|def| match old.remove(&def.id) {
                Some(mut m) => {
                    m.def = def.clone();
                    m
                }
                None => Managed {
                    def: def.clone(),
                    terminal: None,
                    status: Status::Stopped,
                    attempts: 0,
                    ports: Vec::new(),
                    health: Health::Unknown,
                    next_health: Instant::now(),
                },
            })
            .collect();
        // Los que ya no existen se sueltan aquí: Drop mata su grupo de procesos.
    }

    pub fn set_env(&mut self, root: PathBuf, env: HashMap<String, String>) {
        self.root = root;
        self.env = env;
    }

    pub fn get(&self, id: &str) -> Option<&Managed> {
        self.list.iter().find(|m| m.def.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Managed> {
        self.list.iter_mut().find(|m| m.def.id == id)
    }

    pub fn active_ids(&self) -> Vec<String> {
        self.list
            .iter()
            .filter(|m| m.is_active())
            .map(|m| m.def.id.clone())
            .collect()
    }

    pub fn active_count(&self) -> usize {
        self.list.iter().filter(|m| m.is_active()).count()
    }

    /// Variables efectivas del proceso: las del proyecto y encima las suyas.
    pub fn environment(&self, def: &ProcessDef) -> HashMap<String, String> {
        let mut env = self.env.clone();
        env.extend(def.environment.clone());
        env
    }

    pub fn start(&mut self, ctx: &egui::Context, id: &str) {
        let (root, env) = match self.get(id) {
            Some(m) => (self.root.clone(), self.environment(&m.def)),
            None => return,
        };
        let Some(m) = self.get_mut(id) else { return };
        if matches!(m.status, Status::Running { .. } | Status::Stopping { .. }) {
            return;
        }
        let cwd = m
            .def
            .working_directory
            .as_ref()
            .map_or_else(|| root.clone(), |d| root.join(d));
        match Terminal::exec(ctx, &cwd, &m.def.command, &env) {
            Ok(terminal) => {
                m.terminal = Some(terminal);
                m.status = Status::Running {
                    since: Instant::now(),
                };
                m.ports.clear();
                m.health = Health::Unknown;
                m.next_health = Instant::now() + Duration::from_secs(1);
            }
            Err(e) => m.status = Status::Failed(e.to_string()),
        }
    }

    /// Parada limpia: Ctrl+C y, si no termina en 5 s, SIGKILL al grupo.
    pub fn stop(&mut self, id: &str) {
        let Some(m) = self.get_mut(id) else { return };
        match m.status {
            Status::Running { .. } => {
                if let Some(t) = &m.terminal {
                    t.interrupt();
                }
                m.status = Status::Stopping {
                    deadline: Instant::now() + STOP_GRACE,
                    then_start: false,
                };
            }
            Status::Stopping { deadline, .. } => {
                m.status = Status::Stopping {
                    deadline,
                    then_start: false,
                };
            }
            Status::Waiting { .. } => m.status = Status::Stopped,
            _ => {}
        }
        m.attempts = 0;
    }

    pub fn restart(&mut self, ctx: &egui::Context, id: &str) {
        let Some(m) = self.get_mut(id) else { return };
        m.attempts = 0;
        match m.status {
            Status::Running { .. } | Status::Stopping { .. } => {
                if let Some(t) = &m.terminal {
                    t.interrupt();
                }
                m.status = Status::Stopping {
                    deadline: Instant::now() + STOP_GRACE,
                    then_start: true,
                };
            }
            _ => self.start(ctx, id),
        }
    }

    /// Avanza la máquina de estados y recoge el sondeo. Devuelve si hay procesos activos
    /// (para seguir repintando aunque no lleguen eventos).
    pub fn tick(&mut self, ctx: &egui::Context) -> bool {
        let now = Instant::now();
        let mut to_start = Vec::new();
        for m in &mut self.list {
            let code = m.terminal.as_mut().and_then(Terminal::exit_code);
            match m.status {
                Status::Running { since } => {
                    if let Some(code) = code {
                        if since.elapsed() > STABLE_AFTER {
                            m.attempts = 0;
                        }
                        let again = match m.def.auto_restart {
                            AutoRestart::Always => true,
                            AutoRestart::OnFailure => code != 0,
                            AutoRestart::Never => false,
                        };
                        m.ports.clear();
                        m.status = if again && m.attempts < MAX_ATTEMPTS {
                            m.attempts += 1;
                            Status::Waiting {
                                until: now + backoff(m.attempts),
                            }
                        } else {
                            Status::Exited { code }
                        };
                    }
                }
                Status::Stopping {
                    deadline,
                    then_start,
                } => {
                    if code.is_some() {
                        m.ports.clear();
                        m.status = Status::Stopped;
                        if then_start {
                            to_start.push(m.def.id.clone());
                        }
                    } else if now >= deadline
                        && let Some(t) = m.terminal.as_mut()
                    {
                        t.kill(libc::SIGKILL);
                    }
                }
                Status::Waiting { until } if now >= until => to_start.push(m.def.id.clone()),
                _ => {}
            }
        }
        for id in to_start {
            self.start(ctx, &id);
        }

        if let Some(results) = self.poll.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.poll = None;
            for probe in results {
                if let Some(m) = self
                    .get_mut(&probe.id)
                    .filter(|m| matches!(m.status, Status::Running { .. }))
                {
                    m.ports = probe.ports;
                    if let Some(health) = probe.health {
                        m.health = health;
                        let every = m
                            .def
                            .health_check
                            .as_ref()
                            .map_or(5, |h| h.interval_seconds);
                        m.next_health = Instant::now() + Duration::from_secs(every.max(1));
                    }
                }
            }
        }
        if self.poll.is_none() && self.last_poll.elapsed() >= POLL_EVERY {
            self.start_poll(ctx);
        }
        self.list.iter().any(Managed::is_active)
    }

    /// Lanza en un hilo la lectura de puertos (ps + lsof) y los health checks pendientes.
    fn start_poll(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let jobs: Vec<(String, u32, Option<HealthCheck>)> = self
            .list
            .iter()
            .filter(|m| matches!(m.status, Status::Running { .. }))
            .filter_map(|m| {
                let pid = m.terminal.as_ref()?.pid()?;
                let check = m.def.health_check.clone().filter(|_| now >= m.next_health);
                Some((m.def.id.clone(), pid, check))
            })
            .collect();
        self.last_poll = now;
        if jobs.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        let (ctx, root) = (ctx.clone(), self.root.clone());
        std::thread::spawn(move || {
            let tree = process_tree();
            let all: Vec<u32> = jobs
                .iter()
                .flat_map(|(_, pid, _)| descendants(&tree, *pid))
                .collect();
            let listening = listening_ports(&all);
            let results = jobs
                .into_iter()
                .map(|(id, pid, check)| {
                    let mut ports: Vec<u16> = descendants(&tree, pid)
                        .iter()
                        .flat_map(|p| listening.get(p).cloned().unwrap_or_default())
                        .collect();
                    ports.sort_unstable();
                    ports.dedup();
                    Probe {
                        id,
                        ports,
                        health: check.map(|c| run_health_check(&c, &root)),
                    }
                })
                .collect();
            let _ = tx.send(results);
            ctx.request_repaint();
        });
        self.poll = Some(rx);
    }
}

/// Memoria residente (bytes) de unos procesos y todos sus descendientes.
pub fn memory_usage(roots: &[u32]) -> u64 {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,rss="])
        .output();
    let text = output
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    sum_rss(&text, roots)
}

fn sum_rss(ps: &str, roots: &[u32]) -> u64 {
    let mut rss = HashMap::new();
    for line in ps.lines() {
        let mut parts = line
            .split_whitespace()
            .filter_map(|p| p.parse::<u64>().ok());
        if let (Some(pid), Some(_), Some(kb)) = (parts.next(), parts.next(), parts.next()) {
            rss.insert(pid as u32, kb);
        }
    }
    let tree = parse_ps(ps);
    let mut seen = std::collections::HashSet::new();
    roots
        .iter()
        .flat_map(|r| descendants(&tree, *r))
        .filter(|p| seen.insert(*p))
        .map(|p| rss.get(&p).copied().unwrap_or(0) * 1024)
        .sum()
}

/// 1 s, 2 s, 4 s… hasta 30 s.
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.saturating_sub(1).min(5)).min(30))
}

/// pid → hijos, a partir de `ps`.
fn process_tree() -> HashMap<u32, Vec<u32>> {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output();
    parse_ps(
        &output
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default(),
    )
}

fn parse_ps(text: &str) -> HashMap<u32, Vec<u32>> {
    let mut tree: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in text.lines() {
        let mut parts = line
            .split_whitespace()
            .filter_map(|p| p.parse::<u32>().ok());
        if let (Some(pid), Some(ppid)) = (parts.next(), parts.next()) {
            tree.entry(ppid).or_default().push(pid);
        }
    }
    tree
}

/// El proceso y todos sus descendientes.
fn descendants(tree: &HashMap<u32, Vec<u32>>, root: u32) -> Vec<u32> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        out.extend(tree.get(&out[i]).into_iter().flatten().copied());
        i += 1;
    }
    out
}

/// Puertos TCP en escucha por pid (lsof).
fn listening_ports(pids: &[u32]) -> HashMap<u32, Vec<u16>> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let output = std::process::Command::new("lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-p", &list, "-Fpn"])
        .output();
    parse_lsof(
        &output
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default(),
    )
}

/// Formato `-Fpn`: líneas `p<pid>` seguidas de `n<dirección>:<puerto>`.
fn parse_lsof(text: &str) -> HashMap<u32, Vec<u16>> {
    let mut ports: HashMap<u32, Vec<u16>> = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(n), Some(pid)) = (line.strip_prefix('n'), pid)
            && let Some(port) = n.rsplit(':').next().and_then(|p| p.parse().ok())
        {
            let entry = ports.entry(pid).or_default();
            if !entry.contains(&port) {
                entry.push(port);
            }
        }
    }
    ports
}

fn run_health_check(check: &HealthCheck, root: &std::path::Path) -> Health {
    let timeout = Duration::from_secs(check.timeout_seconds.max(1));
    let result = if let Some(port) = check.port {
        connect("localhost", port, timeout)
            .map(drop)
            .map_err(|_| format!("puerto {port} cerrado"))
    } else if let Some(url) = &check.url {
        http_status(url, timeout).and_then(|code| {
            if (200..400).contains(&code) {
                Ok(())
            } else {
                Err(format!("HTTP {code}"))
            }
        })
    } else if let Some(command) = &check.command {
        command_ok(command, root, timeout)
    } else {
        Ok(())
    };
    match result {
        Ok(()) => Health::Healthy,
        Err(why) => Health::Unhealthy(why),
    }
}

fn connect(host: &str, port: u16, timeout: Duration) -> std::io::Result<TcpStream> {
    let mut last = std::io::Error::other("sin direcciones");
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// `http://host[:puerto][/ruta]` → (host, puerto, ruta)
fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = rest
        .find('/')
        .map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || authority.starts_with('[') => (h, p.parse().ok()?),
        _ => (authority, 80),
    };
    Some((
        host.trim_matches(['[', ']']).to_string(),
        port,
        path.to_string(),
    ))
}

// ponytail: HTTP/1.1 mínimo sin TLS; para https el health check usa `port` o `command`.
fn http_status(url: &str, timeout: Duration) -> Result<u16, String> {
    let (host, port, path) = parse_http_url(url).ok_or_else(|| format!("URL no válida: {url}"))?;
    let mut stream =
        connect(&host, port, timeout).map_err(|_| format!("sin respuesta en {host}:{port}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: forge\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut head = [0u8; 64];
    let n = stream
        .read(&mut head)
        .map_err(|_| "sin respuesta HTTP".to_string())?;
    let line = String::from_utf8_lossy(&head[..n]);
    line.split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| "respuesta HTTP inválida".into())
}

fn command_ok(command: &str, root: &std::path::Path, timeout: Duration) -> Result<(), String> {
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", command])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!(
                    "`{command}` salió con {}",
                    status.code().unwrap_or(-1)
                ));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return Err(format!("`{command}` tardó más de {} s", timeout.as_secs()));
            }
        }
    }
}

/// Oculta valores de variables que parecen secretas (solo para mostrarlas).
pub fn mask(key: &str, value: &str) -> String {
    let key = key.to_ascii_uppercase();
    let secret = [
        "KEY",
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASS",
        "PRIVATE",
        "CREDENTIAL",
        "AUTH",
    ]
    .iter()
    .any(|s| key.contains(s));
    if secret {
        format!("{}••••", value.chars().take(2).collect::<String>())
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_process_tree_and_ports() {
        let tree = parse_ps("  1     0\n 100     1\n 101   100\n 102   101\n 200     1\n");
        let mut d = descendants(&tree, 100);
        d.sort();
        assert_eq!(d, vec![100, 101, 102]);

        let ports = parse_lsof("p101\nn*:5173\nn[::1]:5173\np102\nn127.0.0.1:24678\n");
        assert_eq!(ports[&101], vec![5173]);
        assert_eq!(ports[&102], vec![24678]);
    }

    #[test]
    fn sums_memory_of_process_trees() {
        let ps =
            "  1     0  100\n 10     1  2048\n 11    10  1024\n 12    11  512\n 20     1  4096\n";
        assert_eq!(sum_rss(ps, &[10]), (2048 + 1024 + 512) * 1024);
        assert_eq!(
            sum_rss(ps, &[10, 11, 20]),
            (2048 + 1024 + 512 + 4096) * 1024,
            "sin contar dos veces"
        );
    }

    #[test]
    fn http_urls_backoff_and_masking() {
        assert_eq!(
            parse_http_url("http://localhost:3000/health"),
            Some(("localhost".into(), 3000, "/health".into()))
        );
        assert_eq!(
            parse_http_url("http://example.com"),
            Some(("example.com".into(), 80, "/".into()))
        );
        assert_eq!(
            parse_http_url("http://[::1]:8080/x"),
            Some(("::1".into(), 8080, "/x".into()))
        );
        assert_eq!(parse_http_url("https://x"), None);

        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(4));
        assert_eq!(backoff(10), Duration::from_secs(30));

        assert_eq!(mask("STRIPE_SECRET_KEY", "sk_live_123"), "sk••••");
        assert_eq!(mask("NODE_ENV", "development"), "development");
    }

    #[test]
    fn health_checks_against_real_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n");
            }
        });
        let check = |port, url: Option<String>, command: Option<&str>| HealthCheck {
            port,
            url,
            command: command.map(String::from),
            interval_seconds: 5,
            timeout_seconds: 2,
        };
        let root = std::path::Path::new("/tmp");
        assert_eq!(
            run_health_check(&check(Some(port), None, None), root),
            Health::Healthy
        );
        assert_eq!(
            run_health_check(
                &check(None, Some(format!("http://127.0.0.1:{port}/")), None),
                root
            ),
            Health::Unhealthy("HTTP 503".into())
        );
        assert_eq!(
            run_health_check(&check(None, None, Some("true")), root),
            Health::Healthy
        );
        assert!(matches!(
            run_health_check(&check(None, None, Some("exit 2")), root),
            Health::Unhealthy(_)
        ));
    }

    fn wait_until(
        processes: &mut Processes,
        ctx: &egui::Context,
        mut cond: impl FnMut(&Processes) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !cond(processes) {
            assert!(
                Instant::now() < deadline,
                "estado: {:?}",
                processes.list[0].status
            );
            processes.tick(ctx);
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Proceso real: arranca, detecta su puerto, se detiene con Ctrl+C y se reinicia solo al fallar.
    #[test]
    fn lifecycle_with_real_process() {
        let ctx = egui::Context::default();
        let def = |id: &str, command: &str, auto_restart| ProcessDef {
            id: id.into(),
            name: None,
            command: command.into(),
            working_directory: None,
            environment: HashMap::from([("FORGE_TEST".into(), "1".into())]),
            restart: Default::default(),
            auto_restart,
            health_check: None,
        };
        let server = "exec python3 -c 'import socket,time;s=socket.socket();s.bind((\"127.0.0.1\",0));s.listen();print(\"listo\",flush=True);time.sleep(60)'";
        let mut p = Processes::new(
            &[def("srv", server, AutoRestart::Never)],
            "/tmp".into(),
            HashMap::new(),
        );

        p.start(&ctx, "srv");
        wait_until(&mut p, &ctx, |p| !p.list[0].ports.is_empty());
        p.stop("srv");
        wait_until(&mut p, &ctx, |p| {
            matches!(p.list[0].status, Status::Stopped)
        });

        // Falla al instante: debe programar un reinicio automático.
        p.sync(&[def("bad", "exit 7", AutoRestart::OnFailure)]);
        p.start(&ctx, "bad");
        wait_until(&mut p, &ctx, |p| {
            matches!(p.list[0].status, Status::Waiting { .. })
        });
        assert_eq!(p.list[0].attempts, 1);
        p.stop("bad");
        assert!(matches!(p.list[0].status, Status::Stopped));

        // Sin reinicio: queda el código de salida.
        p.sync(&[def("once", "exit 7", AutoRestart::Never)]);
        p.start(&ctx, "once");
        wait_until(&mut p, &ctx, |p| {
            matches!(p.list[0].status, Status::Exited { code: 7 })
        });
    }
}

#[cfg(test)]
mod orphan_tests {
    use super::*;

    fn alive(pattern: &str) -> bool {
        std::process::Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Al soltar el gestor (cerrar proyecto o app) no deben quedar nietos vivos.
    #[test]
    fn drop_kills_background_children() {
        let ctx = egui::Context::default();
        let def = ProcessDef {
            id: "bg".into(),
            name: None,
            command: "sleep 3011 & sleep 3012".into(),
            working_directory: None,
            environment: HashMap::new(),
            restart: Default::default(),
            auto_restart: AutoRestart::Never,
            health_check: None,
        };
        let mut p = Processes::new(&[def], "/tmp".into(), HashMap::new());
        p.start(&ctx, "bg");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !(alive("sleep 3011") && alive("sleep 3012")) {
            assert!(Instant::now() < deadline, "no arrancó");
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(p);
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive("sleep 3011") || alive("sleep 3012") {
            assert!(Instant::now() < deadline, "quedaron procesos huérfanos");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
