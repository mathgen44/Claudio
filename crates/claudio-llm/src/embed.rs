//! Calcul d'embeddings via l'API compatible OpenAI (`/embeddings`) d'un
//! llama-server lancé en mode `--embedding` (ou d'Ollama).

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::chat::{Endpoint, http_error};

/// Normalise un vecteur (norme 1) : le produit scalaire devient la similarité cosinus.
pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Embeddings normalisés des textes, dans l'ordre.
pub async fn embed(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let resp = http
        .post(format!(
            "{}/embeddings",
            endpoint.base_url.trim_end_matches('/')
        ))
        .json(&json!({"model": endpoint.model, "input": inputs}))
        .send()
        .await
        .context("le serveur d'embeddings ne répond pas")?;
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }
    let v: Value = resp.json().await?;
    let mut items: Vec<(usize, Vec<f32>)> = v["data"]
        .as_array()
        .context("réponse d'embeddings illisible")?
        .iter()
        .map(|d| {
            let idx = d["index"].as_u64().unwrap_or(0) as usize;
            let mut e: Vec<f32> = d["embedding"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_f64)
                        .map(|x| x as f32)
                        .collect()
                })
                .unwrap_or_default();
            normalize(&mut e);
            (idx, e)
        })
        .collect();
    if items.len() != inputs.len() || items.iter().any(|(_, e)| e.is_empty()) {
        bail!(
            "le serveur a renvoyé {} vecteurs pour {} textes",
            items.len(),
            inputs.len()
        );
    }
    items.sort_by_key(|(i, _)| *i);
    Ok(items.into_iter().map(|(_, e)| e).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        let mut v = vec![3.0, 4.0];
        normalize(&mut v);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        let mut z = vec![0.0, 0.0];
        normalize(&mut z);
        assert_eq!(z, vec![0.0, 0.0]);
    }
}
