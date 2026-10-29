//! Couche de services de Claudio : assemble matériel, modèles, moteurs et
//! conversations. L'application de bureau et l'outil en ligne de commande
//! n'en sont que des façades.

mod access;
mod attachments;
mod cloud;
mod conversation;
mod hub;
mod knowledge;
pub mod router;
mod schedule;
mod tasks;
mod telegram;
mod voice;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, bail};
use claudio_core::config::ActiveModel;
use claudio_core::{AppPaths, Config, Database, secrets};
use claudio_hw::{HardwareInfo, MachineProfile};
use claudio_llm::chat::Endpoint;
use claudio_llm::ollama::{self, OllamaModel};
use claudio_llm::runtime::{self, InstallEvent, InstalledRuntime};
use claudio_llm::{LocalEngine, ServerConfig};
use claudio_models::select::{self, DEFAULT_CONTEXT};
use claudio_models::{
    CancelToken, Candidate, Catalog, InstalledModel, Progress, Registry, catalog,
};
use serde::Serialize;

pub use access::AccessOverview;
pub use attachments::Attachment;
pub use claudio_agents;
pub use claudio_core;
pub use claudio_hw;
pub use claudio_llm;
pub use claudio_memory;
pub use claudio_models;
pub use claudio_perms;
pub use claudio_tools;
pub use claudio_voice;
pub use cloud::{BudgetStatus, ProviderView, UsageReport};
pub use conversation::{Reply, StreamEvent, ToolTrace};
pub use hub::{RepoView, VariantView};
pub use knowledge::KnowledgeStatus;
pub use schedule::ScheduleView;
pub use tasks::{AppEvent, EventSink, TaskDetail};
pub use telegram::TelegramStatus;
pub use voice::{Transcript, VoiceComponentView, VoiceStatus};

pub const DEFAULT_SESSION_TITLE: &str = "Nouvelle conversation";

pub struct Claudio {
    pub paths: AppPaths,
    config: RwLock<Config>,
    pub db: Database,
    pub(crate) http: reqwest::Client,
    hw: RwLock<HardwareInfo>,
    pub catalog: Catalog,
    engine: LocalEngine,
    downloads: Mutex<HashMap<String, CancelToken>>,
    pub(crate) chats: Mutex<HashMap<String, CancelToken>>,
    /// Moteur de permissions des outils.
    pub perms: claudio_perms::Engine,
    pub(crate) toolbox: claudio_tools::Toolbox,
    /// Demandes d'autorisation en attente de réponse de l'utilisateur.
    pub(crate) approvals:
        Mutex<HashMap<String, tokio::sync::oneshot::Sender<claudio_perms::Answer>>>,
    pub(crate) approval_seq: std::sync::atomic::AtomicU64,
    pub(crate) agents: RwLock<claudio_agents::AgentSet>,
    pub(crate) running_tasks: Mutex<HashMap<String, CancelToken>>,
    pub(crate) events: RwLock<Option<tasks::EventSink>>,
    /// Places d'exécution simultanée des agents (modèle local / distant).
    pub(crate) local_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) cloud_slots: Arc<tokio::sync::Semaphore>,
    /// Serveur d'embeddings (distinct du modèle de conversation).
    pub(crate) embedder: LocalEngine,
    /// Dossiers en cours d'indexation.
    pub(crate) indexing: Mutex<std::collections::HashSet<i64>>,
    pub(crate) watcher: Mutex<
        Option<notify_debouncer_mini::Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>>,
    >,
    /// Serveurs de reconnaissance vocale.
    pub(crate) whisper: claudio_voice::Whisper,
    pub(crate) telegram: telegram::State,
}

/// Agents distants exécutés en même temps au plus.
const CLOUD_PARALLEL: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub hardware: HardwareInfo,
    pub profile: MachineProfile,
    pub runtime: Option<InstalledRuntime>,
    pub active_model: Option<ActiveModel>,
    /// Modèle actuellement chargé en mémoire par llama.cpp.
    /// Modèles locaux en mémoire, du plus récemment utilisé au plus ancien.
    pub loaded_models: Vec<String>,
    /// `None` si Ollama ne tourne pas (ou est désactivé).
    pub ollama_models: Option<Vec<OllamaModel>>,
    pub installed_models: Vec<InstalledModel>,
    pub has_hf_token: bool,
    /// Modèles installés servant aux embeddings (pas à la conversation).
    pub embedding_models: Vec<String>,
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

