//! Éléments téléchargeables de la voix : moteurs (whisper.cpp, Piper) et modèles.
//! Les adresses et empreintes sont figées : rien d'autre n'est installé.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use claudio_hw::Tier;
use claudio_models::download::{self, CancelToken, Progress};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Moteur de reconnaissance vocale (whisper.cpp).
    SttRuntime,
    /// Modèle de reconnaissance vocale.
    Stt,
    /// Moteur de synthèse Piper.
    TtsRuntime,
    /// Voix Piper.
    Voice,
}

#[derive(Debug, Clone, Copy)]
pub struct FileSpec {
    pub url: &'static str,
    pub name: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Component {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: Kind,
    /// Archive zip à extraire (moteurs) ou fichiers posés tels quels (modèles).
    pub archive: bool,
    pub files: &'static [FileSpec],
}

impl Component {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Dossier d'installation.
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(self.id)
    }

    pub fn is_installed(&self, root: &Path) -> bool {
        let dir = self.dir(root);
        if self.archive {
            dir.join(MARKER).is_file()
        } else {
            self.files
                .iter()
                .all(|f| dir.join(f.name).metadata().is_ok_and(|m| m.len() == f.size))
        }
    }

    /// Fichier principal (premier fichier d'un modèle).
    pub fn main_file(&self, root: &Path) -> PathBuf {
        self.dir(root).join(self.files[0].name)
    }
}

const MARKER: &str = ".installed";

pub const WHISPER: &str = "whisper-runtime";
pub const PIPER: &str = "piper-runtime";
/// Petit modèle servant à guetter le mot d'éveil.
pub const WAKE_MODEL: &str = "stt-base";

pub const ALL: &[Component] = &[
    Component {
        id: WHISPER,
        label: "Moteur whisper.cpp",
        description: "Reconnaissance vocale locale (processeur), version b5454.",
        kind: Kind::SttRuntime,
        archive: true,
        files: &[FileSpec {
            url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip",
            name: "whisper-bin-x64.zip",
            sha256: "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb",
            size: 8_928_640,
        }],
    },
    Component {
        id: WAKE_MODEL,
        label: "Whisper Base",
        description: "Très rapide : sert au mot d'éveil, ou à la dictée sur les petites machines.",
        kind: Kind::Stt,
        archive: false,
        files: &[FileSpec {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q5_1.bin",
            name: "ggml-base-q5_1.bin",
            sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
            size: 59_707_625,
        }],
    },
    Component {
        id: "stt-small",
        label: "Whisper Small",
        description: "Bon compromis vitesse et qualité.",
        kind: Kind::Stt,
        archive: false,
        files: &[FileSpec {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
            name: "ggml-small-q5_1.bin",
            sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
            size: 190_085_487,
        }],
    },
    Component {
        id: "stt-turbo",
        label: "Whisper Large v3 Turbo",
        description: "Meilleure transcription, plus lente sur les petits processeurs.",
        kind: Kind::Stt,
        archive: false,
        files: &[FileSpec {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            name: "ggml-large-v3-turbo-q5_0.bin",
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
            size: 574_041_195,
        }],
    },
    Component {
        id: PIPER,
        label: "Moteur Piper",
        description: "Synthèse vocale neuronale locale, plus naturelle que les voix Windows.",
        kind: Kind::TtsRuntime,
        archive: true,
        files: &[FileSpec {
            url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip",
            name: "piper_windows_amd64.zip",
            sha256: "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea",
            size: 22_477_236,
        }],
    },
    Component {
        id: "piper-siwis",
        label: "Siwis (voix féminine)",
        description: "Voix Piper française.",
        kind: Kind::Voice,
        archive: false,
        files: &[
            FileSpec {
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/fr/fr_FR/siwis/medium/fr_FR-siwis-medium.onnx",
                name: "fr_FR-siwis-medium.onnx",
                sha256: "641d1ab097da2b81128c076810edb052b385decc8be3381814802a64a73baf99",
                size: 63_201_294,
            },
            FileSpec {
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/fr/fr_FR/siwis/medium/fr_FR-siwis-medium.onnx.json",
                name: "fr_FR-siwis-medium.onnx.json",
                sha256: "39479916c2db192b5ac9764daddd0c744d83e023ad890c6976c0633ae4df8959",
                size: 4_875,
            },
        ],
    },
    Component {
        id: "piper-tom",
        label: "Tom (voix masculine)",
        description: "Voix Piper française.",
        kind: Kind::Voice,
        archive: false,
        files: &[
            FileSpec {
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/fr/fr_FR/tom/medium/fr_FR-tom-medium.onnx",
                name: "fr_FR-tom-medium.onnx",
                sha256: "bf65074ccdeeeeaa832e75edb1c0a513c01c9a972bdf085ff8a6e71ea234fd41",
                size: 63_511_038,
            },
            FileSpec {
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/fr/fr_FR/tom/medium/fr_FR-tom-medium.onnx.json",
                name: "fr_FR-tom-medium.onnx.json",
                sha256: "2f7f885ad5a0aad802e3cc24e4f57239febdcb142b4876de5d238094674361cc",
                size: 4_959,
            },
        ],
    },
];

