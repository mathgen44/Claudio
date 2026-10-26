//! Catalogue des modèles recommandés, embarqué depuis `catalog/models.json`.

use anyhow::Result;
use serde::{Deserialize, Serialize};

const BUILTIN: &str = include_str!("../../../catalog/models.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    pub models: Vec<CatalogModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub license: String,
    pub params_b: f64,
    /// Paramètres actifs par jeton (différent de `params_b` pour les mélanges d'experts).
    pub active_params_b: f64,
    /// Qualité estimée sur 100, utilisée pour classer les modèles entre eux.
    pub quality: f64,
    /// Taille du cache KV par jeton de contexte, en Kio, avec un cache en f16.
    pub kv_kb_per_token: u64,
    pub context_max: u32,
    pub roles: Vec<String>,
    /// Modèles d'embeddings : regroupement des vecteurs (`last`, `mean`, `cls`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pooling: Option<String>,
    /// Modèles d'embeddings : dimension des vecteurs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dims: Option<u32>,
    /// Modèles d'embeddings : préfixe à placer devant une requête de recherche.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_prefix: Option<String>,
    pub description: String,
    pub quants: Vec<Quant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quant {
    pub name: String,
    pub file: String,
    pub size: u64,
}

impl CatalogModel {
    pub fn is_moe(&self) -> bool {
        self.active_params_b < self.params_b
    }

    pub fn is_embedding(&self) -> bool {
        self.roles.iter().any(|r| r == "embedding")
    }

    pub fn quant(&self, name: &str) -> Option<&Quant> {
        self.quants.iter().find(|q| q.name == name)
    }
}

/// Facteur de qualité conservé selon la quantification (1.0 = quasi sans perte).
pub fn quant_factor(name: &str) -> f64 {
    let n = name.trim_start_matches("UD-");
    match n {
        "Q8_0" => 1.0,
        "Q6_K" | "Q6_K_XL" => 0.99,
        "Q5_K_M" | "Q5_K_XL" => 0.98,
        "Q4_K_M" | "Q4_K_XL" => 0.95,
        "IQ4_XS" => 0.93,
        "Q3_K_M" => 0.85,
        _ => 0.8,
    }
}

impl Catalog {
    pub fn builtin() -> Result<Self> {
        Ok(serde_json::from_str(BUILTIN)?)
    }

    pub fn get(&self, id: &str) -> Option<&CatalogModel> {
        self.models.iter().find(|m| m.id == id)
    }
}

/// Identifiant d'un modèle installé : `<id du catalogue>:<quantification>`.
pub fn model_key(id: &str, quant: &str) -> String {
    format!("{id}:{quant}")
}

pub fn split_key(key: &str) -> Option<(&str, &str)> {
    key.rsplit_once(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_is_valid() {
        let c = Catalog::builtin().unwrap();
        assert!(!c.models.is_empty());
        for m in &c.models {
            assert!(!m.quants.is_empty(), "{} sans quantification", m.id);
            assert!(m.active_params_b <= m.params_b);
            for q in &m.quants {
                assert!(q.size > 100_000_000, "{} {} trop petit", m.id, q.name);
                assert!(q.file.ends_with(".gguf"));
            }
        }
        let mut ids: Vec<_> = c.models.iter().map(|m| &m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), c.models.len(), "identifiants en double");
    }

    #[test]
    fn keys() {
        assert_eq!(model_key("qwen3.5-4b", "Q4_K_M"), "qwen3.5-4b:Q4_K_M");
        assert_eq!(
            split_key("qwen3.5-4b:Q4_K_M"),
            Some(("qwen3.5-4b", "Q4_K_M"))
        );
    }
}
