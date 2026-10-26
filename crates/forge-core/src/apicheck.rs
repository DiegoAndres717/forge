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
        "kimi" => (
            "https://api.moonshot.ai/anthropic".to_string(),
            given(model).unwrap_or("kimi-k3").to_string(),
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

/// La petición de la lista de modelos de un proveedor (sin cuerpo). Anthropic pagina: la
/// siguiente página se pide con `after`. DeepSeek y Kimi la tienen en su dirección de
/// siempre, no bajo `/anthropic`; en una dirección propia se prueba `<dirección>/v1/models`.
pub fn list_request(
    provider: &str,
    base_url: Option<&str>,
    key: &str,
    after: Option<&str>,
) -> Result<CheckRequest, String> {
    let key = key.trim();
    let bearer = ("authorization".to_string(), format!("Bearer {key}"));
    let anthropic = [
        ("x-api-key".to_string(), key.to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
    ];
    let (url, headers) = match provider {
        "anthropic" => {
            let mut url = "https://api.anthropic.com/v1/models?limit=1000".to_string();
            if let Some(after) = after {
                url += &format!("&after_id={after}");
            }
            (url, anthropic.to_vec())
        }
        "deepseek" => ("https://api.deepseek.com/models".to_string(), vec![bearer]),
        "kimi" => (
            "https://api.moonshot.ai/v1/models".to_string(),
            vec![bearer],
        ),
        "custom" => {
            let base = given(base_url).ok_or_else(|| tr!("Falta la dirección.").to_string())?;
            let mut url = format!("{}/v1/models", base.trim_end_matches('/'));
            if let Some(after) = after {
                url += &format!("?after_id={after}");
            }
            let mut headers = anthropic.to_vec();
            headers.push(bearer);
            (url, headers)
        }
        other => return Err(tr!("Proveedor desconocido: {p}", p = other)),
    };
    Ok(CheckRequest {
        url,
        headers,
        body: String::new(),
    })
}

/// Los `id` de una página de la lista y, si hay más, desde dónde seguir. `None` si no es una
/// lista de modelos.
pub fn parse_models(body: &str) -> Option<(Vec<String>, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let ids = v["data"]
        .as_array()?
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();
    let next = (v["has_more"] == true)
        .then(|| v["last_id"].as_str().map(String::from))
        .flatten();
    Some((ids, next))
}

/// Pide la lista de modelos que la propia API ofrece, página a página. La clave viaja como en
/// la prueba, por la entrada de curl.
pub fn list_models(
    provider: &str,
    base_url: Option<&str>,
    key: &str,
) -> Result<Vec<String>, String> {
    let mut models: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    // Un tope por si un servicio repitiera la última página sin fin.
    for _ in 0..20 {
        let request = list_request(provider, base_url, key, after.as_deref())?;
        let (body, status) = send("GET", &request)?;
        if status == 429 {
            return Err(tr!("El servicio pide esperar un momento: prueba otra vez.").to_string());
        }
        explain(status, &body)?;
        let (ids, next) = parse_models(&body)
            .ok_or_else(|| tr!("El servicio no devolvió una lista de modelos.").to_string())?;
        for id in ids {
            if !models.contains(&id) {
                models.push(id);
            }
        }
        match next {
            Some(next) if after.as_deref() != Some(next.as_str()) => after = Some(next),
            _ => break,
        }
    }
    Ok(models)
}

/// Lo que respondió el servicio, en palabras claras. Un límite de uso (429) quiere decir
/// que la clave es buena.
pub fn explain(status: u16, body: &str) -> Result<(), String> {
    match status {
        200..=299 | 429 => Ok(()),
        401 | 403 => Err(tr!("La clave no es válida o no tiene permiso.").to_string()),
        404 => Err(tr!("No se encontró la dirección o el modelo. Revisa los dos.").to_string()),
        other => {
            // Si es el JSON de error del servicio, se muestra solo su mensaje, entero.
            let text = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| body.trim().to_string());
            let detail: String = text.chars().take(400).collect();
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

/// Hace una petición con el curl del sistema y devuelve el cuerpo y el código. La clave viaja
/// por la entrada de curl, no por sus argumentos: así no aparece en la lista de procesos.
fn send(method: &str, request: &CheckRequest) -> Result<(String, u16), String> {
    let mut config = format!(
        "url = {}\nrequest = {}\n",
        quoted(&request.url),
        quoted(method)
    );
    for (name, value) in &request.headers {
        config += &format!("header = {}\n", quoted(&format!("{name}: {value}")));
    }
    if !request.body.is_empty() {
        config += &format!("data = {}\n", quoted(&request.body));
    }
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
        Some((body, status)) => Ok((body.to_string(), status)),
        None => Err(connect(tr!("respuesta inesperada").to_string())),
    }
}

/// Hace la prueba de la clave: una petición mínima y su respuesta explicada.
pub fn check(
    provider: &str,
    base_url: Option<&str>,
    model: Option<&str>,
    key: &str,
) -> Result<(), String> {
    let request = check_request(provider, base_url, model, key)?;
    let (body, status) = send("POST", &request)?;
    explain(status, &body)
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
        let k = check_request("kimi", None, None, "sk-kimi").unwrap();
        assert_eq!(k.url, "https://api.moonshot.ai/anthropic/v1/messages");
        assert!(
            k.headers
                .iter()
                .any(|(n, v)| n == "authorization" && v == "Bearer sk-kimi")
        );
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
    fn a_long_service_message_is_shown_whole_without_the_json_around_it() {
        let message = "This API key is not scoped to a workspace, so this request must include the anthropic-workspace-id header with the workspace it acts in.";
        let body = format!(
            "{{\"type\":\"error\",\"error\":{{\"type\":\"invalid_request_error\",\"message\":\"{message}\"}}}}"
        );
        let shown = explain(400, &body).unwrap_err();
        assert!(shown.contains(message), "{shown}");
        assert!(!shown.contains("invalid_request_error"), "{shown}");
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

    #[test]
    fn the_model_list_is_asked_at_each_providers_own_address() {
        let a = list_request("anthropic", None, "sk-ant-x", None).unwrap();
        assert_eq!(a.url, "https://api.anthropic.com/v1/models?limit=1000");
        assert_eq!(header(&a, "x-api-key"), Some("sk-ant-x"));
        assert_eq!(header(&a, "anthropic-version"), Some("2023-06-01"));
        let next = list_request("anthropic", None, "k", Some("claude-b")).unwrap();
        assert!(next.url.ends_with("&after_id=claude-b"), "{}", next.url);
        // DeepSeek y Kimi: la lista está en su dirección de siempre, no bajo /anthropic.
        let d = list_request("deepseek", None, "sk-ds", None).unwrap();
        assert_eq!(d.url, "https://api.deepseek.com/models");
        assert_eq!(header(&d, "authorization"), Some("Bearer sk-ds"));
        let k = list_request("kimi", None, "sk-kimi", None).unwrap();
        assert_eq!(k.url, "https://api.moonshot.ai/v1/models");
        assert_eq!(header(&k, "authorization"), Some("Bearer sk-kimi"));
        let c = list_request("custom", Some("https://x.example/anthropic/"), "k", None).unwrap();
        assert_eq!(c.url, "https://x.example/anthropic/v1/models");
        assert!(list_request("custom", None, "k", None).is_err());
        assert!(list_request("otro", None, "k", None).is_err());
    }

    #[test]
    fn the_ids_and_the_next_page_are_read_from_the_answer() {
        let page = r#"{"data":[{"id":"claude-a","display_name":"A"},{"id":"claude-b"}],"has_more":true,"last_id":"claude-b"}"#;
        assert_eq!(
            parse_models(page),
            Some((
                vec!["claude-a".into(), "claude-b".into()],
                Some("claude-b".into())
            ))
        );
        // Estilo OpenAI (DeepSeek, Kimi): sin paginación.
        let flat = r#"{"object":"list","data":[{"id":"deepseek-v4-pro","object":"model"}]}"#;
        assert_eq!(
            parse_models(flat),
            Some((vec!["deepseek-v4-pro".into()], None))
        );
        assert_eq!(parse_models("no es json"), None);
        assert_eq!(parse_models(r#"{"error":"x"}"#), None);
    }

    /// Con un servicio de verdad (local): las dos páginas se juntan y un error se explica.
    #[test]
    fn the_model_list_follows_the_pages_of_a_real_service() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).unwrap();
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if request.contains("after_id=m2") {
                    r#"{"data":[{"id":"m3"}],"has_more":false}"#
                } else {
                    r#"{"data":[{"id":"m1"},{"id":"m2"}],"has_more":true,"last_id":"m2"}"#
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).unwrap();
                asked.push(request.lines().next().unwrap_or_default().to_string());
            }
            asked
        });
        let models = list_models("custom", Some(&url), "sk-local").unwrap();
        assert_eq!(models, ["m1", "m2", "m3"]);
        let asked = server.join().unwrap();
        assert!(asked[1].contains("after_id=m2"), "{asked:?}");

        // Una clave mala se explica con palabras claras.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).unwrap();
            let reply =
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
            socket.write_all(reply.as_bytes()).unwrap();
        });
        let err = list_models("custom", Some(&url), "sk-mala").unwrap_err();
        assert!(err.contains("clave"), "{err}");
        let _ = server.join();
    }
}
