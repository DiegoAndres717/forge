// Aviso de versión nueva: consulta la última versión publicada en GitHub (releases).
// Con `gh` autenticado sirve también para repos privados; si no, la API pública.
use std::process::Command;

/// Última versión publicada: (etiqueta, página de la versión).
pub fn latest(repo_url: &str) -> Option<(String, String)> {
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
    Some((
        v["tag_name"].as_str()?.to_string(),
        v["html_url"].as_str()?.to_string(),
    ))
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
}
