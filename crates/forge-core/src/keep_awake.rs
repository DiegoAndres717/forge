// Mantener el equipo despierto mientras un agente trabaja (plan #348). Solo impide la
// suspensión por inactividad del sistema, no que se apague la pantalla. Se toma en cuanto hay
// un agente trabajando y se suelta cuando ninguno trabaja desde hace `RELEASE_AFTER` (así no
// parpadea entre turno y turno). Cada mecanismo se suelta solo si Forge se cierra o se cae.
use std::time::{Duration, Instant};

/// Sin agentes trabajando durante este rato, se suelta.
pub const RELEASE_AFTER: Duration = Duration::from_secs(20);

/// Lo que impide la suspensión (el del sistema, o uno de mentira en las pruebas).
pub trait Inhibitor: Send {
    fn hold(&mut self) -> Result<(), String>;
    fn release(&mut self);
    /// ¿Sigue de verdad tomado? (el proceso que lo sostiene pudo acabar por su cuenta).
    fn alive(&mut self) -> bool {
        true
    }
}

impl Inhibitor for Box<dyn Inhibitor> {
    fn hold(&mut self) -> Result<(), String> {
        (**self).hold()
    }
    fn release(&mut self) {
        (**self).release()
    }
    fn alive(&mut self) -> bool {
        (**self).alive()
    }
}

/// Decide cuándo tomar y soltar el `Inhibitor`.
pub struct KeepAwake<I: Inhibitor> {
    inhibitor: I,
    held: bool,
    /// Desde cuándo no trabaja ningún agente (con la aserción tomada).
    quiet_since: Option<Instant>,
    /// Tomarla falló: no se reintenta hasta que vuelva a empezar el trabajo.
    failed: bool,
}

impl<I: Inhibitor> KeepAwake<I> {
    pub fn new(inhibitor: I) -> Self {
        Self {
            inhibitor,
            held: false,
            quiet_since: None,
            failed: false,
        }
    }

    /// ¿Está impidiendo la suspensión ahora?
    pub fn held(&self) -> bool {
        self.held
    }

    /// Con el ajuste `enabled` y si hay algún agente trabajando (`working`) en `now`.
    pub fn update(&mut self, enabled: bool, working: bool, now: Instant) {
        // Se cayó por su cuenta (systemd-inhibit sin logind, alguien mató caffeinate): ya no
        // se tiene, y cuenta como un intento fallido.
        if self.held && !self.inhibitor.alive() {
            eprintln!("forge: lo que mantenía el equipo despierto terminó por su cuenta");
            self.held = false;
            self.quiet_since = None;
            self.failed = true;
        }
        if enabled && working {
            self.quiet_since = None;
            if !self.held && !self.failed {
                match self.inhibitor.hold() {
                    Ok(()) => self.held = true,
                    Err(e) => {
                        eprintln!("forge: no se pudo mantener el equipo despierto: {e}");
                        self.failed = true;
                    }
                }
            }
            return;
        }
        // Sin trabajo (o apagado): el próximo trabajo vuelve a intentarlo.
        self.failed = false;
        if !self.held {
            return;
        }
        let quiet = *self.quiet_since.get_or_insert(now);
        if !enabled || now.saturating_duration_since(quiet) >= RELEASE_AFTER {
            self.inhibitor.release();
            self.held = false;
            self.quiet_since = None;
        }
    }
}

impl<I: Inhibitor> Drop for KeepAwake<I> {
    fn drop(&mut self) {
        if self.held {
            self.inhibitor.release();
        }
    }
}

/// El del sistema. macOS: `caffeinate -i -w <pid de Forge>` (aserción
/// PreventUserIdleSystemSleep; `-w` la suelta si Forge muere). Linux: `systemd-inhibit` con
/// un `tail --pid` que acaba cuando Forge muere (sin systemd, falla y no hace nada). Windows:
/// `SetThreadExecutionState` en un hilo propio mientras se tiene; el sistema la quita al
/// acabar el hilo o el proceso.
#[derive(Default)]
pub struct System {
    #[cfg(unix)]
    child: Option<std::process::Child>,
    #[cfg(windows)]
    thread: Option<(std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>)>,
}

