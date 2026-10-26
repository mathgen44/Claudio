//! Couche de services de Claudio : assemble matériel, modèles, moteurs et
//! conversations. L'application de bureau et l'outil en ligne de commande
//! n'en sont que des façades.

mod access;
mod cloud;
mod conversation;
mod hub;
pub mod router;

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
pub use claudio_core;
pub use claudio_hw;
pub use claudio_llm;
pub use claudio_models;
pub use claudio_perms;
pub use claudio_tools;
pub use cloud::{BudgetStatus, ProviderView, UsageReport};
pub use conversation::{Reply, StreamEvent, ToolTrace};
pub use hub::{RepoView, VariantView};

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
        Ok(Arc::new(Self {
            perms,
            toolbox: claudio_tools::Toolbox::new(Arc::default()),
            approvals: Mutex::default(),
            approval_seq: Default::default(),
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
        let model_max = m
            .context_max
            .or_else(|| self.catalog.get(&m.model_id).map(|c| c.context_max));
        let ctx = cfg
            .context_size
            .unwrap_or(DEFAULT_CONTEXT)
            .min(model_max.unwrap_or(u32::MAX));
        Ok(ServerConfig {
            exe: rt.server_exe(),
            model_path: self.registry().path_of(m),
            alias: m.key.clone(),
            context: ctx,
            use_gpu: rt.backend.uses_gpu() && profile.gpu_index.is_some(),
            threads: profile.threads,
        })
    }

    /// Arrête proprement les moteurs (appelé à la fermeture).
    pub async fn shutdown(&self) {
        self.engine.unload().await;
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
