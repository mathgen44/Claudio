use std::path::PathBuf;

use anyhow::{Context, Result};

/// Emplacements des fichiers de Claudio sur la machine.
///
/// Par défaut : configuration dans `%APPDATA%\Claudio`, données (base, journaux,
/// modèles, moteurs d'inférence) dans `%LOCALAPPDATA%\Claudio`. La variable
/// `CLAUDIO_HOME` regroupe tout dans un seul dossier (mode portable, tests).
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Result<Self> {
        if let Some(home) = std::env::var_os("CLAUDIO_HOME") {
            return Ok(Self::in_dir(PathBuf::from(home)));
        }
        let base = directories::BaseDirs::new().context("dossiers utilisateur introuvables")?;
        Ok(Self {
            config_dir: base.config_dir().join("Claudio"),
            data_dir: base.data_local_dir().join("Claudio"),
        })
    }

    pub fn in_dir(root: PathBuf) -> Self {
        Self {
            config_dir: root.clone(),
            data_dir: root,
        }
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.logs_dir(),
            &self.default_models_dir(),
            &self.runtimes_dir(),
            &self.workspace_dir(),
        ] {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("création de {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("claudio.db")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn default_models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }

    pub fn runtimes_dir(&self) -> PathBuf {
        self.data_dir.join("runtimes")
    }

    /// Espace de travail des outils : toujours accessible, inscriptible par le bac à sable.
    pub fn workspace_dir(&self) -> PathBuf {
        self.data_dir.join("workspace")
    }
}
