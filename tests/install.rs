#![cfg(windows)]
// El instalador de Windows (scripts/install.ps1) contra una «release» local con el forge.exe
// recién compilado: instala, actualiza encima y rechaza una descarga alterada.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FORGE: &str = env!("CARGO_BIN_EXE_forge");

/// Windows PowerShell 5.1 con su propio PSModulePath (el del CI es el de PowerShell 7).
fn powershell(script: &str) -> Output {
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env_remove("PSModulePath")
        .output()
        .unwrap()
}

fn install(release: &Path, dir: &Path) -> Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("install.ps1");
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .env("FORGE_VERSION", "9.9.9")
        .env("FORGE_RELEASE_DIR", release)
        .env("FORGE_DIR", dir)
        .env("FORGE_SKIP_USER_SETUP", "1")
        // Lanzado desde PowerShell 7, como desde una terminal de Forge: el instalador no
        // puede depender de los módulos de Windows PowerShell.
        .output()
        .unwrap()
}

#[test]
fn installs_updates_and_rejects_a_tampered_download() {
    let base = std::env::temp_dir().join(format!("forge-install-test-{}", std::process::id()));
    let (release, dir) = (base.join("release"), base.join("Forge"));
    std::fs::create_dir_all(&release).unwrap();
    let zip = release.join(format!(
        "forge-9.9.9-windows-{}.zip",
        std::env::consts::ARCH
    ));
    let made = powershell(&format!(
        "$ErrorActionPreference = 'Stop'; \
         Compress-Archive -Path '{FORGE}' -DestinationPath '{}'; \
         $h = (Get-FileHash '{}' -Algorithm SHA256).Hash.ToLower(); \
         \"$h  x.zip\" | Out-File -Encoding ascii '{}.sha256'",
        zip.display(),
        zip.display(),
        zip.display()
    ));
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );

    for round in ["instala", "actualiza encima"] {
        let out = install(&release, &dir);
        assert!(
            out.status.success(),
            "{round}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let exe: PathBuf = dir.join("forge.exe");
    let help = Command::new(&exe).arg("help").output().unwrap();
    assert!(help.status.success(), "el forge.exe instalado no arranca");

    std::fs::write(
        release.join(format!(
            "{}.sha256",
            zip.file_name().unwrap().to_string_lossy()
        )),
        "0000  x.zip",
    )
    .unwrap();
    let before = std::fs::metadata(&exe).unwrap().modified().unwrap();
    let out = install(&release, &dir);
    assert!(
        !out.status.success(),
        "aceptó una suma SHA-256 que no coincide"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("SHA-256"));
    assert_eq!(std::fs::metadata(&exe).unwrap().modified().unwrap(), before);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn picks_the_architecture_from_windows_itself() {
    // En un PC real, RuntimeInformation salió vacío en Windows PowerShell 5.1: la
    // arquitectura sale de PROCESSOR_ARCHITECTURE, la que Windows da a todo proceso.
    let base = std::env::temp_dir().join(format!("forge-install-arch-{}", std::process::id()));
    let out = {
        let mut cmd = Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("scripts")
                .join("install.ps1"),
        )
        .env("FORGE_VERSION", "9.9.9")
        .env("FORGE_RELEASE_DIR", &base)
        .env("FORGE_DIR", base.join("Forge"))
        .env("FORGE_SKIP_USER_SETUP", "1")
        .env("PROCESSOR_ARCHITECTURE", "x86")
        .env_remove("PROCESSOR_ARCHITEW6432");
        cmd.output().unwrap()
    };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && err.contains("no soportada: x86"),
        "{err}"
    );
}
