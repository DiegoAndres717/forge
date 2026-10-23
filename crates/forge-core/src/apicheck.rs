// «Probar la clave» de una cuenta con API: una petición mínima (un token) al servicio, para
// decir en palabras claras si la clave, la dirección y el modelo valen antes de usarla.
use std::io::Write;
use std::process::{Command, Stdio};

use crate::tr;

/// La petición de prueba: dirección, cabeceras (con la clave) y cuerpo.
#[derive(Debug, PartialEq)]
pub struct CheckRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Un dato opcional sin espacios; vacío cuenta como que no está.
fn given(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|v| !v.is_empty())
}

/// Arma la petición mínima (un token de salida) de cada proveedor. Los que no son Anthropic
/// reciben la clave también como `Authorization: Bearer`, que es lo que algunos esperan.
pub fn check_request(
    provider: &str,
    base_url: Option<&str>,
    model: Option<&str>,
    key: &str,
) -> Result<CheckRequest, String> {
    let (base, model, bearer) = match provider {
        "anthropic" => (
            "https://api.anthropic.com".to_string(),
            given(model).unwrap_or("claude-haiku-5-5").to_string(),
            false,
        ),
        "deepseek" => (
            "https://api.deepseek.com/anthropic".to_string(),
            given(model).unwrap_or("deepseek-flash").to_string(),
            true,
        ),
        "custom" => {
            let base = given(base_url).ok_or_else(|| tr!("Falta la dirección.").to_string())?;
            let model = given(model).ok_or_else(|| tr!("Falta el modelo.").to_string())?;
            (base.to_string(), model.to_string(), true)
        }
        other => return Err(tr!("Proveedor desconocido: {p}", p = other)),
    };
    let mut headers = vec![
        ("x-api-key".to_string(), key.trim().to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
    ];
    if bearer {
        headers.push((
            "authorization".to_string(),
            format!("Bearer {}", key.trim()),
        ));
    }
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hola"}],
    })
    .to_string();
    Ok(CheckRequest {
        url: format!("{}/v1/messages", base.trim_end_matches('/')),
        headers,
        body,
    })
}

/// Lo que respondió el servicio, en palabras claras. Un límite de uso (429) quiere decir
/// que la clave es buena.
pub fn explain(status: u16, body: &str) -> Result<(), String> {
    match status {
        200..=299 | 429 => Ok(()),
        401 | 403 => Err(tr!("La clave no es válida o no tiene permiso.").to_string()),
        404 => Err(tr!("No se encontró la dirección o el modelo. Revisa los dos.").to_string()),
        other => {
            let detail: String = body.trim().chars().take(120).collect();
            if detail.is_empty() {
                Err(tr!("El servicio respondió con el error {s}.", s = other))
            } else {
                Err(tr!(
                    "El servicio respondió con el error {s}: {d}",
                    s = other,
                    d = detail
                ))
            }
        }
    }
}

/// Separa la salida de curl (`cuerpo\nestado`) en cuerpo y código.
fn split_status(out: &str) -> Option<(&str, u16)> {
    let (body, code) = out.trim_end().rsplit_once('\n')?;
    Some((body, code.trim().parse().ok()?))
}

