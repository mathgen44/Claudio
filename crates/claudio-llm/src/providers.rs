//! Fournisseurs de modèles distants : adresses par défaut, liste des modèles et prix.

use anyhow::{Context, Result, bail};
use claudio_core::config::{ProviderConfig, ProviderKind};
use serde::Serialize;
use serde_json::Value;

use crate::anthropic;
use crate::chat::{Api, Endpoint, http_error};

pub fn default_base_url(kind: ProviderKind) -> Option<&'static str> {
    Some(match kind {
        ProviderKind::Anthropic => anthropic::DEFAULT_BASE_URL,
        ProviderKind::OpenAi => "https://api.openai.com/v1",
        ProviderKind::Mistral => "https://api.mistral.ai/v1",
        ProviderKind::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
        ProviderKind::OpenRouter => "https://openrouter.ai/api/v1",
        ProviderKind::Custom => return None,
    })
}

pub fn display_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "Anthropic (Claude)",
        ProviderKind::OpenAi => "OpenAI",
        ProviderKind::Mistral => "Mistral AI",
        ProviderKind::Gemini => "Google Gemini",
        ProviderKind::OpenRouter => "OpenRouter",
        ProviderKind::Custom => "Compatible OpenAI",
    }
}

pub fn base_url(p: &ProviderConfig) -> Result<String> {
    p.base_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| default_base_url(p.kind).map(str::to_owned))
        .with_context(|| format!("adresse de l'API manquante pour {}", p.name))
}

/// Les adresses distantes doivent être chiffrées ; seul un serveur local peut être en HTTP.
pub fn validate_base_url(url: &str) -> Result<()> {
    let u = reqwest::Url::parse(url).context("adresse invalide")?;
    let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match u.scheme() {
        "https" => Ok(()),
        "http" if local => Ok(()),
        _ => bail!("l'adresse doit commencer par https:// (http:// seulement pour localhost)"),
    }
}

pub fn endpoint(p: &ProviderConfig, model: &str, api_key: Option<String>) -> Result<Endpoint> {
    let api = match p.kind {
        ProviderKind::Anthropic => Api::Anthropic,
        // Ces deux-là renvoient l'usage en fin de flux sur demande.
        ProviderKind::OpenAi | ProviderKind::OpenRouter => Api::OpenAi { stream_usage: true },
        _ => Api::OpenAi {
            stream_usage: false,
        },
    };
    if p.kind != ProviderKind::Custom && api_key.is_none() {
        bail!("aucune clé d'API enregistrée pour {}", p.name);
    }
    Ok(Endpoint {
        api,
        base_url: base_url(p)?,
        model: model.to_owned(),
        api_key,
        lease: None,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteModel {
    pub id: String,
    pub name: String,
    /// Prix connus, en dollars par million de jetons.
    pub input_per_m: Option<f64>,
    pub output_per_m: Option<f64>,
}

fn per_million(v: &Value) -> Option<f64> {
    // OpenRouter donne des prix par jeton, sous forme de texte.
    let per_token = v.as_str()?.parse::<f64>().ok()?;
    (per_token >= 0.0).then_some(per_token * 1e6)
}

/// Interroge le fournisseur pour lister ses modèles (sert aussi de test de connexion).
pub async fn list_models(
    http: &reqwest::Client,
    p: &ProviderConfig,
    api_key: Option<&str>,
) -> Result<Vec<RemoteModel>> {
    let base = base_url(p)?;
    if p.kind == ProviderKind::Anthropic {
        let key = api_key.context("clé d'API manquante")?;
        return Ok(anthropic::list_models(http, &base, key)
            .await?
            .into_iter()
            .map(|(id, name)| {
                let price = anthropic::price(&id);
                RemoteModel {
                    input_per_m: price.map(|p| p.0),
                    output_per_m: price.map(|p| p.1),
                    id,
                    name,
                }
            })
            .collect());
    }
    let mut req = http.get(format!("{}/models", base.trim_end_matches('/')));
    if let Some(k) = api_key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.context("le fournisseur ne répond pas")?;
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }
    let v: Value = resp.json().await?;
    let mut models: Vec<RemoteModel> = v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?.trim_start_matches("models/").to_owned();
            Some(RemoteModel {
                name: m["name"].as_str().unwrap_or(&id).to_owned(),
                input_per_m: per_million(&m["pricing"]["prompt"]),
                output_per_m: per_million(&m["pricing"]["completion"]),
                id,
            })
        })
        .collect();
    // Les prix saisis par l'utilisateur complètent ceux du fournisseur.
    for m in &mut models {
        if let Some(pr) = p.price(&m.id) {
            m.input_per_m = Some(pr.input_per_m);
            m.output_per_m = Some(pr.output_per_m);
        }
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

/// Prix d'un modèle : saisi par l'utilisateur, sinon tarif public connu.
pub fn price(p: &ProviderConfig, model: &str) -> Option<(f64, f64)> {
    p.price(model)
        .map(|pr| (pr.input_per_m, pr.output_per_m))
        .or_else(|| {
            (p.kind == ProviderKind::Anthropic)
                .then(|| anthropic::price(model))
                .flatten()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use claudio_core::config::ModelPrice;

    fn provider(kind: ProviderKind) -> ProviderConfig {
        ProviderConfig {
            id: "p".into(),
            kind,
            name: "P".into(),
            base_url: None,
            enabled: true,
            prices: vec![],
        }
    }

    #[test]
    fn endpoints() {
        let a = endpoint(
            &provider(ProviderKind::Anthropic),
            "claude-opus-5-5",
            Some("k".into()),
        )
        .unwrap();
        assert_eq!(a.api, Api::Anthropic);
        assert!(endpoint(&provider(ProviderKind::OpenAi), "gpt", None).is_err());
        assert!(
            endpoint(&provider(ProviderKind::Custom), "m", None).is_err(),
            "pas d'adresse"
        );
        let mut c = provider(ProviderKind::Custom);
        c.base_url = Some("http://localhost:1234/v1".into());
        assert!(endpoint(&c, "m", None).is_ok());
    }

    #[test]
    fn urls_must_be_https_unless_local() {
        assert!(validate_base_url("https://api.example.com/v1").is_ok());
        assert!(validate_base_url("http://localhost:1234/v1").is_ok());
        assert!(validate_base_url("http://example.com/v1").is_err());
    }

    #[test]
    fn prices_prefer_user_values() {
        let mut p = provider(ProviderKind::Anthropic);
        assert_eq!(price(&p, "claude-opus-5-5"), Some((4.0, 20.0)));
        p.prices.push(ModelPrice {
            model: "claude-opus-5-5".into(),
            input_per_m: 1.0,
            output_per_m: 2.0,
        });
        assert_eq!(price(&p, "claude-opus-5-5"), Some((1.0, 2.0)));
        assert_eq!(price(&provider(ProviderKind::OpenAi), "gpt-x"), None);
        assert_eq!(per_million(&Value::from("0.000003")), Some(3.0));
    }
}
