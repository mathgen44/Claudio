//! Détection des cartes graphiques : DXGI pour toutes les cartes, enrichie par
//! NVML (bibliothèque du pilote NVIDIA) quand elle est présente.

use crate::{GB, GpuInfo, GpuVendor};

const VENDOR_NVIDIA: u32 = 0x10DE;
const VENDOR_AMD: u32 = 0x1002;
const VENDOR_INTEL: u32 = 0x8086;

pub fn detect() -> Vec<GpuInfo> {
    let mut gpus = dxgi::adapters();
    match nvml::devices() {
        Ok(nv) if !nv.is_empty() => {
            // NVML est plus précis pour les cartes NVIDIA : il remplace leurs entrées DXGI.
            gpus.retain(|g| g.vendor != GpuVendor::Nvidia);
            gpus.splice(0..0, nv);
        }
        Ok(_) => {}
        Err(e) => tracing::debug!("NVML indisponible : {e}"),
    }
    gpus
}

fn vendor(id: u32) -> GpuVendor {
    match id {
        VENDOR_NVIDIA => GpuVendor::Nvidia,
        VENDOR_AMD => GpuVendor::Amd,
        VENDOR_INTEL => GpuVendor::Intel,
        _ => GpuVendor::Other,
    }
}

/// Les puces intégrées n'ont qu'une petite réserve de mémoire « dédiée » prise sur la RAM.
fn looks_integrated(name: &str, vram_total: u64) -> bool {
    const IGPU_NAMES: &[&str] = &[
        "Radeon(TM) Graphics",
        "Radeon Graphics",
        "Radeon(TM) Vega",
        "Intel(R) UHD",
        "Intel(R) HD",
        "Iris",
        "Intel(R) Graphics",
        "Arc(TM) Graphics",
    ];
    vram_total < GB || IGPU_NAMES.iter().any(|n| name.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrated_heuristic() {
        assert!(looks_integrated("AMD Radeon(TM) Graphics", 2 * GB));
        assert!(looks_integrated("Intel(R) Arc(TM) Graphics", 2 * GB));
        assert!(!looks_integrated("Intel(R) Arc(TM) A770 Graphics", 16 * GB));
        assert!(!looks_integrated("AMD Radeon RX 7800 XT", 16 * GB));
    }
}

#[cfg(windows)]
mod dxgi {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
        DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIFactory1,
    };
    use windows::core::Interface;

    use super::*;

    pub fn adapters() -> Vec<GpuInfo> {
        let mut out = Vec::new();
        // SAFETY : appels COM standards ; les interfaces sont libérées à la sortie de portée.
        unsafe {
            let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
                return out;
            };
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                i += 1;
                let Ok(desc) = adapter.GetDesc1() else {
                    continue;
                };
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                    continue;
                }
                let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(128);
                let name = String::from_utf16_lossy(&desc.Description[..len]);
                let vram_total = desc.DedicatedVideoMemory as u64;
                let vram_free = adapter.cast::<IDXGIAdapter3>().ok().and_then(|a3| {
                    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                    a3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)
                        .ok()?;
                    Some(
                        info.Budget
                            .saturating_sub(info.CurrentUsage)
                            .min(vram_total),
                    )
                });
                let integrated = looks_integrated(&name, vram_total);
                out.push(GpuInfo {
                    name,
                    vendor: vendor(desc.VendorId),
                    vram_total,
                    vram_free,
                    driver_version: None,
                    cuda_version: None,
                    compute_capability: None,
                    integrated,
                });
            }
        }
        out
    }
}

#[cfg(not(windows))]
mod dxgi {
    use super::*;
    pub fn adapters() -> Vec<GpuInfo> {
        Vec::new()
    }
}

mod nvml {
    use nvml_wrapper::Nvml;

    use super::*;

    pub fn devices() -> anyhow::Result<Vec<GpuInfo>> {
        let nvml = Nvml::init()?;
        let driver_version = nvml.sys_driver_version().ok();
        let cuda_version = nvml.sys_cuda_driver_version().ok().map(|v| {
            let v = v as u32;
            (v / 1000, (v % 1000) / 10)
        });
        let mut out = Vec::new();
        for i in 0..nvml.device_count()? {
            let dev = nvml.device_by_index(i)?;
            let mem = dev.memory_info()?;
            let cc = dev.cuda_compute_capability().ok();
            out.push(GpuInfo {
                name: dev.name().unwrap_or_else(|_| "NVIDIA".into()),
                vendor: GpuVendor::Nvidia,
                vram_total: mem.total,
                vram_free: Some(mem.free),
                driver_version: driver_version.clone(),
                cuda_version,
                compute_capability: cc.map(|c| (c.major as u32, c.minor as u32)),
                integrated: false,
            });
        }
        Ok(out)
    }
}
