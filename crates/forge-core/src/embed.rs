// Búsqueda por significado en Forge Memory (plan #53, fase 3): un modelo multilingüe local y
// opcional. Se descarga al activarlo en Ajustes; sin él, Forge busca solo por palabras.
// Medido en la memoria #68: paraphrase-multilingual-MiniLM-L12-v2, pesos en f16 (cálculo en
// f32: en f16 la CPU se desborda), umbral 0,30.
use std::io::Read;
use std::path::{Path, PathBuf};

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};

use crate::tr;

/// Nombre del modelo que va con cada vector: si cambia el modelo, los vectores viejos no valen.
pub const MODEL: &str = "minilm-v1";

/// Coseno mínimo para que una memoria venga al caso (medido: 25 de 30 con ruido en 1 de 6).
pub const THRESHOLD: f32 = 0.30;

const BASE_URL: &str = "https://github.com/DiegoAndres717/forge/releases/download/model-minilm-v1";

/// Archivo publicado, huella SHA-256 y tamaño en bytes (para el progreso).
pub const FILES: [(&str, &str, u64); 3] = [
    (
        "minilm-f16.safetensors",
        "1ef46288107e74993283511d00f127168b4723ba7d8c93859386b709f12b730d",
        235_330_976,
    ),
    (
        "minilm-tokenizer.json",
        "2c3387be76557bd40970cec13153b3bbf80407865484b209e655e5e4729076b8",
        9_081_518,
    ),
    (
        "minilm-config.json",
        "6300193cb75e01cf80c96decef7187dfb33094d97cc1490b7ead6ff134476e4e",
        645,
    ),
];

/// Carpeta del modelo, junto a la base de datos (con `FORGE_DB`, junto a esa).
pub fn default_dir() -> Option<PathBuf> {
    Some(
        crate::store::Store::default_path()?
            .parent()?
            .join("models")
            .join(MODEL),
    )
}

pub fn installed(dir: &Path) -> bool {
    FILES.iter().all(|(name, _, _)| dir.join(name).is_file())
}

/// Bytes descargados hasta ahora y el total (para mostrar el progreso desde otro hilo).
pub fn progress(dir: &Path) -> (u64, u64) {
    let total = FILES.iter().map(|(_, _, size)| size).sum();
    let done = FILES
        .iter()
        .map(|(name, _, size)| {
            let file = |p: PathBuf| std::fs::metadata(p).map_or(0, |m| m.len());
            file(dir.join(name)).max(file(part(dir, name))).min(*size)
        })
        .sum();
    (done, total)
}

fn part(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.part"))
}

/// Descarga el modelo publicado y comprueba cada archivo antes de darlo por bueno.
pub fn install(dir: &Path) -> Result<(), String> {
    install_from(BASE_URL, dir, &FILES)
}

pub(crate) fn install_from(
    base: &str,
    dir: &Path,
    files: &[(&str, &str, u64)],
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (name, sha, _) in files {
        let target = dir.join(name);
        if target.is_file() {
            continue;
        }
        let tmp = part(dir, name);
        let out = std::process::Command::new(crate::updates::curl_program())
            .args(["-fsSL", "-m", "1800", "-o"])
            .arg(&tmp)
            .arg(format!("{base}/{name}"))
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(tr!(
                "no se pudo descargar {name}: {p0}",
                name = name,
                p0 = String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        if sha256(&tmp)? != *sha {
            let _ = std::fs::remove_file(&tmp);
            return Err(
                tr!("la huella SHA-256 de la descarga no coincide con la publicada").into(),
            );
        }
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Quita el modelo (libera el disco); la búsqueda vuelve a ser solo por palabras.
pub fn remove(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

fn sha256(file: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(crate::guard::diff::hex(&hasher.finalize()))
}

/// El modelo cargado (≈0,6 s y ≈450 MB de RAM: se carga cuando hace falta y se suelta).
pub struct Embedder {
    model: BertModel,
    tokenizer: tokenizers::Tokenizer,
}

impl Embedder {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let err = |e: &dyn std::fmt::Display| e.to_string();
        let config: Config = serde_json::from_str(
            &std::fs::read_to_string(dir.join("minilm-config.json")).map_err(|e| err(&e))?,
        )
        .map_err(|e| err(&e))?;
        let tokenizer = tokenizers::Tokenizer::from_file(dir.join("minilm-tokenizer.json"))
            .map_err(|e| err(&e))?;
        // SAFETY: el archivo es nuestro (comprobado con su huella) y no cambia mientras se usa.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(
                &[dir.join("minilm-f16.safetensors")],
                DType::F32,
                &Device::Cpu,
            )
        }
        .map_err(|e| err(&e))?;
        let model = BertModel::load(vb, &config).map_err(|e| err(&e))?;
        Ok(Self { model, tokenizer })
    }

    /// Vector normalizado del texto (media de los tokens; como mucho 128).
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let err = |e: &dyn std::fmt::Display| e.to_string();
        let enc = self.tokenizer.encode(text, true).map_err(|e| err(&e))?;
        let ids: Vec<u32> = enc.get_ids().iter().copied().take(128).collect();
        if ids.is_empty() {
            return Err(tr!("texto vacío").into());
        }
        let n = ids.len();
        let input = Tensor::new(ids.as_slice(), &Device::Cpu)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| err(&e))?;
        let types = input.zeros_like().map_err(|e| err(&e))?;
        let mask = Tensor::ones((1, n), DType::U32, &Device::Cpu).map_err(|e| err(&e))?;
        let out = self
            .model
            .forward(&input, &types, Some(&mask))
            .map_err(|e| err(&e))?;
        let mean: Vec<f32> = (out.sum(1).map_err(|e| err(&e))? / n as f64)
            .and_then(|t| t.squeeze(0))
            .and_then(|t| t.to_vec1())
            .map_err(|e| err(&e))?;
        Ok(normalize(mean))
    }
}