#[cfg(unix)]
impl Inhibitor for System {
    fn hold(&mut self) -> Result<(), String> {
        use std::process::{Command, Stdio};
        if self.child.is_some() {
            return Ok(());
        }
        let pid = std::process::id().to_string();
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = Command::new("/usr/bin/caffeinate");
            c.args(["-i", "-w", &pid]);
            c
        } else {
            let mut c = Command::new("systemd-inhibit");
            c.args([
                "--what=idle:sleep",
                "--who=Forge",
                &format!("--why={}", crate::tr!("Un agente está trabajando")),
                "--mode=block",
                "tail",
                &format!("--pid={pid}"),
                "-f",
                "/dev/null",
            ]);
            c
        };
        // Grupo propio: al soltar se cierran juntos systemd-inhibit y su tail.
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        self.child = Some(child);
        Ok(())
    }

    fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            // SAFETY: killpg solo envía una señal al grupo creado con process_group(0).
            unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGTERM) };
            let _ = child.wait();
        }
    }

    fn alive(&mut self) -> bool {
        let gone = self
            .child
            .as_mut()
            .is_none_or(|c| !matches!(c.try_wait(), Ok(None)));
        if gone && let Some(mut child) = self.child.take() {
            // Su grupo (el tail de systemd-inhibit) puede seguir vivo: se cierra entero.
            // SAFETY: killpg solo envía una señal al grupo creado con process_group(0).
            unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGTERM) };
            let _ = child.wait();
        }
        !gone
    }
}

