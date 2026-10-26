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
