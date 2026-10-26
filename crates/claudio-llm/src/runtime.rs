//! Installation des moteurs llama.cpp officiels (publications GitHub de ggml-org),
//! en choisissant la variante adaptée à la carte graphique.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use claudio_hw::{GpuVendor, HardwareInfo, MachineProfile};
use claudio_models::download::{self, CancelToken, Progress};
use serde::{Deserialize, Serialize};

const RELEASES_URL: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=15";
const MANIFEST: &str = "runtime.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Backend {
    /// CUDA dans la version indiquée (ex. "13.4").
    Cuda {
        version: String,
    },
    Vulkan,
    Cpu,
}

impl Backend {
    pub fn label(&self) -> String {
        match self {
            Backend::Cuda { version } => format!("cuda-{version}"),
            Backend::Vulkan => "vulkan".into(),
            Backend::Cpu => "cpu".into(),
        }
    }

    pub fn uses_gpu(&self) -> bool {
        !matches!(self, Backend::Cpu)
    }
}

/// Moteur installé sur le disque.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledRuntime {
    pub tag: String,
    pub backend: Backend,
    pub dir: PathBuf,
}

impl InstalledRuntime {
    pub fn server_exe(&self) -> PathBuf {
        self.dir.join("llama-server.exe")
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

/// Préférence de moteur, du plus rapide au plus universel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preference {
    /// CUDA entre `min` (exigé par la puce) et `max` (supporté par le pilote).
    Cuda {
        max: (u32, u32),
        min: (u32, u32),
    },
    Vulkan,
    Cpu,
}

/// Liste ordonnée des moteurs à essayer pour cette machine.
pub fn preferences(
    hw: &HardwareInfo,
    profile: &MachineProfile,
    forced: Option<&str>,
) -> Vec<Preference> {
    match forced {
        Some("vulkan") => return vec![Preference::Vulkan, Preference::Cpu],
        Some("cpu") => return vec![Preference::Cpu],
        _ => {}
    }
    let mut prefs = Vec::new();
    if let Some(gpu) = profile.gpu_index.map(|i| &hw.gpus[i]) {
        if gpu.vendor == GpuVendor::Nvidia
            && let Some(max) = gpu.cuda_version
        {
            // Les puces Blackwell (capacité 12.x) exigent CUDA 12.8 ou plus.
            let min = if gpu.compute_capability.is_some_and(|c| c.0 >= 12) {
                (12, 8)
            } else {
                (0, 0)
            };
            prefs.push(Preference::Cuda { max, min });
        }
        if forced != Some("cuda") || prefs.is_empty() {
            prefs.push(Preference::Vulkan);
        }
    }
    prefs.push(Preference::Cpu);
    prefs
}

fn parse_cuda_version(name: &str) -> Option<(u32, u32)> {
    let rest = name.split("-cuda-").nth(1)?;
    let ver = rest.split('-').next()?;
    let (a, b) = ver.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Sélectionne, dans une publication, l'archive principale (et la bibliothèque
/// CUDA éventuelle) correspondant à la préférence.
fn pick_assets(release: &Release, pref: &Preference) -> Option<(Backend, Vec<Asset>)> {
    let main_prefix = format!("llama-{}-bin-win-", release.tag_name);
    let find = |suffix: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == format!("{main_prefix}{suffix}"))
            .cloned()
    };
    match pref {
        Preference::Cpu => find("cpu-x64.zip").map(|a| (Backend::Cpu, vec![a])),
        Preference::Vulkan => find("vulkan-x64.zip").map(|a| (Backend::Vulkan, vec![a])),
        Preference::Cuda { max, min } => {
            let best = release
                .assets
                .iter()
                .filter(|a| a.name.starts_with(&main_prefix) && a.name.ends_with("-x64.zip"))
                .filter_map(|a| parse_cuda_version(&a.name).map(|v| (v, a)))
                .filter(|(v, _)| v <= max && v >= min)
                .max_by_key(|(v, _)| *v)?;
            let (v, main) = best;
            let version = format!("{}.{}", v.0, v.1);
            let cudart = release
                .assets
                .iter()
                .find(|a| a.name == format!("cudart-llama-bin-win-cuda-{version}-x64.zip"))?;
            Some((
                Backend::Cuda { version },
                vec![main.clone(), cudart.clone()],
            ))
        }
    }
}