/// Un valor entre comillas en un archivo de configuración de curl.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Hace la prueba con el curl del sistema. La clave viaja por la entrada de curl, no por
/// sus argumentos: así no aparece en la lista de procesos.
pub fn check(
    provider: &str,
    base_url: Option<&str>,
    model: Option<&str>,
    key: &str,
) -> Result<(), String> {
    let request = check_request(provider, base_url, model, key)?;
    let mut config = format!("url = {}\nrequest = \"POST\"\n", quoted(&request.url));
    for (name, value) in &request.headers {
        config += &format!("header = {}\n", quoted(&format!("{name}: {value}")));
    }
    config += &format!("data = {}\n", quoted(&request.body));
    let connect = |e: String| tr!("No se pudo conectar: {e}", e = e);
    let mut child = Command::new(crate::updates::curl_program())
        .args(["-sS", "-m", "20", "-w", "\n%{http_code}", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| connect(e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(config.as_bytes())
            .map_err(|e| connect(e.to_string()))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| connect(e.to_string()))?;
    if !out.status.success() {
        return Err(connect(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    match split_status(&text) {
        Some((body, status)) => explain(status, body),
        None => Err(connect(tr!("respuesta inesperada").to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header<'a>(r: &'a CheckRequest, name: &str) -> Option<&'a str> {
        r.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn each_provider_is_asked_at_its_own_address_with_the_key_in_a_header() {
        let a = check_request("anthropic", None, None, "sk-ant-x").unwrap();
        assert_eq!(a.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(header(&a, "x-api-key"), Some("sk-ant-x"));
        assert_eq!(header(&a, "anthropic-version"), Some("2023-06-01"));
        assert!(a.body.contains("\"max_tokens\":1"), "{}", a.body);
        assert!(a.body.contains("claude-haiku-5-5"), "{}", a.body);

        let d = check_request("deepseek", None, None, "sk-ds").unwrap();
        assert_eq!(d.url, "https://api.deepseek.com/anthropic/v1/messages");
        assert_eq!(header(&d, "x-api-key"), Some("sk-ds"));
        assert_eq!(header(&d, "authorization"), Some("Bearer sk-ds"));
        assert!(d.body.contains("deepseek-flash"), "{}", d.body);

        let c = check_request(
            "custom",
            Some("https://proxy.example/api/"),
            Some("mi-modelo"),
            "k",
        )
        .unwrap();
        assert_eq!(c.url, "https://proxy.example/api/v1/messages");
        assert!(c.body.contains("mi-modelo"), "{}", c.body);
        // La clave nunca va en la dirección: se vería en la lista de procesos.
        for r in [&a, &d, &c] {
            assert!(!r.url.contains("sk-") && !r.url.contains("=k"), "{}", r.url);
        }
        assert!(check_request("custom", None, Some("m"), "k").is_err());
        assert!(check_request("otro", None, None, "k").is_err());
    }

    #[test]
    fn the_answer_is_explained_in_plain_words() {
        assert!(explain(200, "").is_ok());
        // Un límite de uso quiere decir que la clave es buena.
        assert!(explain(429, "").is_ok());
        let bad = explain(401, "{\"error\":\"x\"}").unwrap_err();
        assert!(bad.contains("clave"), "{bad}");
        assert!(explain(403, "").unwrap_err().contains("clave"));
        let missing = explain(404, "").unwrap_err();
        assert!(
            missing.contains("dirección") && missing.contains("modelo"),
            "{missing}"
        );
        let other = explain(500, "boom").unwrap_err();
        assert!(other.contains("500"), "{other}");
    }

    #[test]
    fn the_curl_output_is_split_into_body_and_status() {
        assert_eq!(split_status("{\"a\":1}\n200"), Some(("{\"a\":1}", 200)));
        assert_eq!(split_status("\n401"), Some(("", 401)));
        assert_eq!(split_status("sin estado"), None);
    }

    /// Con un servicio de verdad (local): curl llega, la clave y el cuerpo van como deben y
    /// la respuesta se explica.
    #[test]
    fn a_real_request_reaches_the_service_with_the_key_and_the_answer_is_explained() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let serve = |status: &'static str| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let seen = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).unwrap();
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                );
                socket.write_all(reply.as_bytes()).unwrap();
                String::from_utf8_lossy(&buf[..n]).to_string()
            });
            (url, seen)
        };
        let (url, seen) = serve("200 OK");
        check("custom", Some(&url), Some("mi-modelo"), "sk-local").unwrap();
        let request = seen.join().unwrap().to_lowercase();
        assert!(request.starts_with("post /v1/messages"), "{request}");
        assert!(request.contains("x-api-key: sk-local"), "{request}");
        assert!(
            request.contains("authorization: bearer sk-local"),
            "{request}"
        );
        assert!(request.contains("\"model\":\"mi-modelo\""), "{request}");

        let (url, seen) = serve("401 Unauthorized");
        let err = check("custom", Some(&url), Some("m"), "sk-mala").unwrap_err();
        assert!(err.contains("clave"), "{err}");
        let _ = seen.join();

        // Nadie escucha: se dice que no se pudo conectar.
        let err = check("custom", Some("http://127.0.0.1:9"), Some("m"), "k").unwrap_err();
        assert!(err.contains("conectar"), "{err}");
    }
}
