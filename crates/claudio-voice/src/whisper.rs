//! Reconnaissance vocale : processus `whisper-server` gardés en mémoire (un par
//! modèle), interrogés en local.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use claudio_llm::process;
use serde::Deserialize;
use tokio::process::{Child, Command};

struct Server {
    child: Child,
    port: u16,
}

/// Serveurs démarrés à la demande ; ils s'arrêtent avec Claudio.
#[derive(Default)]
pub struct Whisper {
    servers: tokio::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<Server>>>>,
}

#[derive(Debug, Deserialize)]
struct Inference {
    text: String,
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// Moteur et modèle à utiliser.
pub struct Model<'a> {
    /// `whisper-server.exe`.
    pub exe: &'a Path,
    /// Fichier du modèle (`ggml-….bin`).
    pub file: &'a Path,
    pub threads: usize,
}

impl Whisper {
    /// Transcrit un fichier WAV (16 kHz, mono, 16 bits) ; `prompt` oriente
    /// l'orthographe (noms propres, mot d'éveil).
    pub async fn transcribe(
        &self,
        http: &reqwest::Client,
        model: &Model<'_>,
        wav: Vec<u8>,
        language: &str,
        prompt: Option<&str>,
    ) -> Result<String> {
        let server = self
            .ensure(http, model.exe, model.file, model.threads)
            .await?;
        let port = server.lock().await.port;
        let mut form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(wav)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")?,
            )
            .text("response_format", "json")
            .text("temperature", "0.0")
            .text("language", language.to_owned());
        if let Some(p) = prompt {
            form = form.text("prompt", p.to_owned());
        }
        let r: Inference = http
            .post(format!("http://127.0.0.1:{port}/inference"))
            .multipart(form)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .context("whisper-server ne répond pas")?
            .error_for_status()?
            .json()
            .await?;
        Ok(clean(&r.text))
    }

    async fn ensure(
        &self,
        http: &reqwest::Client,
        exe: &Path,
        model: &Path,
        threads: usize,
    ) -> Result<Arc<tokio::sync::Mutex<Server>>> {
        let mut servers = self.servers.lock().await;
        if let Some(s) = servers.get(model) {
            if matches!(s.lock().await.child.try_wait(), Ok(None)) {
                return Ok(s.clone());
            }
            servers.remove(model);
        }
        let s = Arc::new(tokio::sync::Mutex::new(
            start(http, exe, model, threads).await?,
        ));
        servers.insert(model.to_owned(), s.clone());
        Ok(s)
    }

    /// Arrête les serveurs (voix désactivée ou modèle supprimé).
    pub async fn stop_all(&self) {
        for (_, s) in self.servers.lock().await.drain() {
            let _ = s.lock().await.child.kill().await;
        }
    }
}

async fn start(http: &reqwest::Client, exe: &Path, model: &Path, threads: usize) -> Result<Server> {
    if !model.is_file() {
        bail!(
            "modèle de reconnaissance vocale absent : {}",
            model.display()
        );
    }
    let port = free_port()?;
    let mut cmd = Command::new(exe);
    cmd.arg("--model")
        .arg(model)
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .args(["--threads", &threads.clamp(1, 8).to_string()])
        .arg("--no-timestamps")
        .current_dir(exe.parent().unwrap_or(exe))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    process::hide_window(&mut cmd);
    tracing::info!(model = %model.display(), port, "démarrage de whisper-server");
    let mut child = cmd.spawn().context("lancement de whisper-server")?;
    process::attach(&child);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            bail!("whisper-server s'est arrêté au démarrage ({status})");
        }
        // Le serveur n'écoute qu'une fois le modèle chargé.
        if http
            .get(format!("http://127.0.0.1:{port}/"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_ok()
        {
            tracing::info!(
                secs = started.elapsed().as_secs_f32(),
                "modèle vocal chargé"
            );
            return Ok(Server { child, port });
        }
        if started.elapsed() > Duration::from_secs(120) {
            let _ = child.kill().await;
            bail!("le modèle vocal n'a pas fini de charger après 2 minutes");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Retire les annotations que Whisper ajoute sur les silences ou les bruits.
pub fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // Hallucinations fréquentes de Whisper sur un silence.
    const NOISE: &[&str] = &[
        "sous-titres réalisés par la communauté d'amara.org",
        "sous-titrage st' 501",
        "merci d'avoir regardé cette vidéo !",
        "...",
    ];
    if NOISE.contains(&out.to_lowercase().as_str()) {
        String::new()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_annotations_and_noise() {
        assert_eq!(
            clean(" [Musique] Bonjour (rires) Claudio  "),
            "Bonjour Claudio"
        );
        assert_eq!(clean("[BLANK_AUDIO]"), "");
        assert_eq!(
            clean(" Sous-titres réalisés par la communauté d'Amara.org"),
            ""
        );
    }
}
