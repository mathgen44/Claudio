//! Profileur matériel : détecte CPU, mémoire et cartes graphiques, puis en déduit
//! un profil de machine qui guide le choix des modèles et du parallélisme.

use serde::{Deserialize, Serialize};

mod gpu;

pub const GB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareInfo {
    pub cpu: CpuInfo,
    pub ram_total: u64,
    pub ram_available: u64,
    pub gpus: Vec<GpuInfo>,
    pub os: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub name: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub avx2: bool,
    pub avx512: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: GpuVendor,
    pub vram_total: u64,
    /// Mémoire vidéo libre au moment de la mesure, si le pilote la fournit.
    pub vram_free: Option<u64>,
    pub driver_version: Option<String>,
    /// Version CUDA maximale supportée par le pilote (NVIDIA), ex. (13, 4).
    pub cuda_version: Option<(u32, u32)>,
    /// Capacité de calcul CUDA, ex. (12, 0) pour Blackwell grand public.
    pub compute_capability: Option<(u32, u32)>,
    pub integrated: bool,
}

/// Catégorie de machine, de la plus modeste à la plus puissante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    S,
    M,
    L,
    XL,
}

/// Synthèse exploitable par le reste de Claudio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineProfile {
    pub tier: Tier,
    /// Carte graphique retenue pour l'inférence (index dans `HardwareInfo::gpus`).
    pub gpu_index: Option<usize>,
    /// Mémoire vidéo utilisable pour un modèle, marge système déduite.
    pub vram_budget: u64,
    /// Mémoire vive utilisable pour un modèle (repli CPU ou déchargement partiel).
    pub ram_budget: u64,
    /// Nombre d'agents pouvant travailler en parallèle.
    pub max_parallel_agents: usize,
    /// Threads CPU conseillés pour l'inférence.
    pub threads: usize,
}

/// Marge laissée au bureau Windows et aux autres applications sur la carte graphique.
const VRAM_RESERVE: u64 = GB * 3 / 4;

impl HardwareInfo {
    pub fn detect() -> Self {
        use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_memory(MemoryRefreshKind::everything())
                .with_cpu(CpuRefreshKind::nothing()),
        );
        let cpu_name = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_owned())
            .unwrap_or_default();
        let logical = sys.cpus().len().max(1);
        let physical = System::physical_core_count().unwrap_or(logical);

        #[cfg(target_arch = "x86_64")]
        let (avx2, avx512) = (
            std::arch::is_x86_feature_detected!("avx2"),
            std::arch::is_x86_feature_detected!("avx512f"),
        );
        #[cfg(not(target_arch = "x86_64"))]
        let (avx2, avx512) = (false, false);

        let info = Self {
            cpu: CpuInfo {
                name: cpu_name,
                physical_cores: physical,
                logical_cores: logical,
                avx2,
                avx512,
            },
            ram_total: sys.total_memory(),
            ram_available: sys.available_memory(),
            gpus: gpu::detect(),
            os: System::long_os_version().unwrap_or_else(|| "inconnu".into()),
        };
        tracing::info!(?info, "matériel détecté");
        info
    }

    /// Carte la plus adaptée à l'inférence : dédiée d'abord, puis la plus grosse mémoire.
    pub fn best_gpu(&self) -> Option<usize> {
        self.gpus
            .iter()
            .enumerate()
            .filter(|(_, g)| g.vram_total >= 2 * GB)
            .max_by_key(|(_, g)| (!g.integrated, g.vendor == GpuVendor::Nvidia, g.vram_total))
            .map(|(i, _)| i)
    }

    pub fn profile(&self) -> MachineProfile {
        let gpu_index = self.best_gpu().filter(|&i| !self.gpus[i].integrated);
        let vram_budget = gpu_index
            .map(|i| {
                let g = &self.gpus[i];
                let usable = g.vram_free.unwrap_or(g.vram_total).min(g.vram_total);
                usable.saturating_sub(VRAM_RESERVE)
            })
            .unwrap_or(0);
        let ram_budget = (self.ram_available.saturating_sub(2 * GB)).min(self.ram_total * 6 / 10);

        let vram_total = gpu_index.map(|i| self.gpus[i].vram_total).unwrap_or(0);
        let tier = match vram_total {
            v if v >= 20 * GB => Tier::XL,
            v if v >= 11 * GB => Tier::L,
            v if v >= 6 * GB => Tier::M,
            _ => Tier::S,
        };
        let max_parallel_agents = match tier {
            Tier::S => 1,
            Tier::M => 2,
            Tier::L => 3,
            Tier::XL => 4,
        };
        MachineProfile {
            tier,
            gpu_index,
            vram_budget,
            ram_budget,
            max_parallel_agents,
            threads: self.cpu.physical_cores.clamp(1, 16),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(gpus: Vec<GpuInfo>) -> HardwareInfo {
        HardwareInfo {
            cpu: CpuInfo {
                name: "cpu".into(),
                physical_cores: 8,
                logical_cores: 16,
                avx2: true,
                avx512: false,
            },
            ram_total: 32 * GB,
            ram_available: 24 * GB,
            gpus,
            os: "test".into(),
        }
    }

    fn gpu(vendor: GpuVendor, vram_gb: u64, integrated: bool) -> GpuInfo {
        GpuInfo {
            name: "gpu".into(),
            vendor,
            vram_total: vram_gb * GB,
            vram_free: None,
            driver_version: None,
            cuda_version: None,
            compute_capability: None,
            integrated,
        }
    }

    #[test]
    fn tiers() {
        assert_eq!(machine(vec![]).profile().tier, Tier::S);
        assert_eq!(
            machine(vec![gpu(GpuVendor::Nvidia, 8, false)])
                .profile()
                .tier,
            Tier::M
        );
        assert_eq!(
            machine(vec![gpu(GpuVendor::Nvidia, 16, false)])
                .profile()
                .tier,
            Tier::L
        );
        assert_eq!(
            machine(vec![gpu(GpuVendor::Amd, 24, false)]).profile().tier,
            Tier::XL
        );
    }

    #[test]
    fn prefers_discrete_gpu_and_ignores_igpu() {
        let hw = machine(vec![
            gpu(GpuVendor::Intel, 2, true),
            gpu(GpuVendor::Nvidia, 12, false),
        ]);
        assert_eq!(hw.best_gpu(), Some(1));
        let p = hw.profile();
        assert_eq!(p.gpu_index, Some(1));
        assert_eq!(p.vram_budget, 12 * GB - VRAM_RESERVE);

        let igpu_only = machine(vec![gpu(GpuVendor::Intel, 4, true)]);
        assert_eq!(igpu_only.profile().gpu_index, None);
        assert_eq!(igpu_only.profile().vram_budget, 0);
    }

    #[test]
    fn detect_runs() {
        let hw = HardwareInfo::detect();
        assert!(hw.ram_total > 0);
        assert!(hw.cpu.logical_cores > 0);
    }
}