impl Claudio {
    pub fn open(paths: AppPaths) -> Result<Arc<Self>> {
        paths.ensure()?;
        let config = Config::load(&paths.config_file())?;
        let db = Database::open(&paths.database_file())?;
        let workspace = paths.workspace_dir();
        if let Err(e) = claudio_perms::sandbox::label_low_integrity(&workspace) {
            tracing::warn!("espace de travail : {e:#}");
        }
        let protected =
            claudio_perms::Protected::defaults(&[paths.config_dir.clone(), paths.data_dir.clone()]);
        let perms = claudio_perms::Engine::new(
            db.clone(),
            config.permissions.clone(),
            protected,
            workspace,
        );
        let readme = paths.agents_dir().join("LISEZMOI.txt");
        if !readme.exists() {
            std::fs::write(&readme, claudio_agents::defs::README).ok();
        }
        let agents = claudio_agents::AgentSet::load(Some(&paths.agents_dir()));
        let interrupted = db.mark_interrupted_tasks()?;
        if interrupted > 0 {
            tracing::info!(interrupted, "tâches interrompues par l'arrêt précédent");
        }
        let hw = HardwareInfo::detect();
        let parallel = hw.profile().max_parallel_agents.max(1);
        Ok(Arc::new(Self {
            agents: RwLock::new(agents),
            embedder: LocalEngine::default(),
            indexing: Mutex::default(),
            watcher: Mutex::default(),
            whisper: Default::default(),
            telegram: Default::default(),
            running_tasks: Mutex::default(),
            events: RwLock::default(),
            local_slots: Arc::new(tokio::sync::Semaphore::new(parallel)),
            cloud_slots: Arc::new(tokio::sync::Semaphore::new(CLOUD_PARALLEL)),
            perms,
            toolbox: claudio_tools::Toolbox::new(Arc::default()),
            approvals: Mutex::default(),
            approval_seq: Default::default(),
            config: RwLock::new(config),
            db,
            http: claudio_models::download::client(),
            hw: RwLock::new(hw),
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

    pub(crate) fn hf_token(&self) -> Option<String> {
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
            loaded_models: self.engine.loaded().await,
            ollama_models,
            installed_models: self.registry().list()?,
            has_hf_token: self.hf_token().is_some(),
            embedding_models: self
                .registry()
                .list()?
                .into_iter()
                .filter(|m| {
                    self.catalog
                        .get(&m.model_id)
                        .is_some_and(|c| c.is_embedding())
                })
                .map(|m| m.key)
                .collect(),
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

    pub(crate) fn begin_download(&self, key: &str) -> Result<CancelToken> {
        if self.downloads.lock().unwrap().contains_key(key) {
            bail!("ce modèle est déjà en cours de téléchargement");
        }
        Ok(self.register_download(key))
    }

    pub(crate) fn end_download(&self, key: &str) {
        self.downloads.lock().unwrap().remove(key);
    }

    /// Le premier modèle installé devient le modèle actif.
    pub(crate) fn after_install(&self, installed: InstalledModel) -> Result<InstalledModel> {
        if self.config().active_model.is_none() {
            self.set_active_model(ActiveModel::Local {
                id: installed.key.clone(),
            })?;
        }
        Ok(installed)
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
        let cancel = self.begin_download(&key)?;
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
        self.end_download(&key);
        self.after_install(res?)
    }

    pub async fn remove_model(&self, key: &str) -> Result<()> {
        self.engine.unload_model(key).await;
        self.registry().remove(key)?;
        let removed = ActiveModel::Local { id: key.to_owned() };
        self.update_config(|c| {
            if c.active_model.as_ref() == Some(&removed) {
                c.active_model = None;
            }
            c.agent_models.retain(|_, m| *m != removed);
        })?;
        Ok(())
    }

    pub fn set_active_model(&self, model: ActiveModel) -> Result<()> {
        self.update_config(|c| c.active_model = Some(model))?;
        Ok(())
    }

    /// Modèle choisi pour un agent ; sinon `None` (modèle actif).
    pub(crate) fn local_model_for(&self, agent_id: &str) -> Option<ActiveModel> {
        self.config().agent_models.get(agent_id).cloned()
    }

    /// Choisit le modèle local d'un agent (`None` : modèle actif).
    pub fn set_agent_model(&self, agent_id: &str, model: Option<ActiveModel>) -> Result<()> {
        self.update_config(|c| match model {
            Some(m) => {
                c.agent_models.insert(agent_id.to_owned(), m);
            }
            None => {
                c.agent_models.remove(agent_id);
            }
        })?;
        Ok(())
    }

    /// Prépare le modèle actif et renvoie son point d'accès (chargement si besoin).
    pub async fn active_endpoint(&self) -> Result<Endpoint> {
        self.endpoint_for(None).await
    }

    /// Prépare le modèle local donné (sinon le modèle actif) et renvoie son
    /// point d'accès ; d'autres modèles peuvent rester chargés à côté.
    pub async fn endpoint_for(&self, model: Option<&ActiveModel>) -> Result<Endpoint> {
        let cfg = self.config();
        match model.or(cfg.active_model.as_ref()).cloned() {
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
                let profile = self.hardware().profile();
                let budget = if server.use_gpu {
                    profile.vram_budget
                } else {
                    profile.ram_budget
                };
                self.engine.ensure(&self.http, server, budget).await
            }
        }
    }

    /// Mémoire estimée d'un modèle installé pour un contexte total donné.
    fn model_memory(&self, profile: &MachineProfile, m: &InstalledModel, context: u32) -> u64 {
        match self
            .catalog
            .get(&m.model_id)
            .and_then(|c| c.quant(&m.quant).map(|q| (c, q)))
        {
            Some((c, q)) => select::memory_needed(c, q, context),
            None => {
                let meta = claudio_models::select::ModelMeta {
                    params_b: m.params_b,
                    active_params_b: None,
                    moe: false,
                };
                select::estimate_fit(profile, m.size, &m.quant, context, &meta).memory_needed
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
        let model_max = m
            .context_max
            .or_else(|| self.catalog.get(&m.model_id).map(|c| c.context_max));
        let ctx = cfg
            .context_size
            .unwrap_or(DEFAULT_CONTEXT)
            .min(model_max.unwrap_or(u32::MAX));
        // Place gardée pour les autres modèles choisis (agents), s'ils peuvent
        // tenir ensemble ; sinon ils se relaieront en mémoire.
        let mut others: Vec<&str> = cfg
            .agent_models
            .values()
            .chain(cfg.active_model.as_ref())
            .filter_map(|a| match a {
                ActiveModel::Local { id } if *id != m.key => Some(id.as_str()),
                _ => None,
            })
            .collect();
        others.sort_unstable();
        others.dedup();
        let reserve: u64 = others
            .iter()
            .filter_map(|id| self.registry().get(id).ok().flatten())
            .map(|o| self.model_memory(&profile, &o, ctx))
            .sum();
        let reserve = if self.model_memory(&profile, m, ctx) + reserve <= profile.vram_budget {
            reserve
        } else {
            0
        };
        // Plusieurs agents peuvent interroger le modèle en même temps : le contexte
        // partagé est agrandi tant qu'il tient dans la mémoire vidéo.
        let parallel = profile.max_parallel_agents.max(1) as u32;
        let fits =
            |total: u32| self.model_memory(&profile, m, total) + reserve <= profile.vram_budget;
        let total = [parallel, 2]
            .into_iter()
            .map(|k| ctx.saturating_mul(k))
            .find(|&t| t > ctx && fits(t))
            .unwrap_or(ctx);
        Ok(ServerConfig {
            exe: rt.server_exe(),
            model_path: self.registry().path_of(m),
            alias: m.key.clone(),
            context: total,
            use_gpu: rt.backend.uses_gpu() && profile.gpu_index.is_some(),
            threads: profile.threads,
            parallel,
            embedding_pooling: None,
            memory: self.model_memory(&profile, m, total),
        })
    }

    /// Arrête proprement les moteurs (appelé à la fermeture).
    pub async fn shutdown(&self) {
        self.stop_telegram();
        self.engine.unload().await;
        self.embedder.unload().await;
        self.whisper.stop_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_in_portable_dir() {
        let dir = tempfile::tempdir().unwrap();
        let app = Claudio::open(AppPaths::in_dir(dir.path().to_path_buf())).unwrap();
        assert!(!app.recommendations().unwrap().is_empty() || app.hardware().ram_total < (4 << 30));
        let s = app.create_session().unwrap();
        assert_eq!(app.db.list_sessions().unwrap()[0].id, s.id);
    }
}
