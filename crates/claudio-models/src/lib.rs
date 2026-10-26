//! Modèles : catalogue, choix selon la machine, Hugging Face, téléchargements
//! et registre des modèles installés.

pub mod catalog;
pub mod download;
pub mod hf;
pub mod registry;
pub mod select;

use anyhow::{Context, Result};
use chrono::Utc;

pub use catalog::{Catalog, CatalogModel, Quant, model_key};
pub use download::{CancelToken, Progress};
pub use registry::{InstalledModel, Registry};
pub use select::{Candidate, Placement};

/// Télécharge et enregistre une quantification d'un modèle du catalogue.
///
/// L'empreinte SHA-256 est lue sur Hugging Face puis vérifiée ; un téléchargement
/// interrompu reprend là où il s'était arrêté.
pub async fn install(
    http: &reqwest::Client,
    registry: &Registry,
    model: &CatalogModel,
    quant: &str,
    hf_token: Option<&str>,
    cancel: &CancelToken,
    on_progress: impl FnMut(&Progress),
) -> Result<InstalledModel> {
    let q = model
        .quant(quant)
        .with_context(|| format!("quantification {quant} inconnue pour {}", model.id))?;
    let remote = hf::file_info(http, &model.repo, &q.file, hf_token).await?;
    let dest = registry.target_path(&model.id, &q.file);
    let url = hf::resolve_url(&model.repo, &q.file);
    tracing::info!(model = %model.id, quant, size = remote.size, "téléchargement du modèle");
    download::download(
        http,
        download::Request {
            url: &url,
            dest: &dest,
            bearer: hf_token,
            sha256: remote.sha256.as_deref(),
            expected_size: Some(remote.size),
        },
        cancel,
        on_progress,
    )
    .await?;
    let installed = InstalledModel {
        key: model_key(&model.id, quant),
        model_id: model.id.clone(),
        name: model.name.clone(),
        quant: quant.to_owned(),
        repo: model.repo.clone(),
        file: dest
            .strip_prefix(registry.root())
            .unwrap_or(&dest)
            .to_path_buf(),
        size: remote.size,
        sha256: remote.sha256,
        installed_at: Utc::now(),
    };
    registry.add(installed.clone())?;
    Ok(installed)
}
