//! Réutilisation d'une instance Ollama déjà installée sur la machine.

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::chat::Endpoint;

pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    pub size: u64,
    #[serde(default)]
    pub details: Details,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Details {
    #[serde(default)]
    pub parameter_size: String,
    #[serde(default)]
    pub quantization_level: String,
}

#[derive(Deserialize)]
struct Tags {
    models: Vec<OllamaModel>,
}

/// Modèles disponibles, ou `None` si Ollama ne tourne pas.
pub async fn list(http: &reqwest::Client, base: &str) -> Option<Vec<OllamaModel>> {
    let resp = http
        .get(format!("{base}/api/tags"))
        .timeout(Duration::from_millis(1500))
        .send()
        .await
        .ok()?;
    let tags: Tags = resp.error_for_status().ok()?.json().await.ok()?;
    Some(tags.models)
}

pub fn endpoint(base: &str, model: &str) -> Endpoint {
    Endpoint {
        base_url: format!("{base}/v1"),
        model: model.to_owned(),
        api_key: None,
    }
}

/// Chemin d'Ollama s'il est installé (même arrêté).
pub fn installed_exe() -> Option<std::path::PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let exe = std::path::Path::new(&local)
        .join("Programs")
        .join("Ollama")
        .join("ollama.exe");
    exe.is_file().then_some(exe)
}

/// Démarre `ollama serve` en arrière-plan s'il est installé mais arrêté.
pub async fn start_if_installed(http: &reqwest::Client) -> Result<bool> {
    if list(http, DEFAULT_URL).await.is_some() {
        return Ok(true);
    }
    let Some(exe) = installed_exe() else {
        return Ok(false);
    };
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("serve");
    crate::process::hide_window(&mut cmd);
    let child = cmd.kill_on_drop(false).spawn()?;
    // Ollama démarré par Claudio s'arrête avec lui.
    crate::process::attach(&child);
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if list(http, DEFAULT_URL).await.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}
