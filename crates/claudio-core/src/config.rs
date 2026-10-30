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
    /// Modèle local propre à certains agents (identifiant d'agent → modèle) ;
    /// les autres utilisent le modèle actif. Plusieurs modèles restent chargés
    /// ensemble tant qu'ils tiennent en mémoire.
    pub agent_models: std::collections::BTreeMap<String, ActiveModel>,
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
    /// Règles d'accès des outils aux ressources de la machine.
    pub permissions: Permissions,
    /// Serveurs MCP (outils externes).
    pub mcp_servers: Vec<McpServerConfig>,
    /// Laisser l'assistant utiliser des outils (fichiers, web, commandes, MCP).
    pub tools_enabled: bool,
    pub memory: MemorySettings,
    pub voice: VoiceSettings,
    pub telegram: TelegramSettings,
    /// Vérifier au démarrage si une nouvelle version de Claudio est publiée.
    pub check_updates: bool,
}

/// Quand lire les réponses à voix haute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeakMode {
    Never,
    /// Seulement quand la question a été posée à la voix.
    #[default]
    Voice,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsEngine {
    /// Voix installées avec Windows.
    #[default]
    System,
    Piper,
}

/// Voix : reconnaissance et synthèse locales.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// Langue parlée (code ISO, ex. `fr`).
    pub language: String,
    /// Modèle de dictée ; sinon choisi selon la machine.
    pub stt_model: Option<String>,
    /// Écouter en permanence le mot d'éveil.
    pub wake_word: bool,
    pub wake_phrase: String,
    pub speak: SpeakMode,
    pub tts_engine: TtsEngine,
    /// Voix Windows ; sinon la première dans la langue choisie.
    pub system_voice: Option<String>,
    pub piper_voice: String,
    /// Débit, de -10 à 10.
    pub rate: i32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            language: "fr".into(),
            stt_model: None,
            wake_word: false,
            wake_phrase: "Claudio".into(),
            speak: SpeakMode::Voice,
            tts_engine: TtsEngine::System,
            system_voice: None,
            piper_voice: "piper-siwis".into(),
            rate: 0,
        }
    }
}

/// Canal Telegram (le jeton du robot est dans le Gestionnaire d'identifiants).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramSettings {
    pub enabled: bool,
    /// Conversations Telegram autorisées (jumelées).
    pub chats: Vec<TelegramChat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    /// Nom affiché de l'interlocuteur.
    pub name: String,
}

/// Mémoire à long terme et apprentissage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemorySettings {
    /// Retenir automatiquement les faits durables des conversations (modèle local uniquement).
    pub auto_extract: bool,
    /// Rappeler les souvenirs utiles au début de chaque réponse.
    pub recall: bool,
    /// Proposer des compétences réutilisables après une tâche réussie.
    pub learn_skills: bool,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            auto_extract: true,
            recall: true,
            learn_skills: true,
        }
    }
}

/// Que faire quand un outil demande une ressource non encore autorisée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// Demander à l'utilisateur.
    #[default]
    Ask,
    /// Autoriser sans demander.
    Allow,
    /// Toujours refuser.
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    /// Accès aux sites web (par domaine).
    pub network: Policy,
    /// Commandes PowerShell : `Ask` (chaque commande) ou `Deny`.
    pub shell: Policy,
    /// Exécuter les commandes dans le bac à sable (intégrité basse : écriture limitée
    /// à l'espace de travail de Claudio).
    pub shell_sandbox: bool,
    /// Appels d'outils MCP non encore autorisés.
    pub mcp: Policy,
    /// Autoriser les adresses du réseau local (routeur, NAS…) pour le web.
    pub allow_private_network: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            network: Policy::Ask,
            shell: Policy::Ask,
            shell_sandbox: true,
            mcp: Policy::Ask,
            allow_private_network: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpTransport {
    /// Programme local lancé par Claudio, dialogue sur l'entrée/sortie standard.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
    },
    /// Serveur distant (« Streamable HTTP »).
    Http { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    pub enabled: bool,
    /// Clés transmises au serveur : variables d'environnement (stdio) ou en-têtes
    /// HTTP (`Authorization`…). Seuls les noms sont ici ; les valeurs sont dans le
    /// Gestionnaire d'identifiants.
    #[serde(default)]
    pub secrets: Vec<String>,
}

impl McpServerConfig {
    /// Nom, dans le Gestionnaire d'identifiants, de la valeur d'une clé du serveur.
    pub fn secret_name(&self, key: &str) -> String {
        format!("mcp-{}-{key}", self.id)
    }
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
            agent_models: Default::default(),
            context_size: None,
            use_ollama: true,
            llama_backend: None,
            thinking: None,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_owned(),
            onboarding_done: false,
            providers: Vec::new(),
            routing: Routing::default(),
            budget: Budget::default(),
            permissions: Permissions::default(),
            mcp_servers: Vec::new(),
            tools_enabled: true,
            memory: MemorySettings::default(),
            voice: VoiceSettings::default(),
            telegram: TelegramSettings::default(),
            check_updates: true,
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
