// Actualizaciones: consulta la última versión publicada en GitHub (releases), descarga su
// .dmg y deja la app nueva preparada; al reiniciar, un script sustituye la app y la abre.
// Con `gh` autenticado sirve también para repos privados; si no, la API pública.
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    /// Página de la versión (para verla o descargarla a mano).
    pub page: String,
    /// El .dmg publicado, si lo hay.
    pub dmg: Option<String>,
    /// Su huella SHA-256 (`<dmg>.sha256`): sin ella no se instala nada.
    pub checksum: Option<String>,
}

/// Última versión publicada.
pub fn latest(repo_url: &str) -> Option<Release> {
    let repo = repo_url
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")?;
    let api = format!("repos/{repo}/releases/latest");
    // La app abierta desde el Finder no tiene el PATH de la terminal: rutas habituales.
    let gh = ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    let json = gh
        .and_then(|gh| Command::new(gh).args(["api", &api]).output().ok())
        .filter(|o| o.status.success())
        .or_else(|| {
            Command::new("/usr/bin/curl")
                .args([
                    "-fsSL",
                    "-m",
                    "10",
                    &format!("https://api.github.com/{api}"),
                ])
                .output()
                .ok()
                .filter(|o| o.status.success())
        })?
        .stdout;
    let v: serde_json::Value = serde_json::from_slice(&json).ok()?;
    let asset = |suffix: &str| {
        v["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["browser_download_url"].as_str())
            .find(|u| u.ends_with(suffix))
            .map(str::to_string)
    };
    Some(Release {
        tag: v["tag_name"].as_str()?.to_string(),
        page: v["html_url"].as_str()?.to_string(),
        dmg: asset(".dmg"),
        checksum: asset(".dmg.sha256"),
    })
}

/// El Forge.app en ejecución (None si no se ejecuta desde un .app, p. ej. `cargo run`).
pub fn current_app() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Comprueba que el archivo tenga la huella publicada (salida de `shasum -a 256`).
fn verify(file: &Path, published: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let expected = published
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    let actual = crate::guard::diff::hex(&Sha256::digest(&bytes));
    if expected.len() == 64 && actual == expected {
        Ok(())
    } else {
        Err(crate::tr!("la huella SHA-256 de la descarga no coincide con la publicada").into())
    }
}

/// Descarga el .dmg, comprueba su huella y copia su Forge.app a una carpeta temporal;
/// devuelve esa app. Bajado con curl no lleva cuarentena: al abrirla, macOS no pregunta.
pub fn download(dmg_url: &str, checksum_url: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("forge-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dmg = dir.join("Forge.dmg");
    let mount = dir.join("mnt");
    run(Command::new("/usr/bin/curl")
        .args(["-fsSL", "-m", "600", "-o"])
        .arg(&dmg)
        .arg(dmg_url))?;
    let published = Command::new("/usr/bin/curl")
        .args(["-fsSL", "-m", "30", checksum_url])
        .output()
        .map_err(|e| e.to_string())?;
    if let Err(e) = verify(&dmg, &String::from_utf8_lossy(&published.stdout)) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    run(Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount)
        .arg(&dmg))?;
    let app = dir.join("Forge.app");
    let copied = run(Command::new("/usr/bin/ditto")
        .arg(mount.join("Forge.app"))
        .arg(&app));
    let _ = run(Command::new("/usr/bin/hdiutil")
        .args(["detach", "-quiet"])
        .arg(&mount));
    copied?;
    let _ = std::fs::remove_file(&dmg);
    Ok(app)
}

/// Script que espera a que Forge (pid) se cierre, sustituye la app y la vuelve a abrir.
pub fn relaunch_script(pid: u32, staged: &Path, app: &Path) -> String {
    let q = |p: &Path| crate::agents::shell_quote(&p.to_string_lossy());
    let new = q(&app.with_extension("app.new"));
    let (staged, app) = (q(staged), q(app));
    format!(
        "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; \
         rm -rf {new} && /usr/bin/ditto {staged} {new} && rm -rf {app} && mv {new} {app}; \
         open {app}"
    )
}

/// "v0.2.0" es más nueva que "0.1.0"?
pub fn is_newer(tag: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.trim_start_matches('v')
            .split(['.', '-'])
            .map_while(|p| p.parse().ok())
            .collect()
    };
    parse(tag) > parse(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(is_newer("v1.0.0-beta", "0.9.0"));
    }

    #[test]
    fn rejects_a_download_whose_checksum_differs() {
        let file = std::env::temp_dir().join(format!("forge-sha-{}", std::process::id()));
        std::fs::write(&file, "hola").unwrap();
        // shasum -a 256 de "hola"
        let good = "b221d9dbb083a7f33428d7c2a3c3198ae925614d70210e28716ccaa7cd4ddb79  Forge.dmg";
        assert!(verify(&file, good).is_ok());
        assert!(verify(&file, &good.replace('b', "c")).is_err());
        assert!(verify(&file, "").is_err(), "sin huella no se acepta");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn relaunch_replaces_the_app_after_exit() {
        let script = relaunch_script(
            42,
            Path::new("/tmp/forge-update/Forge.app"),
            Path::new("/Applications/Forge.app"),
        );
        assert!(script.starts_with("while kill -0 42"));
        assert!(script.contains("mv '/Applications/Forge.app.new' '/Applications/Forge.app'"));
        assert!(script.ends_with("open '/Applications/Forge.app'"));
    }

    /// Contra GitHub de verdad: `cargo test -p forge-core real_download -- --ignored`.
    #[test]
    #[ignore]
    fn real_download_stages_the_published_app() {
        let release = latest("https://github.com/DiegoAndres717/forge").expect("sin release");
        let Some(checksum) = release.checksum else {
            eprintln!(
                "{} no publica .sha256 (versiones anteriores a 0.1.2)",
                release.tag
            );
            return;
        };
        let app = download(&release.dmg.expect("sin .dmg"), &checksum).unwrap();
        assert!(app.join("Contents/MacOS/forge").is_file());
        let _ = std::fs::remove_dir_all(app.parent().unwrap());
    }
}
