// Detección y enmascarado de secretos en las líneas añadidas.
use super::*;

// ---------------------------------------------------------------- secretos

pub(crate) static SECRET_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("clave privada", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
        ("clave de AWS", r"\b(AKIA|ASIA)[0-9A-Z]{16}\b"),
        ("token de GitHub", r"\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}\b|\bgithub_pat_[A-Za-z0-9_]{60,}"),
        ("clave de Anthropic", r"\bsk-ant-[A-Za-z0-9_-]{20,}"),
        ("clave de OpenAI", r"\bsk-(proj-)?[A-Za-z0-9_-]{32,}"),
        ("clave de Stripe", r"\b(sk|rk)_live_[0-9A-Za-z]{20,}"),
        ("token de Slack", r"\bxox[abposr]-[0-9A-Za-z-]{10,}"),
        ("clave de Google", r"\bAIza[0-9A-Za-z_-]{35}"),
        (
            "contraseña o clave en el código",
            r#"(?i)\b(password|passwd|secret|api_?key|access_?token|auth_?token|client_?secret)\b["']?\s*[:=]\s*["'][^"'\s]{8,}["']"#,
        ),
    ]
    .into_iter()
    .map(|(name, re)| (name, Regex::new(re).expect("patrón de secreto válido")))
    .collect()
});

#[derive(Debug, Clone, PartialEq)]
pub struct SecretFinding {
    pub path: String,
    pub line: usize,
    pub kind: &'static str,
    /// Fragmento enmascarado: nunca se muestra el secreto completo.
    pub preview: String,
}

/// Marcador para líneas con datos de prueba que parecen secretos (como `gitleaks:allow`).
pub const ALLOW_SECRET: &str = "forge:allow-secret";

/// Sustituye los secretos detectables por un marcador (antes de enviar texto a un modelo).
pub fn mask_secrets(text: &str) -> String {
    let mut out = text.to_string();
    for (_, re) in SECRET_PATTERNS.iter() {
        out = re.replace_all(&out, "[SECRETO OCULTO]").into_owned();
    }
    out
}

pub fn scan_secrets(lines: &[(String, usize, String)]) -> Vec<SecretFinding> {
    lines
        .iter()
        .filter(|(_, _, text)| !text.contains(ALLOW_SECRET))
        .filter_map(|(path, line, text)| {
            SECRET_PATTERNS.iter().find_map(|(kind, re)| {
                let m = re.find(text)?;
                let preview = format!("{}••••", m.as_str().chars().take(6).collect::<String>());
                Some(SecretFinding {
                    path: path.clone(),
                    line: *line,
                    kind,
                    preview,
                })
            })
        })
        .collect()
}
