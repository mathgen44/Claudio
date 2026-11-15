//! Registre des modèles installés, stocké dans `installed.json` à la racine du
//! dossier des modèles.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledModel {
    /// `<id du catalogue>:<quantification>`.
    pub key: String,
    pub model_id: String,
    pub name: String,
    pub quant: String,
    pub repo: String,
    /// Chemin relatif au dossier des modèles (premier fichier si le modèle est découpé).
    pub file: PathBuf,
    /// Fichiers suivants d'un modèle découpé.
    #[serde(default)]
    pub parts: Vec<PathBuf>,
    pub size: u64,
    pub sha256: Option<String>,
    /// Nombre de paramètres (milliards), connu ou estimé.
    #[serde(default)]
    pub params_b: Option<f64>,
    #[serde(default)]
    pub context_max: Option<u32>,
    /// Projecteur de vision (chemin relatif au dossier des modèles) : le modèle voit les images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj_size: Option<u64>,
    pub installed_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    models: Vec<InstalledModel>,
}

pub struct Registry {
    root: PathBuf,
}

impl Registry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join("installed.json")
    }

    fn load(&self) -> Result<Manifest> {
        match std::fs::read_to_string(self.manifest_path()) {
            Ok(t) => Ok(serde_json::from_str(&t).context("installed.json illisible")?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, m: &Manifest) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let tmp = self.root.join("installed.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(m)?)?;
        std::fs::rename(&tmp, self.manifest_path())?;
        Ok(())
    }

    /// Modèles installés dont le fichier est toujours présent.
    pub fn list(&self) -> Result<Vec<InstalledModel>> {
        Ok(self
            .load()?
            .models
            .into_iter()
            .filter(|m| self.path_of(m).is_file())
            .collect())
    }

    pub fn get(&self, key: &str) -> Result<Option<InstalledModel>> {
        Ok(self.list()?.into_iter().find(|m| m.key == key))
    }

    pub fn path_of(&self, m: &InstalledModel) -> PathBuf {
        self.root.join(&m.file)
    }

    /// Projecteur de vision du modèle, s'il est présent sur le disque.
    pub fn mmproj_of(&self, m: &InstalledModel) -> Option<PathBuf> {
        m.mmproj
            .as_ref()
            .map(|p| self.root.join(p))
            .filter(|p| p.is_file())
    }

    /// Emplacement de destination d'un fichier de modèle.
    pub fn target_path(&self, model_id: &str, file: &str) -> PathBuf {
        self.root.join(model_id).join(file)
    }

    pub fn add(&self, model: InstalledModel) -> Result<()> {
        let mut m = self.load()?;
        m.models.retain(|x| x.key != model.key);
        m.models.push(model);
        self.save(&m)
    }

    /// Retire un modèle du registre et supprime son fichier (et son projecteur
    /// de vision, s'il ne sert plus à aucune autre quantification).
    pub fn remove(&self, key: &str) -> Result<()> {
        let mut m = self.load()?;
        if let Some(found) = m.models.iter().find(|x| x.key == key) {
            let shared = |p: &PathBuf| {
                m.models
                    .iter()
                    .any(|o| o.key != key && o.mmproj.as_ref() == Some(p))
            };
            let mmproj = found.mmproj.as_ref().filter(|p| !shared(p));
            for rel in std::iter::once(&found.file)
                .chain(&found.parts)
                .chain(mmproj)
            {
                let path = self.root.join(rel);
                if path.exists() {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("suppression de {}", path.display()))?;
                }
            }
        }
        m.models.retain(|x| x.key != key);
        self.save(&m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_remove() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path());
        let target = reg.target_path("m", "m-Q4.gguf");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"gguf").unwrap();
        reg.add(InstalledModel {
            key: "m:Q4".into(),
            model_id: "m".into(),
            name: "M".into(),
            quant: "Q4".into(),
            repo: "r/m".into(),
            file: PathBuf::from("m").join("m-Q4.gguf"),
            parts: vec![],
            size: 4,
            sha256: None,
            params_b: None,
            context_max: None,
            mmproj: None,
            mmproj_size: None,
            installed_at: Utc::now(),
        })
        .unwrap();
        assert_eq!(reg.list().unwrap().len(), 1);
        assert!(reg.get("m:Q4").unwrap().is_some());
        reg.remove("m:Q4").unwrap();
        assert!(reg.list().unwrap().is_empty());
        assert!(!target.exists());
    }

    #[test]
    fn shared_projector_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path());
        let proj = PathBuf::from("m").join("mmproj-F16.gguf");
        std::fs::create_dir_all(dir.path().join("m")).unwrap();
        std::fs::write(dir.path().join(&proj), b"p").unwrap();
        for q in ["Q4", "Q8"] {
            let file = PathBuf::from("m").join(format!("m-{q}.gguf"));
            std::fs::write(dir.path().join(&file), b"gguf").unwrap();
            reg.add(InstalledModel {
                key: format!("m:{q}"),
                model_id: "m".into(),
                name: "M".into(),
                quant: q.into(),
                repo: "r/m".into(),
                file,
                parts: vec![],
                size: 4,
                sha256: None,
                params_b: None,
                context_max: None,
                mmproj: Some(proj.clone()),
                mmproj_size: Some(1),
                installed_at: Utc::now(),
            })
            .unwrap();
        }
        let m = reg.get("m:Q4").unwrap().unwrap();
        assert!(reg.mmproj_of(&m).is_some());
        reg.remove("m:Q4").unwrap();
        assert!(dir.path().join(&proj).exists(), "encore utilisé par Q8");
        reg.remove("m:Q8").unwrap();
        assert!(!dir.path().join(&proj).exists());
    }
}
