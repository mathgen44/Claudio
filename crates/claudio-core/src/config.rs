use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::AppPaths;

/// Configuration utilisateur, stockée en TOML dans `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Dossier des modèles téléchargés (par défaut dans les données locales).
    pub models_dir: Option<PathBuf>,
    /// Modèle utilisé pour la conversation.
    pub active_model: Option<ActiveModel>,
    /// Taille de contexte forcée ; sinon calculée selon la machine.
    pub context_size: Option<u32>,
    /// Réutiliser une instance Ollama si elle est disponible.
    pub use_ollama: bool,
    /// Moteur llama.cpp forcé (`cuda`, `vulkan`, `cpu`) ; sinon détecté.
    pub llama_backend: Option<String>,
    /// Laisser le modèle « réfléchir » avant de répondre ; `None` = selon la taille du modèle.
    pub thinking: Option<bool>,
    /// Prompt système de l'assistant.
    pub system_prompt: String,
    /// Étape « Mise en route » terminée.
    pub onboarding_done: bool,
    /// Fournisseurs de modèles distants (les clés sont dans le Gestionnaire d'identifiants).
    pub providers: Vec<ProviderConfig>,
    pub routing: Routing,
    pub budget: Budget,
}

/// Famille d'API d'un fournisseur distant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Mistral,
    Gemini,
    OpenRouter,
    /// Toute API compatible OpenAI (LM Studio, vLLM, serveur maison…).
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    pub name: String,
    /// Adresse de l'API ; obligatoire pour `Custom`, sinon celle du fournisseur.
    pub base_url: Option<String>,
    pub enabled: bool,
    /// Prix connus par modèle, en dollars par million de jetons.
    #[serde(default)]
    pub prices: Vec<ModelPrice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    pub model: String,
    pub input_per_m: f64,
    pub output_per_m: f64,
}

impl ProviderConfig {
    /// Nom du secret contenant la clé d'API.
    pub fn secret_name(&self) -> String {
        format!("provider-{}", self.id)
    }

    pub fn price(&self, model: &str) -> Option<&ModelPrice> {
        self.prices.iter().find(|p| p.model == model)
    }
}

/// Où envoyer les demandes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingMode {
    /// Toujours le modèle local ; rien ne sort de la machine.
    Local,
    /// Local d'abord ; le cloud seulement si le local ne peut pas répondre.
    #[default]
    Auto,
    /// Le modèle distant d'abord, le local en repli.
    Cloud,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudModel {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Routing {
    pub mode: RoutingMode,
    /// Modèle distant utilisé par le routeur.
    pub cloud: Option<CloudModel>,
}

/// Plafonds de dépense en dollars ; au-delà, le cloud est bloqué.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    pub daily_usd: Option<f64>,
    pub monthly_usd: Option<f64>,
}

/// Référence vers le modèle actif, quel que soit le moteur qui le sert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum ActiveModel {
    /// Fichier GGUF géré par Claudio, servi par llama.cpp.
    Local { id: String },
    /// Modèle déjà présent dans Ollama.
    Ollama { name: String },
}

pub const DEFAULT_SYSTEM_PROMPT: &str = "Tu es Claudio, un assistant personnel qui fonctionne \
sur l'ordinateur de l'utilisateur. Tu réponds en français, de façon claire et précise. \
Quand tu ne sais pas, tu le dis.";

impl Default for Config {
    fn default() -> Self {
        Self {
            models_dir: None,
            active_model: None,
            context_size: None,
            use_ollama: true,
            llama_backend: None,
            thinking: None,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_owned(),
            onboarding_done: false,
            providers: Vec::new(),
            routing: Routing::default(),
            budget: Budget::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("lecture de {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("lecture de {}", path.display())),
        }
    }

    /// Écrit la configuration de façon atomique (fichier temporaire puis renommage).
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("écriture de {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("écriture de {}", path.display()))?;
        Ok(())
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn models_dir(&self, paths: &AppPaths) -> PathBuf {
        self.models_dir
            .clone()
            .unwrap_or_else(|| paths.default_models_dir())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = Config::load(&path).unwrap();
        assert!(cfg.use_ollama);
        cfg.active_model = Some(ActiveModel::Local {
            id: "qwen3.5-4b:Q4_K_M".into(),
        });
        cfg.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert_eq!(back.active_model, cfg.active_model);
    }
}
