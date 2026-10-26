//! Accès à l'API Hugging Face Hub : recherche, contenu des dépôts, fichiers GGUF.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const HUB: &str = "https://huggingface.co";

#[derive(Debug, Clone, Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: u64,
    lfs: Option<Lfs>,
}

#[derive(Debug, Clone, Deserialize)]
struct Lfs {
    oid: String,
}

/// Fichier d'un dépôt, avec son empreinte SHA-256 quand il est stocké en LFS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
}

/// Résultat de recherche.
#[derive(Debug, Clone, Serialize)]
pub struct ModelSummary {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub license: Option<String>,
    pub pipeline_tag: Option<String>,
    pub updated: Option<String>,
}

/// Une variante GGUF d'un dépôt : un fichier, ou plusieurs s'il est découpé.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GgufVariant {
    /// Quantification devinée depuis le nom (ex. `Q4_K_M`), ou le nom du fichier.
    pub quant: String,
    pub files: Vec<RemoteFile>,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoInfo {
    pub id: String,
    pub license: Option<String>,
    /// Dépôt à accès restreint : il faut accepter ses conditions sur le site et un jeton.
    pub gated: bool,
    pub downloads: u64,
    pub likes: u64,
    /// Nombre total de paramètres, d'après les métadonnées GGUF du dépôt.
    pub params_total: Option<u64>,
    pub architecture: Option<String>,
    pub context_length: Option<u32>,
    pub variants: Vec<GgufVariant>,
}

impl RepoInfo {
    /// Mélange d'experts, d'après l'architecture ou le nom (`…-A3B`).
    pub fn is_moe(&self) -> bool {
        self.architecture
            .as_deref()
            .is_some_and(|a| a.contains("moe"))
            || active_params_from_name(&self.id).is_some()
    }
}

/// Paramètres actifs écrits dans le nom, ex. `Qwen3.6-35B-A3B` → 3.
pub fn active_params_from_name(name: &str) -> Option<f64> {
    name.to_ascii_uppercase()
        .split(['-', '_', '/'])
        .find_map(|seg| {
            let n = seg.strip_prefix('A')?.strip_suffix('B')?;
            n.parse::<f64>().ok().filter(|v| *v > 0.0 && *v < 100.0)
        })
}

pub fn resolve_url(repo: &str, file: &str) -> String {
    format!("{HUB}/{repo}/resolve/main/{file}")
}

pub fn repo_url(repo: &str) -> String {
    format!("{HUB}/{repo}")
}

