//! Couche de services de Claudio : assemble matériel, modèles, moteurs et
//! conversations. L'application de bureau et l'outil en ligne de commande
//! n'en sont que des façades.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use claudio_core::config::ActiveModel;
use claudio_core::{AppPaths, Config, Database, Message, MessageMeta, Role, Session, secrets};
use claudio_hw::{HardwareInfo, MachineProfile};
use claudio_llm::chat::{self, ChatEvent, ChatMessage, Endpoint, Options};
use claudio_llm::ollama::{self, OllamaModel};
use claudio_llm::runtime::{self, InstallEvent, InstalledRuntime};
use claudio_llm::{LocalEngine, ServerConfig};
use claudio_models::select::{self, DEFAULT_CONTEXT};
use claudio_models::{
    CancelToken, Candidate, Catalog, InstalledModel, Progress, Registry, catalog,
};
use serde::Serialize;

pub use claudio_core;
pub use claudio_hw;
pub use claudio_llm;
pub use claudio_models;

pub const DEFAULT_SESSION_TITLE: &str = "Nouvelle conversation";

pub struct Claudio {
    pub paths: AppPaths,
    config: RwLock<Config>,
    pub db: Database,
    http: reqwest::Client,
    hw: RwLock<HardwareInfo>,
    pub catalog: Catalog,
    engine: LocalEngine,
    downloads: Mutex<HashMap<String, CancelToken>>,
    chats: Mutex<HashMap<String, CancelToken>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub hardware: HardwareInfo,
    pub profile: MachineProfile,
    pub runtime: Option<InstalledRuntime>,
    pub active_model: Option<ActiveModel>,
    /// Modèle actuellement chargé en mémoire par llama.cpp.
    pub loaded_model: Option<String>,
    /// `None` si Ollama ne tourne pas (ou est désactivé).
    pub ollama_models: Option<Vec<OllamaModel>>,
    pub installed_models: Vec<InstalledModel>,
    pub has_hf_token: bool,
    pub onboarding_done: bool,
    pub models_dir: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    #[serde(flatten)]
    pub candidate: Candidate,
    pub installed: bool,
    pub description: String,
    pub license: String,
}

/// Fin d'une réponse : le message enregistré et quelques mesures.
#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub message: Message,
    pub reasoning: String,
    pub tokens_per_sec: Option<f64>,
    pub cancelled: bool,
}

impl Claudio {
    pub fn open(paths: AppPaths) -> Result<Arc<Self>> {
        paths.ensure()?;
        let config = Config::load(&paths.config_file())?;
        let db = Database::open(&paths.database_file())?;
        Ok(Arc::new(Self {
            config: RwLock::new(config),
            db,
            http: claudio_models::download::client(),
            hw: RwLock::new(HardwareInfo::detect()),
            catalog: Catalog::builtin()?,
            engine: LocalEngine::default(),
            downloads: Mutex::default(),
            chats: Mutex::default(),
            paths,
        }))
    }