#[cfg(windows)]
impl Inhibitor for System {
    fn hold(&mut self) -> Result<(), String> {
        use windows_sys::Win32::System::Power::{
            ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
        };
        if self.thread.is_some() {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        // Lo que devuelve es el estado anterior (en un hilo nuevo puede ser 0): no distingue
        // un fallo, así que no se mira.
        let handle = std::thread::spawn(move || {
            // SAFETY: solo cambia el estado de ejecución de este hilo.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            // Espera a que se suelte (se cierra el canal) y deja el estado como estaba.
            let _ = rx.recv();
            // SAFETY: igual que arriba.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        });
        self.thread = Some((tx, handle));
        Ok(())
    }

    fn release(&mut self) {
        if let Some((tx, handle)) = self.thread.take() {
            drop(tx);
            let _ = handle.join();
        }
    }
}

#[cfg(not(any(unix, windows)))]
impl Inhibitor for System {
    fn hold(&mut self) -> Result<(), String> {
        Err("sin mecanismo en este sistema".into())
    }
    fn release(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Cuenta las veces que se tomó y se soltó; `fail` hace que tomarla falle.
    #[derive(Default, Clone)]
    struct Fake {
        log: Arc<Mutex<(u32, u32)>>,
        fail: bool,
        /// El proceso que lo sostiene terminó por su cuenta.
        dead: Arc<Mutex<bool>>,
    }

    impl Inhibitor for Fake {
        fn hold(&mut self) -> Result<(), String> {
            if self.fail {
                return Err("no".into());
            }
            self.log.lock().unwrap().0 += 1;
            Ok(())
        }
        fn release(&mut self) {
            self.log.lock().unwrap().1 += 1;
        }
        fn alive(&mut self) -> bool {
            !*self.dead.lock().unwrap()
        }
    }

    #[test]
    fn a_hold_that_died_on_its_own_is_no_longer_held_and_retried_on_the_next_work() {
        let fake = Fake::default();
        let mut k = KeepAwake::new(fake.clone());
        let t = Instant::now();
        k.update(true, true, t);
        *fake.dead.lock().unwrap() = true;
        k.update(true, true, t + Duration::from_secs(1));
        assert!(!k.held(), "el indicador no miente");
        k.update(true, true, t + Duration::from_secs(2));
        assert_eq!(counts(&fake).0, 1, "no se reintenta en cada vuelta");
        *fake.dead.lock().unwrap() = false;
        k.update(true, false, t + Duration::from_secs(3));
        k.update(true, true, t + Duration::from_secs(4));
        assert!(k.held(), "otra tanda de trabajo, otro intento");
        assert_eq!(counts(&fake).0, 2);
    }

    /// Un proceso que acaba enseguida (como systemd-inhibit sin logind) no se da por tomado.
    #[cfg(unix)]
    #[test]
    fn the_system_inhibitor_notices_its_process_ended() {
        let mut s = System {
            child: Some(std::process::Command::new("true").spawn().unwrap()),
        };
        let start = Instant::now();
        while s.alive() && start.elapsed() < Duration::from_secs(5) {
            std::thread::yield_now();
        }
        assert!(!s.alive());
        assert!(s.child.is_none(), "recogido, sin zombi");
    }

    fn counts(f: &Fake) -> (u32, u32) {
        *f.log.lock().unwrap()
    }

    /// Prueba manual en un Mac (toca la aserción de verdad): `pmset -g assertions` la lista
    /// mientras se tiene y deja de listarla al soltar.
    /// `cargo test -p forge-core --lib keep_awake::tests::system -- --ignored`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn system_assertion_shows_in_pmset_while_held() {
        let ours = |s: &System| {
            let pid = s.child.as_ref().map(|c| c.id()).unwrap_or(0);
            let out = std::process::Command::new("pmset")
                .args(["-g", "assertions"])
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.lines().any(|l| {
                l.contains(&format!("pid {pid}(caffeinate)"))
                    && l.contains("PreventUserIdleSystemSleep")
            })
        };
        let mut s = System::default();
        s.hold().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(ours(&s), "pmset la lista mientras se tiene");
        assert!(s.alive(), "caffeinate sigue vivo");
        let pid = s.child.as_ref().unwrap().id();
        s.release();
        std::thread::sleep(Duration::from_millis(500));
        let out = std::process::Command::new("pmset")
            .args(["-g", "assertions"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains(&format!("pid {pid}(caffeinate)")),
            "al soltar desaparece"
        );
    }

    #[test]
    fn nothing_is_held_while_the_setting_is_off() {
        let fake = Fake::default();
        let mut k = KeepAwake::new(fake.clone());
        k.update(false, true, Instant::now());
        assert!(!k.held());
        assert_eq!(counts(&fake), (0, 0));
    }

    #[test]
    fn it_holds_once_while_an_agent_works() {
        let fake = Fake::default();
        let mut k = KeepAwake::new(fake.clone());
        let t = Instant::now();
        k.update(true, true, t);
        k.update(true, true, t + Duration::from_secs(1));
        k.update(true, true, t + Duration::from_secs(60));
        assert!(k.held());
        assert_eq!(counts(&fake), (1, 0), "una sola vez, sin soltar");
    }

    #[test]
    fn it_releases_only_after_a_quiet_spell_and_a_short_pause_does_not_flap() {
        let fake = Fake::default();
        let mut k = KeepAwake::new(fake.clone());
        let t = Instant::now();
        k.update(true, true, t);
        // Una pausa corta entre turnos (esperar permiso cuenta como no trabajar).
        k.update(true, false, t + Duration::from_secs(1));
        k.update(true, false, t + Duration::from_secs(5));
        assert!(k.held(), "una pausa corta no la suelta");
        k.update(true, true, t + Duration::from_secs(6));
        // La pausa vuelve a contar desde cero.
        k.update(true, false, t + Duration::from_secs(10));
        k.update(true, false, t + Duration::from_secs(10) + RELEASE_AFTER / 2);
        assert!(k.held());
        assert_eq!(counts(&fake), (1, 0), "sin parpadeos");
        k.update(true, false, t + Duration::from_secs(10) + RELEASE_AFTER);
        assert!(!k.held(), "tras el rato sin trabajo, se suelta");
        assert_eq!(counts(&fake), (1, 1));
        // Si vuelve el trabajo, se toma otra vez.
        k.update(true, true, t + Duration::from_secs(100));
        assert!(k.held());
        assert_eq!(counts(&fake), (2, 1));
    }

    #[test]
    fn turning_the_setting_off_releases_at_once() {
        let fake = Fake::default();
        let mut k = KeepAwake::new(fake.clone());
        let t = Instant::now();
        k.update(true, true, t);
        k.update(false, true, t + Duration::from_millis(10));
        assert!(!k.held());
        assert_eq!(counts(&fake), (1, 1));
    }

    #[test]
    fn dropping_it_releases() {
        let fake = Fake::default();
        let mut k = KeepAwake::new(fake.clone());
        k.update(true, true, Instant::now());
        drop(k);
        assert_eq!(counts(&fake), (1, 1));
    }

    #[test]
    fn a_failed_hold_is_not_retried_every_frame_but_again_when_work_restarts() {
        let fake = Fake {
            fail: true,
            ..Default::default()
        };
        let attempts = Arc::new(Mutex::new(0));
        struct Counting(Fake, Arc<Mutex<u32>>);
        impl Inhibitor for Counting {
            fn hold(&mut self) -> Result<(), String> {
                *self.1.lock().unwrap() += 1;
                self.0.hold()
            }
            fn release(&mut self) {
                self.0.release()
            }
        }
        let mut k = KeepAwake::new(Counting(fake.clone(), attempts.clone()));
        let t = Instant::now();
        k.update(true, true, t);
        k.update(true, true, t + Duration::from_secs(1));
        assert!(!k.held());
        assert_eq!(
            *attempts.lock().unwrap(),
            1,
            "no se reintenta en cada vuelta"
        );
        k.update(true, false, t + Duration::from_secs(2));
        k.update(true, true, t + Duration::from_secs(3));
        assert_eq!(
            *attempts.lock().unwrap(),
            2,
            "otra tanda de trabajo, otro intento"
        );
        drop(k);
        assert_eq!(counts(&fake), (0, 0), "nada que soltar");
    }
}
