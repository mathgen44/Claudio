//! Choix automatique du modèle et de sa quantification selon le profil de la machine.

use claudio_hw::{GB, MachineProfile};
use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, CatalogModel, Quant, quant_factor};

/// Mémoire de travail de llama.cpp en plus des poids et du cache KV.
const COMPUTE_OVERHEAD: u64 = GB / 2;
/// Contexte par défaut d'une conversation.
pub const DEFAULT_CONTEXT: u32 = 8192;
/// Au-delà, l'inférence sur processeur devient trop lente pour converser.
const CPU_MAX_ACTIVE_PARAMS_B: f64 = 4.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Tout le modèle tient dans la mémoire de la carte graphique.
    Gpu,
    /// Réparti entre carte graphique et mémoire vive (idéal pour les mélanges d'experts).
    Hybrid,
    /// Exécution sur le processeur et la mémoire vive.
    Cpu,
    /// Ne tient pas en mémoire sur cette machine.
    TooBig,
}

impl Placement {
    fn priority(self) -> u8 {
        match self {
            Placement::Gpu => 0,
            Placement::Hybrid => 1,
            Placement::Cpu => 2,
            Placement::TooBig => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub model_id: String,
    pub model_name: String,
    pub quant: String,
    pub file_size: u64,
    pub placement: Placement,
    pub context: u32,
    /// Mémoire totale estimée (poids + cache KV + marge de calcul).
    pub memory_needed: u64,
    pub score: f64,
}

/// Mémoire du cache KV ; Claudio utilise un cache quantifié en q8_0 (moitié du f16).
pub fn kv_cache_bytes(model: &CatalogModel, context: u32) -> u64 {
    model.kv_kb_per_token * 1024 * context as u64 / 2
}

pub fn memory_needed(model: &CatalogModel, quant: &Quant, context: u32) -> u64 {
    quant.size + kv_cache_bytes(model, context) + COMPUTE_OVERHEAD
}

/// Où un modèle peut tourner, selon la mémoire nécessaire et ses paramètres actifs.
pub fn placement(
    profile: &MachineProfile,
    need: u64,
    active_params_b: f64,
    moe: bool,
) -> Placement {
    let gpu = profile.gpu_index.is_some();
    if gpu && need <= profile.vram_budget {
        Placement::Gpu
    } else if gpu && moe && need <= profile.vram_budget + profile.ram_budget {
        // llama.cpp garde l'attention sur la carte et place des experts en RAM (`--fit`).
        Placement::Hybrid
    } else if need <= profile.ram_budget && active_params_b <= CPU_MAX_ACTIVE_PARAMS_B {
        Placement::Cpu
    } else {
        Placement::TooBig
    }
}

/// Bits par poids approximatifs d'une quantification, pour estimer la taille d'un modèle.
fn bits_per_weight(quant: &str) -> f64 {
    let q = quant.trim_start_matches("UD-").to_ascii_uppercase();
    match q.as_str() {
        s if s.starts_with("Q8") => 8.5,
        s if s.starts_with("Q6") => 6.6,
        s if s.starts_with("Q5") => 5.7,
        s if s.starts_with("Q4") || s.starts_with("IQ4") || s.starts_with("MXFP4") => 4.8,
        s if s.starts_with("Q3") || s.starts_with("IQ3") => 3.9,
        s if s.starts_with("Q2") || s.starts_with("IQ2") => 2.9,
        s if s.starts_with("IQ1") || s.starts_with("TQ1") => 2.0,
        "F16" | "BF16" => 16.0,
        "F32" => 32.0,
        _ => 5.0,
    }
}

/// Ce que l'on sait d'un modèle hors catalogue (métadonnées Hugging Face).
#[derive(Debug, Clone, Default)]
pub struct ModelMeta {
    pub params_b: Option<f64>,
    pub active_params_b: Option<f64>,
    pub moe: bool,
}

/// Estimation pour un fichier GGUF quelconque (hors catalogue).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fit {
    pub params_b: f64,
    pub memory_needed: u64,
    pub placement: Placement,
}

pub fn estimate_fit(
    profile: &MachineProfile,
    size: u64,
    quant: &str,
    context: u32,
    meta: &ModelMeta,
) -> Fit {
    let params_b = meta
        .params_b
        .unwrap_or_else(|| size as f64 * 8.0 / bits_per_weight(quant) / 1e9);
    let active = meta
        .active_params_b
        .unwrap_or(if meta.moe { params_b / 8.0 } else { params_b });
    // Cache KV prudent, faute de connaître l'architecture : ~100 Kio + 6 Kio par milliard.
    let kv_kb_per_token = 100.0 + 6.0 * params_b.min(70.0);
    let kv = (kv_kb_per_token * 1024.0 * context as f64 / 2.0) as u64;
    let need = size + kv + COMPUTE_OVERHEAD;
    Fit {
        params_b,
        memory_needed: need,
        placement: placement(profile, need, active, meta.moe),
    }
}

/// Toutes les combinaisons modèle/quantification exécutables sur la machine,
/// de la meilleure à la moins bonne.
pub fn rank(
    catalog: &Catalog,
    profile: &MachineProfile,
    role: &str,
    context: u32,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    for m in catalog
        .models
        .iter()
        .filter(|m| m.roles.iter().any(|r| r == role))
    {
        let ctx = context.min(m.context_max);
        for q in &m.quants {
            let need = memory_needed(m, q, ctx);
            let placement = placement(profile, need, m.active_params_b, m.is_moe());
            let mut score = m.quality * quant_factor(&q.name);
            match placement {
                Placement::Gpu => {}
                // Plus lent qu'en tout-GPU, mais bien plus capable qu'un petit modèle.
                Placement::Hybrid => score *= 0.85,
                // Sur processeur, la vitesse compte autant que la qualité.
                Placement::Cpu => score /= 1.0 + m.active_params_b / 2.0,
                Placement::TooBig => continue,
            }
            out.push(Candidate {
                model_id: m.id.clone(),
                model_name: m.name.clone(),
                quant: q.name.clone(),
                file_size: q.size,
                placement,
                context: ctx,
                memory_needed: need,
                score,
            });
        }
    }
    out.sort_by(|a, b| {
        a.placement
            .priority()
            .cmp(&b.placement.priority())
            .then(b.score.total_cmp(&a.score))
    });
    out
}

/// Meilleure combinaison pour ce rôle, ou `None` si rien ne tient en mémoire.
pub fn recommend(catalog: &Catalog, profile: &MachineProfile, role: &str) -> Option<Candidate> {
    rank(catalog, profile, role, DEFAULT_CONTEXT)
        .into_iter()
        .next()
}

#[cfg(test)]
mod tests {
    use claudio_hw::Tier;

