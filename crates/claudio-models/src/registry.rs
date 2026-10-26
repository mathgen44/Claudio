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

    /// Retire un modèle du registre et supprime son fichier.
    pub fn remove(&self, key: &str) -> Result<()> {
        let mut m = self.load()?;
        if let Some(found) = m.models.iter().find(|x| x.key == key) {
            for rel in std::iter::once(&found.file).chain(&found.parts) {
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
            installed_at: Utc::now(),
        })
        .unwrap();
        assert_eq!(reg.list().unwrap().len(), 1);
        assert!(reg.get("m:Q4").unwrap().is_some());
        reg.remove("m:Q4").unwrap();
        assert!(reg.list().unwrap().is_empty());
        assert!(!target.exists());
    }
}
