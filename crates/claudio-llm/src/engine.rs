//! Gestion du modèle local chargé : un seul serveur llama.cpp à la fois pour l'instant.

use anyhow::Result;
use tokio::sync::Mutex;

use crate::chat::Endpoint;
use crate::server::{LlamaServer, ServerConfig};

#[derive(Default)]
pub struct LocalEngine {
    current: Mutex<Option<LlamaServer>>,
}

fn same(a: &ServerConfig, b: &ServerConfig) -> bool {
    a.model_path == b.model_path
        && a.context == b.context
        && a.use_gpu == b.use_gpu
        && a.parallel == b.parallel
        && a.exe == b.exe
}

impl LocalEngine {
    /// Renvoie le point d'accès du modèle demandé, en (re)démarrant le serveur si besoin.
    pub async fn ensure(&self, http: &reqwest::Client, config: ServerConfig) -> Result<Endpoint> {
        let mut cur = self.current.lock().await;
        if let Some(s) = cur.as_mut()
            && same(&s.config, &config)
            && s.is_running()
        {
            return Ok(s.endpoint());
        }
        if let Some(old) = cur.take() {
            old.stop().await;
        }
        let server = LlamaServer::start(http, config).await?;
        let ep = server.endpoint();
        *cur = Some(server);
        Ok(ep)
    }

    /// Alias du modèle chargé, s'il y en a un.
    pub async fn loaded(&self) -> Option<String> {
        let mut cur = self.current.lock().await;
        let s = cur.as_mut()?;
        s.is_running().then(|| s.config.alias.clone())
    }

    pub async fn unload(&self) {
        if let Some(s) = self.current.lock().await.take() {
            s.stop().await;
        }
    }
}