/// Moteur déjà installé, s'il existe.
pub fn installed(runtimes_dir: &Path) -> Option<InstalledRuntime> {
    let text = std::fs::read_to_string(runtimes_dir.join(MANIFEST)).ok()?;
    let rt: InstalledRuntime = serde_json::from_str(&text).ok()?;
    rt.server_exe().is_file().then_some(rt)
}

/// Événements d'installation remontés à l'interface.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "lowercase")]
pub enum InstallEvent {
    Resolving,
    Downloading { file: String, progress: Progress },
    Extracting,
}

/// Télécharge et installe le meilleur moteur pour la machine.
pub async fn install(
    http: &reqwest::Client,
    runtimes_dir: &Path,
    prefs: &[Preference],
    cancel: &CancelToken,
    mut on_event: impl FnMut(InstallEvent),
) -> Result<InstalledRuntime> {
    on_event(InstallEvent::Resolving);
    let releases: Vec<Release> = http
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("connexion à GitHub")?
        .error_for_status()?
        .json()
        .await?;

    let (tag, backend, assets) = prefs
        .iter()
        .find_map(|p| {
            releases
                .iter()
                .filter(|r| r.tag_name.starts_with('b'))
                .find_map(|r| pick_assets(r, p).map(|(b, a)| (r.tag_name.clone(), b, a)))
        })
        .context("aucune version de llama.cpp compatible trouvée")?;
    tracing::info!(%tag, backend = backend.label(), "installation de llama.cpp");

    let dir = runtimes_dir.join(format!("llama-{tag}-{}", backend.label()));
    if let Some(current) = installed(runtimes_dir)
        && current.dir == dir
    {
        return Ok(current);
    }
    let downloads = runtimes_dir.join("downloads");
    for asset in &assets {
        let Some(sha) = asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
        else {
            bail!(
                "{} n'a pas d'empreinte publiée : installation refusée",
                asset.name
            );
        };
        let zip_path = downloads.join(&asset.name);
        if !zip_path.is_file() {
            let name = asset.name.clone();
            download::download(
                http,
                download::Request {
                    url: &asset.browser_download_url,
                    dest: &zip_path,
                    bearer: None,
                    sha256: Some(sha),
                    expected_size: Some(asset.size),
                },
                cancel,
                |p| {
                    on_event(InstallEvent::Downloading {
                        file: name.clone(),
                        progress: p.clone(),
                    })
                },
            )
            .await?;
        }
        on_event(InstallEvent::Extracting);
        let (zip_path, dir) = (zip_path.clone(), dir.clone());
        tokio::task::spawn_blocking(move || extract(&zip_path, &dir)).await??;
    }

    let rt = InstalledRuntime { tag, backend, dir };
    if !rt.server_exe().is_file() {
        bail!("llama-server.exe absent de l'archive téléchargée");
    }
    std::fs::write(
        runtimes_dir.join(MANIFEST),
        serde_json::to_string_pretty(&rt)?,
    )?;
    std::fs::remove_dir_all(&downloads).ok();
    // Les versions précédentes ne servent plus.
    for entry in std::fs::read_dir(runtimes_dir)?.flatten() {
        let path = entry.path();
        if path.is_dir()
            && path != rt.dir
            && entry.file_name().to_string_lossy().starts_with("llama-")
        {
            std::fs::remove_dir_all(&path).ok();
        }
    }
    Ok(rt)
}