pub fn get(id: &str) -> Option<&'static Component> {
    ALL.iter().find(|c| c.id == id)
}

/// Modèle de dictée conseillé selon la puissance de la machine (tout tourne sur
/// le processeur pour laisser la carte graphique au modèle de conversation).
pub fn default_stt(tier: Tier) -> &'static str {
    match tier {
        Tier::S => WAKE_MODEL,
        Tier::M => "stt-small",
        Tier::L | Tier::XL => "stt-turbo",
    }
}

/// Télécharge, vérifie et installe un élément.
pub async fn install(
    http: &reqwest::Client,
    root: &Path,
    c: &Component,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&Progress),
) -> Result<()> {
    if c.is_installed(root) {
        return Ok(());
    }
    let dir = c.dir(root);
    let staging = root.join("downloads");
    let total = c.size();
    let mut done = 0;
    for f in c.files {
        let dest = if c.archive {
            staging.join(f.name)
        } else {
            dir.join(f.name)
        };
        download::download(
            http,
            download::Request {
                url: f.url,
                dest: &dest,
                bearer: None,
                sha256: Some(f.sha256),
                expected_size: Some(f.size),
            },
            cancel,
            |p| {
                on_progress(&Progress {
                    downloaded: done + p.downloaded,
                    total: Some(total),
                    bytes_per_sec: p.bytes_per_sec,
                })
            },
        )
        .await
        .with_context(|| format!("téléchargement de {}", f.name))?;
        done += f.size;
        if c.archive {
            let target = dir.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                let dir = target;
                if dir.exists() {
                    std::fs::remove_dir_all(&dir)?;
                }
                std::fs::create_dir_all(&dir)?;
                let mut archive = zip::ZipArchive::new(std::fs::File::open(&dest)?)
                    .context("archive illisible")?;
                // `extract` refuse les chemins qui sortiraient du dossier cible.
                archive.extract(&dir).context("extraction de l'archive")?;
                std::fs::remove_file(&dest).ok();
                Ok(())
            })
            .await??;
            std::fs::write(dir.join(MARKER), c.id)?;
        }
    }
    std::fs::remove_dir(&staging).ok();
    Ok(())
}

pub fn remove(root: &Path, c: &Component) -> Result<()> {
    let dir = c.dir(root);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Cherche un exécutable dans le dossier d'un moteur (les archives le rangent
/// parfois dans un sous-dossier).
pub fn find_exe(dir: &Path, name: &str) -> Result<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        if depth == 0 {
            return None;
        }
        std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter(|e| e.path().is_dir())
            .find_map(|e| walk(&e.path(), name, depth - 1))
    }
    match walk(dir, name, 2) {
        Some(p) => Ok(p),
        None => bail!("{name} introuvable dans {}", dir.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        for c in ALL {
            assert!(!c.files.is_empty());
            for f in c.files {
                assert_eq!(f.sha256.len(), 64, "{}", f.name);
                assert!(f.url.starts_with("https://") && f.url.ends_with(f.name));
                assert!(f.size > 0);
            }
            assert_eq!(get(c.id).unwrap().id, c.id);
        }
        for t in [Tier::S, Tier::M, Tier::L, Tier::XL] {
            assert_eq!(get(default_stt(t)).unwrap().kind, Kind::Stt);
        }
    }

    #[test]
    fn installation_state_and_exe_lookup() {
        let root = tempfile::tempdir().unwrap();
        let base = get(WAKE_MODEL).unwrap();
        assert!(!base.is_installed(root.path()));
        let whisper = get(WHISPER).unwrap();
        let sub = whisper.dir(root.path()).join("Release");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("whisper-server.exe"), b"").unwrap();
        assert_eq!(
            find_exe(&whisper.dir(root.path()), "whisper-server.exe").unwrap(),
            sub.join("whisper-server.exe")
        );
        assert!(
            !whisper.is_installed(root.path()),
            "sans marqueur, pas installé"
        );
        remove(root.path(), whisper).unwrap();
        assert!(find_exe(&whisper.dir(root.path()), "whisper-server.exe").is_err());
    }
}
