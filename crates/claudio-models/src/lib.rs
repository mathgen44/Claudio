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
    install_files(
        http,
        registry,
        NewModel {
            model_id: &model.id,
            name: &model.name,
            repo: &model.repo,
            quant,
            params_b: Some(model.params_b),
            context_max: Some(model.context_max),
        },
        &[remote],
        hf_token,
        cancel,
        on_progress,
    )
    .await
}

/// Description d'un modèle à enregistrer.
pub struct NewModel<'a> {
    pub model_id: &'a str,
    pub name: &'a str,
    pub repo: &'a str,
    pub quant: &'a str,
    pub params_b: Option<f64>,
    pub context_max: Option<u32>,
}

/// Télécharge une variante GGUF (un ou plusieurs fichiers) et l'enregistre.
/// La progression couvre l'ensemble des fichiers.
pub async fn install_files(
    http: &reqwest::Client,
    registry: &Registry,
    model: NewModel<'_>,
    files: &[hf::RemoteFile],
    hf_token: Option<&str>,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&Progress),
) -> Result<InstalledModel> {
    anyhow::ensure!(!files.is_empty(), "aucun fichier à télécharger");
    let total: u64 = files.iter().map(|f| f.size).sum();
    tracing::info!(
        model = model.model_id,
        quant = model.quant,
        size = total,
        "téléchargement du modèle"
    );
    let mut done = 0;
    let mut paths = Vec::new();
    for f in files {
        let dest = registry.target_path(model.model_id, &f.path);
        let url = hf::resolve_url(model.repo, &f.path);
        download::download(
            http,
            download::Request {
                url: &url,
                dest: &dest,
                bearer: hf_token,
                sha256: f.sha256.as_deref(),
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
        .await?;
        done += f.size;
        paths.push(
            dest.strip_prefix(registry.root())
                .unwrap_or(&dest)
                .to_path_buf(),
        );
    }
    let installed = InstalledModel {
        key: model_key(model.model_id, model.quant),
        model_id: model.model_id.to_owned(),
        name: model.name.to_owned(),
        quant: model.quant.to_owned(),
        repo: model.repo.to_owned(),
        file: paths.remove(0),
        parts: paths,
        size: total,
        sha256: files[0].sha256.clone(),
        params_b: model.params_b,
        context_max: model.context_max,
        installed_at: Utc::now(),
    };
    registry.add(installed.clone())?;
    Ok(installed)
}