fn get(http: &reqwest::Client, url: &str, token: Option<&str>) -> reqwest::RequestBuilder {
    let req = http.get(url);
    match token {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    token: Option<&str>,
) -> Result<T> {
    let resp = get(http, url, token)
        .send()
        .await
        .context("connexion à Hugging Face")?;
    match resp.status() {
        s if s.is_success() => Ok(resp.json().await?),
        s if s.as_u16() == 401 || s.as_u16() == 403 => {
            bail!("accès refusé par Hugging Face ({s}) : dépôt privé ou restreint")
        }
        s if s.as_u16() == 404 => bail!("dépôt introuvable sur Hugging Face"),
        s => bail!("Hugging Face a répondu {s}"),
    }
}

fn valid_repo(repo: &str) -> Result<()> {
    let ok = repo.split('/').count() == 2
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
        && !repo.contains("..");
    if !ok {
        bail!("nom de dépôt invalide : {repo}");
    }
    Ok(())
}

async fn tree(http: &reqwest::Client, repo: &str, token: Option<&str>) -> Result<Vec<TreeEntry>> {
    valid_repo(repo)?;
    get_json(
        http,
        &format!("{HUB}/api/models/{repo}/tree/main?recursive=true"),
        token,
    )
    .await
}

/// Décrit un fichier d'un dépôt (taille et empreinte).
pub async fn file_info(
    http: &reqwest::Client,
    repo: &str,
    file: &str,
    token: Option<&str>,
) -> Result<RemoteFile> {
    let e = tree(http, repo, token)
        .await?
        .into_iter()
        .find(|e| e.path == file)
        .with_context(|| format!("{file} introuvable dans {repo}"))?;
    Ok(RemoteFile {
        path: e.path,
        size: e.size,
        sha256: e.lfs.map(|l| l.oid),
    })
}

fn license_from_tags(tags: &[Value]) -> Option<String> {
    tags.iter()
        .filter_map(Value::as_str)
        .find_map(|t| t.strip_prefix("license:"))
        .map(str::to_owned)
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
            ("limit", "40"),
            ("full", "false"),
        ],
    )?;
    let raw: Vec<Value> = get_json(http, url.as_str(), token).await?;
    Ok(raw
        .into_iter()
        .filter_map(|m| {
            let tags = m
                .get("tags")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Some(ModelSummary {
                id: m.get("id")?.as_str()?.to_owned(),
                downloads: m.get("downloads").and_then(Value::as_u64).unwrap_or(0),
                likes: m.get("likes").and_then(Value::as_u64).unwrap_or(0),
                license: license_from_tags(&tags),
                pipeline_tag: m
                    .get("pipeline_tag")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                updated: m
                    .get("lastModified")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect())
}

/// Informations d'un dépôt et ses variantes GGUF, de la plus légère à la plus lourde.
pub async fn repo_info(
    http: &reqwest::Client,
    repo: &str,
    token: Option<&str>,
) -> Result<RepoInfo> {
    valid_repo(repo)?;
    let info: Value = get_json(http, &format!("{HUB}/api/models/{repo}"), token).await?;
    let tags = info
        .get("tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let license = info
        .pointer("/cardData/license")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| license_from_tags(&tags));
    let gated = !matches!(
        info.get("gated"),
        None | Some(Value::Bool(false)) | Some(Value::Null)
    );
    let files = tree(http, repo, token)
        .await?
        .into_iter()
        .filter(|e| e.kind == "file")
        .map(|e| RemoteFile {
            path: e.path,
            size: e.size,
            sha256: e.lfs.map(|l| l.oid),
        })
        .collect();
    Ok(RepoInfo {
        id: repo.to_owned(),
        license,
        gated,
        downloads: info.get("downloads").and_then(Value::as_u64).unwrap_or(0),
        likes: info.get("likes").and_then(Value::as_u64).unwrap_or(0),
        params_total: info.pointer("/gguf/total").and_then(Value::as_u64),
        architecture: info
            .pointer("/gguf/architecture")
            .and_then(Value::as_str)
            .map(str::to_owned),
        context_length: info
            .pointer("/gguf/context_length")
            .and_then(Value::as_u64)
            .map(|c| c.min(u32::MAX as u64) as u32),
        variants: group_gguf(files),
    })
}

/// Sépare `nom-00001-of-00003` en (`nom`, 1).
fn split_part(stem: &str) -> Option<(&str, u32)> {
    let (head, total) = stem.rsplit_once("-of-")?;
    let (base, idx) = head.rsplit_once('-')?;
    (total.len() == 5 && idx.len() == 5 && total.chars().all(|c| c.is_ascii_digit()))
        .then(|| (base, idx.parse().ok()))
        .and_then(|(b, i)| Some((b, i?)))
}

/// Quantification devinée depuis un nom de fichier GGUF.
pub fn guess_quant(file: &str) -> String {
    let name = file.rsplit('/').next().unwrap_or(file);
    let stem = name.strip_suffix(".gguf").unwrap_or(name);
    let stem = split_part(stem).map(|(b, _)| b).unwrap_or(stem);
    let upper = stem.to_ascii_uppercase();
    let looks_like_quant = |s: &str| {
        ["Q", "IQ", "F16", "BF16", "F32", "MXFP", "TQ"]
            .iter()
            .any(|p| s.starts_with(p))
            && s.chars().any(|c| c.is_ascii_digit())
    };
    // Cherche le dernier segment qui ressemble à une quantification, avec le préfixe UD- éventuel.
    let parts: Vec<&str> = upper.split(['-', '.']).collect();
    for i in (0..parts.len()).rev() {
        let mut q = parts[i..].join("-");
        if looks_like_quant(parts[i]) {
            if i > 0 && parts[i - 1] == "UD" {
                q = format!("UD-{q}");
            }
            return q;
        }
    }
    stem.to_owned()
}

/// Regroupe les fichiers GGUF par variante (fichiers découpés réunis), sans les
/// projecteurs de vision (`mmproj`).
pub fn group_gguf(files: Vec<RemoteFile>) -> Vec<GgufVariant> {
    let mut groups: BTreeMap<String, Vec<RemoteFile>> = BTreeMap::new();
    for f in files {
        let lower = f.path.to_ascii_lowercase();
        let name = lower.rsplit('/').next().unwrap_or(&lower);
        if !name.ends_with(".gguf") || name.starts_with("mmproj") {
            continue;
        }
        let stem = f.path.strip_suffix(".gguf").unwrap_or(&f.path);
        let key = split_part(stem)
            .map(|(b, _)| b.to_owned())
            .unwrap_or_else(|| stem.to_owned());
        groups.entry(key).or_default().push(f);
    }
    let mut out: Vec<GgufVariant> = groups
        .into_values()
        .map(|mut files| {
            files.sort_by(|a, b| a.path.cmp(&b.path));
            GgufVariant {
                quant: guess_quant(&files[0].path),
                size: files.iter().map(|f| f.size).sum(),
                files,
            }
        })
        .collect();
    out.sort_by_key(|v| v.size);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, size: u64) -> RemoteFile {
        RemoteFile {
            path: path.into(),
            size,
            sha256: None,
        }
    }

    #[test]
    fn quants() {
        assert_eq!(guess_quant("Qwen3.5-4B-Q4_K_M.gguf"), "Q4_K_M");
        assert_eq!(
            guess_quant("gemma-4-26B-A4B-it-UD-Q4_K_XL.gguf"),
            "UD-Q4_K_XL"
        );
        assert_eq!(guess_quant("model.Q8_0.gguf"), "Q8_0");
        assert_eq!(guess_quant("BF16/model-BF16-00001-of-00002.gguf"), "BF16");
        assert_eq!(guess_quant("gpt-oss-20b-MXFP4.gguf"), "MXFP4");
        assert_eq!(guess_quant("weird.gguf"), "weird");
    }

    #[test]
    fn groups_split_files_and_skips_mmproj() {
        let v = group_gguf(vec![
            f("Q8_0/m-Q8_0-00002-of-00002.gguf", 5),
            f("Q8_0/m-Q8_0-00001-of-00002.gguf", 10),
            f("m-Q4_K_M.gguf", 8),
            f("mmproj-F16.gguf", 1),
            f("README.md", 1),
        ]);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].quant, "Q4_K_M");
        assert_eq!(v[1].quant, "Q8_0");
        assert_eq!(v[1].size, 15);
        assert!(v[1].files[0].path.contains("00001"));
    }

    #[test]
    fn active_params() {
        assert_eq!(
            active_params_from_name("unsloth/Qwen3.6-35B-A3B-GGUF"),
            Some(3.0)
        );
        assert_eq!(active_params_from_name("gemma-4-26B-A4B-it"), Some(4.0));
        assert_eq!(active_params_from_name("unsloth/gemma-4-12b-it-GGUF"), None);
    }

    #[test]
    fn rejects_bad_repo_names() {
        assert!(valid_repo("unsloth/Qwen3.5-4B-GGUF").is_ok());
        assert!(valid_repo("../etc").is_err());
        assert!(valid_repo("a/b/c").is_err());
    }
}
