//! Accès à l'API Hugging Face Hub.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const HUB: &str = "https://huggingface.co";

#[derive(Debug, Clone, Deserialize)]
struct TreeEntry {
    path: String,
    size: u64,
    lfs: Option<Lfs>,
}

#[derive(Debug, Clone, Deserialize)]
struct Lfs {
    oid: String,
}

/// Fichier d'un dépôt, avec son empreinte SHA-256 quand il est stocké en LFS.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub id: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
}

pub fn resolve_url(repo: &str, file: &str) -> String {
    format!("{HUB}/{repo}/resolve/main/{file}")
}

fn get(http: &reqwest::Client, url: &str, token: Option<&str>) -> reqwest::RequestBuilder {
    let req = http.get(url);
    match token {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

/// Décrit un fichier d'un dépôt (taille et empreinte).
pub async fn file_info(
    http: &reqwest::Client,
    repo: &str,
    file: &str,
    token: Option<&str>,
) -> Result<RemoteFile> {
    let dir = file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let url = format!("{HUB}/api/models/{repo}/tree/main/{dir}");
    let resp = get(http, &url, token)
        .send()
        .await
        .context("connexion à Hugging Face")?;
    if !resp.status().is_success() {
        bail!("Hugging Face a répondu {} pour {repo}", resp.status());
    }
    let entries: Vec<TreeEntry> = resp.json().await?;
    let e = entries
        .into_iter()
        .find(|e| e.path == file)
        .with_context(|| format!("{file} introuvable dans {repo}"))?;
    Ok(RemoteFile {
        path: e.path,
        size: e.size,
        sha256: e.lfs.map(|l| l.oid),
    })
}

/// Recherche de modèles GGUF, triés par téléchargements.
pub async fn search(
    http: &reqwest::Client,
    query: &str,
    token: Option<&str>,
) -> Result<Vec<ModelSummary>> {
    let url = reqwest::Url::parse_with_params(
        &format!("{HUB}/api/models"),
        &[
            ("search", query),
            ("filter", "gguf"),
            ("sort", "downloads"),
            ("direction", "-1"),
            ("limit", "30"),
        ],
    )?;
    let resp = get(http, url.as_str(), token).send().await?;
    if !resp.status().is_success() {
        bail!("recherche Hugging Face : {}", resp.status());
    }
    Ok(resp.json().await?)
}