    use super::*;

    fn profile(vram_gb: f64, ram_gb: f64) -> MachineProfile {
        MachineProfile {
            tier: Tier::L,
            gpu_index: (vram_gb > 0.0).then_some(0),
            vram_budget: (vram_gb * GB as f64) as u64,
            ram_budget: (ram_gb * GB as f64) as u64,
            max_parallel_agents: 1,
            threads: 8,
        }
    }

    #[test]
    fn picks_bigger_models_with_more_vram() {
        let c = Catalog::builtin().unwrap();
        let small = recommend(&c, &profile(5.0, 8.0), "chat").unwrap();
        let big = recommend(&c, &profile(12.5, 32.0), "chat").unwrap();
        let huge = recommend(&c, &profile(23.0, 64.0), "chat").unwrap();
        assert_eq!(small.placement, Placement::Gpu);
        assert!(small.memory_needed <= 5 * GB);
        assert!(big.score > small.score);
        assert!(huge.score > big.score);
        assert!(big.memory_needed <= (12.5 * GB as f64) as u64);
    }

    #[test]
    fn cpu_only_machines_get_small_fast_models() {
        let c = Catalog::builtin().unwrap();
        let r = recommend(&c, &profile(0.0, 10.0), "chat").unwrap();
        assert_eq!(r.placement, Placement::Cpu);
        let m = c.get(&r.model_id).unwrap();
        assert!(m.active_params_b <= CPU_MAX_ACTIVE_PARAMS_B);
    }

    #[test]
    fn moe_models_run_hybrid_when_ram_allows() {
        let c = Catalog::builtin().unwrap();
        let ranked = rank(&c, &profile(12.0, 40.0), "chat", DEFAULT_CONTEXT);
        let moe = ranked
            .iter()
            .find(|r| r.model_id == "qwen3.6-35b-a3b")
            .unwrap();
        assert_eq!(moe.placement, Placement::Hybrid);
        assert_eq!(ranked[0].placement, Placement::Gpu);
    }

    #[test]
    fn generic_fit() {
        let p = profile(12.0, 32.0);
        let dense = ModelMeta::default();
        let small = estimate_fit(&p, 5_000_000_000, "Q4_K_M", DEFAULT_CONTEXT, &dense);
        assert_eq!(small.placement, Placement::Gpu);
        assert!((small.params_b - 8.3).abs() < 0.5);
        let huge = estimate_fit(&p, 80_000_000_000, "Q4_K_M", DEFAULT_CONTEXT, &dense);
        assert_eq!(huge.placement, Placement::TooBig);
        let moe = ModelMeta {
            params_b: Some(35.0),
            active_params_b: Some(3.0),
            moe: true,
        };
        let hybrid = estimate_fit(&p, 22_000_000_000, "UD-Q4_K_M", DEFAULT_CONTEXT, &moe);
        assert_eq!(hybrid.placement, Placement::Hybrid);
    }

    #[test]
    fn nothing_fits_on_tiny_machine() {
        let c = Catalog::builtin().unwrap();
        assert!(recommend(&c, &profile(0.0, 0.5), "chat").is_none());
    }
}