fn extract(zip_path: &Path, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).context("archive illisible")?;
    // `extract` refuse les chemins qui sortiraient du dossier cible.
    archive.extract(dir).context("extraction de l'archive")?;
    // Certaines archives rangent les binaires dans un sous-dossier : on les remonte.
    if !dir.join("llama-server.exe").exists() {
        for entry in std::fs::read_dir(dir)? {
            let sub = entry?.path();
            if sub.join("llama-server.exe").is_file() {
                for f in std::fs::read_dir(&sub)? {
                    let f = f?.path();
                    std::fs::rename(&f, dir.join(f.file_name().unwrap()))?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use claudio_hw::{CpuInfo, GpuInfo};

    use super::*;

    fn release() -> Release {
        let a = |n: &str| Asset {
            name: n.into(),
            browser_download_url: format!("https://x/{n}"),
            size: 1,
            digest: Some("sha256:00".into()),
        };
        Release {
            tag_name: "b100".into(),
            assets: vec![
                a("cudart-llama-bin-win-cuda-12.4-x64.zip"),
                a("cudart-llama-bin-win-cuda-13.4-x64.zip"),
                a("llama-b100-bin-win-cpu-x64.zip"),
                a("llama-b100-bin-win-cuda-12.4-x64.zip"),
                a("llama-b100-bin-win-cuda-13.4-x64.zip"),
                a("llama-b100-bin-win-cuda-13.4-arm64.zip"),
                a("llama-b100-bin-win-vulkan-x64.zip"),
            ],
        }
    }

    #[test]
    fn picks_highest_supported_cuda() {
        let r = release();
        let (b, assets) = pick_assets(
            &r,
            &Preference::Cuda {
                max: (13, 4),
                min: (0, 0),
            },
        )
        .unwrap();
        assert_eq!(
            b,
            Backend::Cuda {
                version: "13.4".into()
            }
        );
        assert_eq!(assets.len(), 2);
        let (b, _) = pick_assets(
            &r,
            &Preference::Cuda {
                max: (12, 9),
                min: (0, 0),
            },
        )
        .unwrap();
        assert_eq!(
            b,
            Backend::Cuda {
                version: "12.4".into()
            }
        );
        // Blackwell avec un pilote trop ancien pour CUDA 13 : pas de CUDA compatible.
        assert!(
            pick_assets(
                &r,
                &Preference::Cuda {
                    max: (12, 9),
                    min: (12, 8)
                }
            )
            .is_none()
        );
        assert_eq!(
            pick_assets(&r, &Preference::Vulkan).unwrap().0,
            Backend::Vulkan
        );
    }

    #[test]
    fn preferences_by_gpu() {
        let gpu = |vendor, cuda: Option<(u32, u32)>| GpuInfo {
            name: "g".into(),
            vendor,
            vram_total: 16 << 30,
            vram_free: None,
            driver_version: None,
            cuda_version: cuda,
            compute_capability: Some((12, 0)),
            integrated: false,
        };
        let hw = |gpus| HardwareInfo {
            cpu: CpuInfo {
                name: "c".into(),
                physical_cores: 8,
                logical_cores: 16,
                avx2: true,
                avx512: false,
            },
            ram_total: 32 << 30,
            ram_available: 16 << 30,
            gpus,
            os: "w".into(),
        };
        let nv = hw(vec![gpu(GpuVendor::Nvidia, Some((13, 4)))]);
        let p = preferences(&nv, &nv.profile(), None);
        assert!(matches!(
            p[0],
            Preference::Cuda {
                max: (13, 4),
                min: (12, 8)
            }
        ));
        assert_eq!(p[1], Preference::Vulkan);
        let amd = hw(vec![gpu(GpuVendor::Amd, None)]);
        assert_eq!(
            preferences(&amd, &amd.profile(), None),
            vec![Preference::Vulkan, Preference::Cpu]
        );
        let none = hw(vec![]);
        assert_eq!(
            preferences(&none, &none.profile(), None),
            vec![Preference::Cpu]
        );
        assert_eq!(
            preferences(&nv, &nv.profile(), Some("cpu")),
            vec![Preference::Cpu]
        );
    }
}
