//! Pilotage d'un processus `llama-server` servant un modèle GGUF.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::chat::Endpoint;
use crate::process;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub exe: PathBuf,
    pub model_path: PathBuf,
    /// Nom sous lequel le modèle est exposé par l'API.
    pub alias: String,
    pub context: u32,
    /// Utiliser la carte graphique ; llama.cpp répartit alors lui-même les couches
    /// (et les experts des modèles MoE) selon la mémoire libre (`--fit`).
    pub use_gpu: bool,
    pub threads: usize,
    /// Demandes traitées en même temps (agents en parallèle) ; le contexte est
    /// partagé entre elles (`--kv-unified`).
    pub parallel: u32,
    /// Serveur d'embeddings : mode de regroupement des vecteurs (`last`, `mean`…).
    pub embedding_pooling: Option<String>,
}

impl ServerConfig {
    fn args(&self, port: u16) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "--model".into(),
            self.model_path.display().to_string(),
            "--alias".into(),
            self.alias.clone(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "--ctx-size".into(),
            self.context.to_string(),
            "--threads".into(),
            self.threads.to_string(),
            "--parallel".into(),
            self.parallel.max(1).to_string(),
            "--kv-unified".into(),
            "--no-webui".into(),
        ];
        if let Some(pooling) = &self.embedding_pooling {
            // Un passage entier doit tenir dans un lot.
            let batch = self.context.to_string();
            a.extend(["--embedding".into(), "--pooling".into(), pooling.clone()]);
            a.extend([
                "--batch-size".into(),
                batch.clone(),
                "--ubatch-size".into(),
                batch,
            ]);
        } else {
            // Modèle de conversation intégré au fichier GGUF (outils, raisonnement).
            a.push("--jinja".into());
        }
        if self.use_gpu {
            // Attention optimisée et cache KV quantifié : moitié moins de mémoire.
            a.extend(
                [
                    "--flash-attn",
                    "on",
                    "--cache-type-k",
                    "q8_0",
                    "--cache-type-v",
                    "q8_0",
                ]
                .map(String::from),
            );
        } else {
            a.extend(["--n-gpu-layers", "0"].map(String::from));
        }
        a
    }
}

/// Serveur en cours d'exécution ; il est arrêté quand la valeur est détruite.
pub struct LlamaServer {
    child: Child,
    port: u16,
    pub config: ServerConfig,
    log_tail: Arc<Mutex<VecDeque<String>>>,
}

const LOG_TAIL: usize = 40;

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

impl LlamaServer {
    /// Démarre le serveur et attend que le modèle soit chargé.
    pub async fn start(http: &reqwest::Client, config: ServerConfig) -> Result<Self> {
        if !config.exe.is_file() {
            bail!("moteur llama.cpp introuvable : {}", config.exe.display());
        }
        let port = free_port()?;
        let mut cmd = Command::new(&config.exe);
        cmd.args(config.args(port))
            .current_dir(config.exe.parent().unwrap_or(&config.exe))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        process::hide_window(&mut cmd);
        tracing::info!(model = %config.model_path.display(), port, gpu = config.use_gpu, ctx = config.context, "démarrage de llama-server");
        let mut child = cmd.spawn().context("lancement de llama-server")?;
        process::attach(&child);

        let log_tail = Arc::new(Mutex::new(VecDeque::with_capacity(LOG_TAIL)));
        for pipe in [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        ]
        .into_iter()
        .flatten()
        {
            let tail = log_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "llama_server", "{line}");
                    let mut t = tail.lock().unwrap();
                    if t.len() == LOG_TAIL {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            });
        }

        let mut server = Self {
            child,
            port,
            config,
            log_tail,
        };
        server.wait_ready(http).await?;
        Ok(server)
    }

    async fn wait_ready(&mut self, http: &reqwest::Client) -> Result<()> {
        let started = Instant::now();
        let url = format!("http://127.0.0.1:{}/health", self.port);
        loop {
            if let Some(status) = self.child.try_wait()? {
                bail!(
                    "llama-server s'est arrêté ({status}) :\n{}",
                    self.log_tail()
                );
            }
            if let Ok(r) = http.get(&url).timeout(Duration::from_secs(2)).send().await
                && r.status().is_success()
            {
                tracing::info!(secs = started.elapsed().as_secs_f32(), "modèle chargé");
                return Ok(());
            }
            if started.elapsed() > Duration::from_secs(300) {
                bail!(
                    "le modèle n'a pas fini de charger après 5 minutes :\n{}",
                    self.log_tail()
                );
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    pub fn log_tail(&self) -> String {
        self.log_tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            api: crate::chat::Api::Local,
            base_url: format!("http://127.0.0.1:{}/v1", self.port),
            model: self.config.alias.clone(),
            api_key: None,
        }
    }

    pub async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}
