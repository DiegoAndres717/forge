// Cada terminal es responsable de sí misma ante macOS (permisos de Fotos, Música…):
// `forge --disclaim` se convierte en el programa sin que Forge sea su responsable.
use std::process::{Command, Stdio};
use std::time::Duration;

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

unsafe extern "C" {
    fn responsibility_get_pid_responsible_for_pid(pid: libc::pid_t) -> libc::pid_t;
}

#[test]
fn terminal_programs_are_responsible_for_themselves() {
    let responsible = |args: &[&str]| {
        let mut child = Command::new(args[0]).args(&args[1..]).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let pid = child.id() as libc::pid_t;
        // SAFETY: función de libSystem sin efectos; el proceso sigue vivo.
        let r = unsafe { responsibility_get_pid_responsible_for_pid(pid) };
        child.kill().unwrap();
        child.wait().unwrap();
        (pid, r)
    };
    let (pid, r) = responsible(&["/bin/sleep", "5"]);
    assert_ne!(r, pid, "sin trampolín hereda el responsable");
    let (pid, r) = responsible(&[FORGE, "--disclaim", "/bin/sleep", "sleep", "5"]);
    assert_eq!(r, pid);
}

#[test]
fn disclaim_keeps_argv_signals_and_exit_code() {
    // argv0 propio, SIGPIPE por defecto (Rust lo ignora y no debe heredarse) y su código.
    let out = Command::new(FORGE)
        .args([
            "--disclaim",
            "/bin/bash",
            "-login",
            "-c",
            "echo $0; yes | head -1 >/dev/null; echo ${PIPESTATUS[0]}; exit 7",
        ])
        .stderr(Stdio::inherit())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "-login\n141\n");
    assert_eq!(out.status.code(), Some(7));
    let missing = Command::new(FORGE)
        .args(["--disclaim", "/no/existe", "x"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(127));
}