/// Vector de `text` si el modelo carga y responde antes de `timeout`; si no, nada (y quien
/// llama busca solo por palabras). El hilo que se pasa de tiempo muere con el proceso.
pub fn embed_within(dir: &Path, text: &str, timeout: std::time::Duration) -> Option<Vec<f32>> {
    if !installed(dir) {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (dir, text) = (dir.to_path_buf(), text.to_string());
    std::thread::spawn(move || {
        let _ = tx.send(Embedder::load(&dir).and_then(|e| e.embed(&text)));
    });
    rx.recv_timeout(timeout).ok()?.ok()
}

/// Calcula hasta `limit` vectores que faltan (memorias nuevas o cambiadas). Devuelve cuántos.
pub fn embed_pending(
    store: &crate::store::Store,
    dir: &Path,
    limit: usize,
) -> Result<usize, String> {
    let pending = store.pending_vectors(MODEL, limit)?;
    if pending.is_empty() {
        return Ok(0);
    }
    let embedder = Embedder::load(dir)?;
    for m in &pending {
        store.set_memory_vector(m.id, MODEL, &embedder.embed(&memory_text(m))?)?;
    }
    Ok(pending.len())
}

fn normalize(v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return v;
    }
    v.into_iter().map(|x| x / norm).collect()
}

/// Parecido entre dos vectores normalizados (1 = lo mismo).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// El texto de una memoria que se convierte en vector: título, comienzo del texto y etiquetas.
pub fn memory_text(m: &crate::memory::Memory) -> String {
    let body: String = m.body.chars().take(300).collect();
    format!("{}. {} {}", m.title, body, m.tags)
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_are_checked_before_they_count() {
        let tmp = std::env::temp_dir().join(format!("forge-embed-{}", std::process::id()));
        let (src, dir) = (tmp.join("src"), tmp.join("models"));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.bin"), b"hola").unwrap();
        // En Windows, file:///C:/… (con barras normales).
        let path = src.display().to_string().replace('\\', "/");
        let base = format!(
            "file://{}{path}",
            if path.starts_with('/') { "" } else { "/" }
        );
        let good = "b221d9dbb083a7f33428d7c2a3c3198ae925614d70210e28716ccaa7cd4ddb79";
        // Huella equivocada: error y nada a medias en la carpeta.
        let bad = [("a.bin", "00", 4)];
        assert!(install_from(&base, &dir, &bad).is_err());
        assert!(!dir.join("a.bin").exists() && !dir.join("a.bin.part").exists());
        install_from(&base, &dir, &[("a.bin", good, 4)]).unwrap();
        assert_eq!(std::fs::read(dir.join("a.bin")).unwrap(), b"hola");
        remove(&dir).unwrap();
        remove(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn vectors_round_trip_and_compare() {
        let v = normalize(vec![3.0, 4.0]);
        assert_eq!(from_blob(&to_blob(&v)), v);
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
        assert!(cosine(&v, &normalize(vec![-4.0, 3.0])).abs() < 1e-6);
    }

    /// Con el modelo instalado de verdad (no en CI): una frase en español encuentra la nota en
    /// inglés que trata de lo mismo.
    #[test]
    #[ignore]
    fn spanish_finds_the_english_note_with_the_real_model() {
        let dir = std::env::var_os("FORGE_MODEL_DIR")
            .map(PathBuf::from)
            .or_else(default_dir)
            .unwrap();
        let e = Embedder::load(&dir).unwrap();
        let q = e
            .embed("la prueba falla solo en windows por los saltos de línea")
            .unwrap();
        let near = e
            .embed("Tests that include_str! another file fail on Windows (CRLF checkout)")
            .unwrap();
        let far = e
            .embed("Alert sounds play through afplay; notifications are silent")
            .unwrap();
        assert!(cosine(&q, &near) > THRESHOLD && cosine(&q, &near) > cosine(&q, &far) + 0.1);
    }
}
