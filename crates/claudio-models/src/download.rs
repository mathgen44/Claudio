//! Téléchargement de gros fichiers : reprise après interruption, vérification
//! SHA-256, suivi de progression et annulation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{StatusCode, header};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: u64,
}

/// Jeton d'annulation partagé entre l'interface et la tâche de téléchargement.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

pub struct Request<'a> {
    pub url: &'a str,
    pub dest: &'a Path,
    pub bearer: Option<&'a str>,
    /// Empreinte attendue en hexadécimal ; le fichier est rejeté s'il ne correspond pas.
    pub sha256: Option<&'a str>,
    pub expected_size: Option<u64>,
}

pub fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Claudio/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("client HTTP")
}

/// Télécharge `req.url` vers `req.dest`. Un fichier `.part` existant est repris.
pub async fn download(
    http: &reqwest::Client,
    req: Request<'_>,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&Progress),
) -> Result<()> {
    if let Some(dir) = req.dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let part = partial_path(req.dest);
    let mut hasher = Sha256::new();
    let mut offset = match tokio::fs::metadata(&part).await {
        Ok(m) => m.len(),
        Err(_) => 0,
    };
    if req.expected_size.is_some_and(|s| offset > s) {
        offset = 0;
    }
    if offset > 0 {
        // Le hachage doit couvrir tout le fichier : on relit la partie déjà présente.
        hash_file_prefix(&part, offset, &mut hasher).await?;
    }

    let mut builder = http.get(req.url);
    if let Some(token) = req.bearer {
        builder = builder.bearer_auth(token);
    }
    if offset > 0 {
        builder = builder.header(header::RANGE, format!("bytes={offset}-"));
    }
    let resp = builder
        .send()
        .await
        .with_context(|| format!("connexion à {}", req.url))?;
    let status = resp.status();
    let mut file = if offset > 0 && status == StatusCode::PARTIAL_CONTENT {
        tracing::info!(offset, "reprise du téléchargement");
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await?
    } else if status.is_success() {
        offset = 0;
        hasher = Sha256::new();
        tokio::fs::File::create(&part).await?
    } else if status == StatusCode::RANGE_NOT_SATISFIABLE && Some(offset) == req.expected_size {
        // Tout est déjà téléchargé : il ne reste qu'à vérifier.
        return finish(&part, req.dest, hasher, req.sha256).await;
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        bail!("accès refusé ({status}) : ce modèle demande peut-être un jeton Hugging Face");
    } else {
        bail!("téléchargement refusé par le serveur : {status}");
    };

    let total = resp
        .content_length()
        .map(|l| l + offset)
        .or(req.expected_size);
    let mut downloaded = offset;
    let started = Instant::now();
    let mut last_report = Instant::now() - Duration::from_secs(1);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            file.flush().await?;
            bail!("téléchargement annulé");
        }
        let chunk = chunk.context("connexion interrompue")?;
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        if last_report.elapsed() >= Duration::from_millis(250) {
            last_report = Instant::now();
            let secs = started.elapsed().as_secs_f64().max(0.001);
            on_progress(&Progress {
                downloaded,
                total,
                bytes_per_sec: ((downloaded - offset) as f64 / secs) as u64,
            });
        }
    }
    file.flush().await?;
    drop(file);
    if let Some(t) = total
        && downloaded != t
    {
        bail!("téléchargement incomplet : {downloaded} octets sur {t}");
    }
    on_progress(&Progress {
        downloaded,
        total,
        bytes_per_sec: 0,
    });
    finish(&part, req.dest, hasher, req.sha256).await
}

async fn hash_file_prefix(path: &Path, len: u64, hasher: &mut Sha256) -> Result<()> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; 1 << 20];
    let mut left = len;
    while left > 0 {
        let want = buf.len().min(left as usize);
        let n = f.read(&mut buf[..want]).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(())
}

async fn finish(part: &Path, dest: &Path, hasher: Sha256, expected: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        let got = hex::encode(hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            tokio::fs::remove_file(part).await.ok();
            bail!(
                "empreinte SHA-256 incorrecte (attendu {expected}, obtenu {got}) : fichier supprimé"
            );
        }
    }
    tokio::fs::rename(part, dest)
        .await
        .with_context(|| format!("installation de {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_name() {
        assert_eq!(
            partial_path(Path::new("C:/m/a.gguf")),
            PathBuf::from("C:/m/a.gguf.part")
        );
    }
}
