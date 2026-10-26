//! Recherche et installation de modèles Hugging Face hors catalogue.

use anyhow::{Context, Result, bail};
use claudio_models::hf::RepoInfo;
use claudio_models::hf::{self, GgufVariant, ModelSummary};
use claudio_models::select::{DEFAULT_CONTEXT, Fit, ModelMeta, estimate_fit};
use claudio_models::{InstalledModel, NewModel, Progress, model_key};
use serde::Serialize;

use crate::Claudio;

/// Licences dont l'usage ne demande pas de lecture particulière.
const PERMISSIVE: &[&str] = &[
    "apache-2.0",
    "mit",
    "bsd-3-clause",
    "bsd-2-clause",
    "cc-by-4.0",
];

#[derive(Debug, Clone, Serialize)]
pub struct VariantView {
    #[serde(flatten)]
    pub variant: GgufVariant,
    pub fit: Fit,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoView {
    pub id: String,
    pub url: String,
    pub license: Option<String>,
    /// La licence doit être lue et acceptée avant le téléchargement.
    pub license_needs_review: bool,
    pub gated: bool,
    pub downloads: u64,
    pub likes: u64,
    pub variants: Vec<VariantView>,
}

fn meta(info: &RepoInfo) -> ModelMeta {
    ModelMeta {
        params_b: info.params_total.map(|p| p as f64 / 1e9),
        active_params_b: hf::active_params_from_name(&info.id),
        moe: info.is_moe(),
    }
}

impl Claudio {
    pub async fn hf_search(&self, query: &str) -> Result<Vec<ModelSummary>> {
        let query = query.trim();
        if query.len() < 2 {
            bail!("tapez au moins deux caractères");
        }
        hf::search(&self.http, query, self.hf_token().as_deref()).await
    }

    pub async fn hf_repo(&self, repo: &str) -> Result<RepoView> {
        let info = hf::repo_info(&self.http, repo.trim(), self.hf_token().as_deref()).await?;
        let profile = self.hardware().profile();
        let ctx = self.config().context_size.unwrap_or(DEFAULT_CONTEXT);
        let installed: Vec<String> = self.registry().list()?.into_iter().map(|m| m.key).collect();
        let meta = meta(&info);
        let license_needs_review = !info
            .license
            .as_deref()
            .is_some_and(|l| PERMISSIVE.contains(&l.to_ascii_lowercase().as_str()));
        Ok(RepoView {
            url: hf::repo_url(&info.id),
            license_needs_review,
            gated: info.gated,
            downloads: info.downloads,
            likes: info.likes,
            license: info.license,
            variants: info
                .variants
                .into_iter()
                .map(|v| VariantView {
                    fit: estimate_fit(&profile, v.size, &v.quant, ctx, &meta),
                    installed: installed.contains(&model_key(&info.id, &v.quant)),
                    variant: v,
                })
                .collect(),
            id: info.id,
        })
    }

    /// Télécharge une variante GGUF d'un dépôt quelconque.
    pub async fn install_hf(
        &self,
        repo: &str,
        quant: &str,
        on_progress: impl FnMut(&Progress),
    ) -> Result<InstalledModel> {
        let token = self.hf_token();
        let info = hf::repo_info(&self.http, repo.trim(), token.as_deref()).await?;
        let variant = info
            .variants
            .iter()
            .find(|v| v.quant == quant)
            .with_context(|| format!("variante {quant} introuvable dans {repo}"))?;
        let fit = estimate_fit(
            &self.hardware().profile(),
            variant.size,
            quant,
            DEFAULT_CONTEXT,
            &meta(&info),
        );
        let name = info
            .id
            .rsplit('/')
            .next()
            .unwrap_or(&info.id)
            .trim_end_matches("-GGUF");
        let key = model_key(&info.id, quant);
        let cancel = self.begin_download(&key)?;
        let res = claudio_models::install_files(
            &self.http,
            &self.registry(),
            NewModel {
                model_id: &info.id,
                name,
                repo: &info.id,
                quant,
                params_b: Some(fit.params_b),
                context_max: info.context_length,
            },
            &variant.files,
            token.as_deref(),
            &cancel,
            on_progress,
        )
        .await;
        self.end_download(&key);
        self.after_install(res?)
    }
}