    // ---- Configuration ------------------------------------------------------

    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) -> Result<Config> {
        let mut cfg = self.config.write().unwrap();
        let mut next = cfg.clone();
        f(&mut next);
        next.save(&self.paths.config_file())?;
        *cfg = next.clone();
        Ok(next)
    }

    pub fn registry(&self) -> Registry {
        Registry::new(self.config().models_dir(&self.paths))
    }

    fn hf_token(&self) -> Option<String> {
        secrets::get(secrets::HF_TOKEN).ok().flatten()
    }

    pub fn set_hf_token(&self, token: Option<&str>) -> Result<()> {
        match token.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => secrets::set(secrets::HF_TOKEN, t),
            None => secrets::delete(secrets::HF_TOKEN),
        }
    }

    // ---- Machine et état ----------------------------------------------------

    pub fn refresh_hardware(&self) -> HardwareInfo {
        let hw = HardwareInfo::detect();
        *self.hw.write().unwrap() = hw.clone();
        hw
    }

    pub fn hardware(&self) -> HardwareInfo {
        self.hw.read().unwrap().clone()
    }

    pub async fn status(&self) -> Result<Status> {
        let cfg = self.config();
        let hardware = self.hardware();
        let ollama_models = if cfg.use_ollama {
            ollama::list(&self.http, ollama::DEFAULT_URL).await
        } else {
            None
        };
        Ok(Status {
            profile: hardware.profile(),
            hardware,
            runtime: runtime::installed(&self.paths.runtimes_dir()),
            active_model: cfg.active_model.clone(),
            loaded_model: self.engine.loaded().await,
            ollama_models,
            installed_models: self.registry().list()?,
            has_hf_token: self.hf_token().is_some(),
            onboarding_done: cfg.onboarding_done,
            models_dir: cfg.models_dir(&self.paths).display().to_string(),
        })
    }

    /// Modèles du catalogue exécutables sur cette machine, du meilleur au moins bon.
    pub fn recommendations(&self) -> Result<Vec<Recommendation>> {
        let profile = self.hardware().profile();
        let installed: Vec<String> = self.registry().list()?.into_iter().map(|m| m.key).collect();
        let ctx = self.config().context_size.unwrap_or(DEFAULT_CONTEXT);
        Ok(select::rank(&self.catalog, &profile, "chat", ctx)
            .into_iter()
            .map(|c| {
                let m = self.catalog.get(&c.model_id).expect("modèle du catalogue");
                Recommendation {
                    installed: installed.contains(&catalog::model_key(&c.model_id, &c.quant)),
                    description: m.description.clone(),
                    license: m.license.clone(),
                    candidate: c,
                }
            })
            .collect())
    }

    // ---- Moteur llama.cpp ---------------------------------------------------

    pub async fn install_runtime(
        &self,
        on_event: impl FnMut(InstallEvent),
    ) -> Result<InstalledRuntime> {
        let hw = self.hardware();
        let forced = self.config().llama_backend;
        let prefs = runtime::preferences(&hw, &hw.profile(), forced.as_deref());
        let cancel = self.register_download("runtime");
        let res = runtime::install(
            &self.http,
            &self.paths.runtimes_dir(),
            &prefs,
            &cancel,
            on_event,
        )
        .await;
        self.downloads.lock().unwrap().remove("runtime");
        res
    }

    // ---- Modèles ------------------------------------------------------------

    fn register_download(&self, key: &str) -> CancelToken {
        let token = CancelToken::default();
        self.downloads
            .lock()
            .unwrap()
            .insert(key.to_owned(), token.clone());
        token
    }

    pub fn cancel_download(&self, key: &str) {
        if let Some(t) = self.downloads.lock().unwrap().get(key) {
            t.cancel();
        }
    }

    pub async fn install_model(
        &self,
        model_id: &str,
        quant: &str,
        on_progress: impl FnMut(&Progress),
    ) -> Result<InstalledModel> {
        let model = self
            .catalog
            .get(model_id)
            .with_context(|| format!("modèle inconnu : {model_id}"))?;
        let key = catalog::model_key(model_id, quant);
        if self.downloads.lock().unwrap().contains_key(&key) {
            bail!("ce modèle est déjà en cours de téléchargement");
        }
        let cancel = self.register_download(&key);
        let token = self.hf_token();
        let res = claudio_models::install(
            &self.http,
            &self.registry(),
            model,
            quant,
            token.as_deref(),
            &cancel,
            on_progress,
        )
        .await;
        self.downloads.lock().unwrap().remove(&key);
        let installed = res?;
        // Premier modèle installé : il devient le modèle actif.
        if self.config().active_model.is_none() {
            self.set_active_model(ActiveModel::Local {
                id: installed.key.clone(),
            })?;
        }
        Ok(installed)
    }

    pub async fn remove_model(&self, key: &str) -> Result<()> {
        if self.engine.loaded().await.as_deref() == Some(key) {
            self.engine.unload().await;
        }
        self.registry().remove(key)?;
        if self.config().active_model == Some(ActiveModel::Local { id: key.to_owned() }) {
            self.update_config(|c| c.active_model = None)?;
        }
        Ok(())
    }

    pub fn set_active_model(&self, model: ActiveModel) -> Result<()> {
        self.update_config(|c| c.active_model = Some(model))?;
        Ok(())
    }

    /// Prépare le modèle actif et renvoie son point d'accès (chargement si besoin).
    pub async fn active_endpoint(&self) -> Result<Endpoint> {
        let cfg = self.config();
        match cfg.active_model.clone() {
            None => bail!("aucun modèle sélectionné : installez-en un depuis l'écran Modèles"),
            Some(ActiveModel::Ollama { name }) => {
                if ollama::list(&self.http, ollama::DEFAULT_URL)
                    .await
                    .is_none()
                    && !ollama::start_if_installed(&self.http).await?
                {
                    bail!("Ollama ne répond pas");
                }
                Ok(ollama::endpoint(ollama::DEFAULT_URL, &name))
            }
            Some(ActiveModel::Local { id }) => {
                let installed = self
                    .registry()
                    .get(&id)?
                    .with_context(|| format!("le modèle {id} n'est plus installé"))?;
                let rt = runtime::installed(&self.paths.runtimes_dir())
                    .context("le moteur llama.cpp n'est pas installé")?;
                let server = self.server_config(&cfg, &rt, &installed)?;
                self.engine.ensure(&self.http, server).await
            }
        }
    }

    fn server_config(
        &self,
        cfg: &Config,
        rt: &InstalledRuntime,
        m: &InstalledModel,
    ) -> Result<ServerConfig> {
        let profile = self.hardware().profile();
        let model = self.catalog.get(&m.model_id);
        let ctx = cfg
            .context_size
            .unwrap_or(DEFAULT_CONTEXT)
            .min(model.map(|m| m.context_max).unwrap_or(DEFAULT_CONTEXT));
        Ok(ServerConfig {
            exe: rt.server_exe(),
            model_path: self.registry().path_of(m),
            alias: m.key.clone(),
            context: ctx,
            use_gpu: rt.backend.uses_gpu() && profile.gpu_index.is_some(),
            threads: profile.threads,
        })
    }

    /// Raisonnement : choix de l'utilisateur, sinon activé pour les modèles assez grands
    /// (les petits modèles tournent en rond quand ils réfléchissent).
    fn thinking_for(&self, cfg: &Config) -> Option<bool> {
        match &cfg.active_model {
            Some(ActiveModel::Local { id }) => {
                let params = catalog::split_key(id)
                    .and_then(|(m, _)| self.catalog.get(m))
                    .map(|m| m.params_b)
                    .unwrap_or(0.0);
                Some(cfg.thinking.unwrap_or(params >= 9.0))
            }
            _ => cfg.thinking,
        }
    }

    // ---- Conversations ------------------------------------------------------

    pub fn create_session(&self) -> Result<Session> {
        self.db.create_session(DEFAULT_SESSION_TITLE)
    }

    pub fn cancel_chat(&self, session_id: &str) {
        if let Some(t) = self.chats.lock().unwrap().get(session_id) {
            t.cancel();
        }
    }

    /// Ajoute le message de l'utilisateur, fait répondre le modèle actif en continu
    /// et enregistre la réponse.
    pub async fn send_message(
        &self,
        session_id: &str,
        text: &str,
        on_event: impl FnMut(ChatEvent),
    ) -> Result<Reply> {
        let session = self
            .db
            .get_session(session_id)?
            .context("conversation introuvable")?;
        let text = text.trim();
        if text.is_empty() {
            bail!("message vide");
        }
        self.db
            .add_message(session_id, Role::User, text, MessageMeta::default())?;
        if session.title == DEFAULT_SESSION_TITLE {
            self.db.rename_session(session_id, &title_from(text))?;
        }

        let cfg = self.config();
        let mut messages = vec![ChatMessage {
            role: "system".into(),
            content: cfg.system_prompt.clone(),
        }];
        messages.extend(
            self.db
                .list_messages(session_id)?
                .into_iter()
                .map(|m| ChatMessage {
                    role: m.role.as_str().to_owned(),
                    content: m.content,
                }),
        );

        let endpoint = self.active_endpoint().await?;
        let opts = Options {
            thinking: self.thinking_for(&cfg),
            ..Default::default()
        };
        let cancel = CancelToken::default();
        self.chats
            .lock()
            .unwrap()
            .insert(session_id.to_owned(), cancel.clone());
        let started = Instant::now();
        let res =
            chat::stream_chat(&self.http, &endpoint, &messages, &opts, &cancel, on_event).await;
        self.chats.lock().unwrap().remove(session_id);
        let done = res?;

        let content = if done.content.is_empty() && done.cancelled {
            "*(réponse interrompue)*".to_owned()
        } else {
            done.content
        };
        let message = self.db.add_message(
            session_id,
            Role::Assistant,
            &content,
            MessageMeta {
                model: Some(endpoint.model.clone()),
                tokens: done.completion_tokens.map(|n| n as i64),
                duration_ms: Some(started.elapsed().as_millis() as i64),
            },
        )?;
        Ok(Reply {
            message,
            reasoning: done.reasoning,
            tokens_per_sec: done.tokens_per_sec,
            cancelled: done.cancelled,
        })
    }

    /// Arrête proprement les moteurs (appelé à la fermeture).
    pub async fn shutdown(&self) {
        self.engine.unload().await;
    }
}

/// Titre court tiré du premier message.
fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    let mut title: String = line.chars().take(48).collect();
    if line.chars().count() > 48 {
        title.push('…');
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title_from("Bonjour\nsuite"), "Bonjour");
        assert!(title_from(&"a".repeat(100)).ends_with('…'));
    }

    #[test]
    fn opens_in_portable_dir() {
        let dir = tempfile::tempdir().unwrap();
        let app = Claudio::open(AppPaths::in_dir(dir.path().to_path_buf())).unwrap();
        assert!(!app.recommendations().unwrap().is_empty() || app.hardware().ram_total < (4 << 30));
        let s = app.create_session().unwrap();
        assert_eq!(app.db.list_sessions().unwrap()[0].id, s.id);
    }
}
