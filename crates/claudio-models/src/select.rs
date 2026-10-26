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
    /// Exécution sur le processeur et la mémoire vive.
    Cpu,
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
            let placement = if profile.gpu_index.is_some() && need <= profile.vram_budget {
                Placement::Gpu
            } else if need <= profile.ram_budget && m.active_params_b <= CPU_MAX_ACTIVE_PARAMS_B {
                Placement::Cpu
            } else {
                continue;
            };
            let mut score = m.quality * quant_factor(&q.name);
            if placement == Placement::Cpu {
                // Sur processeur, la vitesse compte autant que la qualité.
                score /= 1.0 + m.active_params_b / 2.0;
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
        (b.placement == Placement::Gpu)
            .cmp(&(a.placement == Placement::Gpu))
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
    fn nothing_fits_on_tiny_machine() {
        let c = Catalog::builtin().unwrap();
        assert!(recommend(&c, &profile(0.0, 0.5), "chat").is_none());
    }
}
