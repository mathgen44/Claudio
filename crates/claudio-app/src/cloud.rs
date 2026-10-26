//! Fournisseurs distants, budgets et consommation.

use anyhow::{Context, Result, bail};
use chrono::Local;
use claudio_core::config::{
    Budget, CloudModel, ModelPrice, ProviderConfig, ProviderKind, RoutingMode,
};
use claudio_core::secrets;
use claudio_core::usage::{DailyCost, UsageRow, period_starts};
use claudio_llm::Endpoint;
use claudio_llm::providers::{self, RemoteModel};
use serde::Serialize;

use crate::Claudio;

#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    #[serde(flatten)]
    pub config: ProviderConfig,
    pub has_key: bool,
    pub kind_label: &'static str,
    pub default_base_url: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BudgetStatus {
    pub today_usd: f64,
    pub month_usd: f64,
    pub daily_usd: Option<f64>,
    pub monthly_usd: Option<f64>,
    /// Un plafond est atteint : le cloud est bloqué.
    pub exceeded: bool,
    /// Plus de 80 % d'un plafond consommé.
    pub warning: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    pub budget: BudgetStatus,
    /// Détail du mois en cours par fournisseur et modèle.
    pub month: Vec<UsageRow>,
    pub daily: Vec<DailyCost>,
}

pub(crate) fn cost(price: Option<(f64, f64)>, input: u64, output: u64) -> Option<f64> {
    price.map(|(i, o)| (input as f64 * i + output as f64 * o) / 1e6)
}

impl Claudio {
    pub fn providers(&self) -> Vec<ProviderView> {
        self.config()
            .providers
            .into_iter()
            .map(|p| ProviderView {
                has_key: secrets::get(&p.secret_name()).ok().flatten().is_some(),
                kind_label: providers::display_name(p.kind),
                default_base_url: providers::default_base_url(p.kind),
                config: p,
            })
            .collect()
    }

    fn provider_config(&self, id: &str) -> Result<ProviderConfig> {
        self.config()
            .provider(id)
            .cloned()
            .with_context(|| format!("fournisseur inconnu : {id}"))
    }

    fn provider_key(&self, p: &ProviderConfig) -> Option<String> {
        secrets::get(&p.secret_name()).ok().flatten()
    }

    pub fn add_provider(
        &self,
        kind: ProviderKind,
        name: &str,
        base_url: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<ProviderView> {
        let base_url = base_url.map(str::trim).filter(|u| !u.is_empty());
        if let Some(u) = base_url {
            providers::validate_base_url(u)?;
        } else if kind == ProviderKind::Custom {
            bail!("indiquez l'adresse de l'API (ex. http://localhost:1234/v1)");
        }
        let cfg = self.config();
        let slug = format!("{kind:?}").to_ascii_lowercase();
        let id = (1..)
            .map(|n| format!("{slug}-{n}"))
            .find(|id| cfg.provider(id).is_none())
            .expect("identifiant libre");
        let name = match name.trim() {
            "" => providers::display_name(kind).to_owned(),
            n => n.to_owned(),
        };
        let p = ProviderConfig {
            id: id.clone(),
            kind,
            name,
            base_url: base_url.map(str::to_owned),
            enabled: true,
            prices: Vec::new(),
        };
        if let Some(k) = api_key.map(str::trim).filter(|k| !k.is_empty()) {
            secrets::set(&p.secret_name(), k)?;
        }
        self.update_config(|c| c.providers.push(p))?;
        self.providers()
            .into_iter()
            .find(|v| v.config.id == id)
            .context("fournisseur ajouté introuvable")
    }

    pub fn update_provider(
        &self,
        id: &str,
        name: Option<&str>,
        base_url: Option<Option<&str>>,
        enabled: Option<bool>,
    ) -> Result<()> {
        if let Some(Some(u)) = base_url.filter(|u| u.is_some_and(|u| !u.trim().is_empty())) {
            providers::validate_base_url(u.trim())?;
        }
        self.provider_config(id)?;
        self.update_config(|c| {
            if let Some(p) = c.providers.iter_mut().find(|p| p.id == id) {
                if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
                    p.name = n.to_owned();
                }
                if let Some(u) = base_url {
                    p.base_url = u.map(|u| u.trim().to_owned()).filter(|u| !u.is_empty());
                }
                if let Some(e) = enabled {
                    p.enabled = e;
                }
            }
        })?;
        Ok(())
    }

    pub fn set_provider_key(&self, id: &str, key: Option<&str>) -> Result<()> {
        let p = self.provider_config(id)?;
        match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => secrets::set(&p.secret_name(), k),
            None => secrets::delete(&p.secret_name()),
        }
    }

    pub fn remove_provider(&self, id: &str) -> Result<()> {
        let p = self.provider_config(id)?;
        secrets::delete(&p.secret_name())?;
        self.update_config(|c| {
            c.providers.retain(|p| p.id != id);
            if c.routing
                .cloud
                .as_ref()
                .is_some_and(|m| m.provider_id == id)
            {
                c.routing.cloud = None;
            }
        })?;
        Ok(())
    }

    /// Liste les modèles du fournisseur ; sert aussi à vérifier la clé.
    pub async fn provider_models(&self, id: &str) -> Result<Vec<RemoteModel>> {
        let p = self.provider_config(id)?;
        let key = self.provider_key(&p);
        providers::list_models(&self.http, &p, key.as_deref()).await
    }

    pub fn set_model_price(
        &self,
        provider_id: &str,
        model: &str,
        input: f64,
        output: f64,
    ) -> Result<()> {
        if !(input >= 0.0 && output >= 0.0 && input.is_finite() && output.is_finite()) {
            bail!("prix invalide");
        }
        self.provider_config(provider_id)?;
        self.update_config(|c| {
            if let Some(p) = c.providers.iter_mut().find(|p| p.id == provider_id) {
                p.prices.retain(|pr| pr.model != model);
                p.prices.push(ModelPrice {
                    model: model.to_owned(),
                    input_per_m: input,
                    output_per_m: output,
                });
            }
        })?;
        Ok(())
    }

    /// Choisit le modèle distant du routeur (et retient son prix s'il est connu).
    pub fn set_cloud_model(
        &self,
        model: Option<CloudModel>,
        price: Option<(f64, f64)>,
    ) -> Result<()> {
        if let Some(m) = &model {
            self.provider_config(&m.provider_id)?;
            if let Some((i, o)) = price {
                self.set_model_price(&m.provider_id, &m.model, i, o)?;
            }
        }
        self.update_config(|c| c.routing.cloud = model)?;
        Ok(())
    }

    pub fn set_routing_mode(&self, mode: RoutingMode) -> Result<()> {
        self.update_config(|c| c.routing.mode = mode)?;
        Ok(())
    }

    pub fn set_budget(&self, budget: Budget) -> Result<()> {
        let valid = |v: Option<f64>| v.is_none_or(|x| x.is_finite() && x >= 0.0);
        if !valid(budget.daily_usd) || !valid(budget.monthly_usd) {
            bail!("plafond invalide");
        }
        self.update_config(|c| c.budget = budget)?;
        Ok(())
    }

    pub fn budget_status(&self) -> Result<BudgetStatus> {
        let (day, month) = period_starts(Local::now());
        let today_usd = self.db.spend_since(day)?;
        let month_usd = self.db.spend_since(month)?;
        let b = self.config().budget;
        let ratio = |spent: f64, limit: Option<f64>| {
            limit.map(|l| if l <= 0.0 { f64::INFINITY } else { spent / l })
        };
        let ratios = [
            ratio(today_usd, b.daily_usd),
            ratio(month_usd, b.monthly_usd),
        ];
        Ok(BudgetStatus {
            today_usd,
            month_usd,
            daily_usd: b.daily_usd,
            monthly_usd: b.monthly_usd,
            exceeded: ratios.iter().flatten().any(|r| *r >= 1.0),
            warning: ratios.iter().flatten().any(|r| *r >= 0.8),
        })
    }

    pub fn usage_report(&self) -> Result<UsageReport> {
        let (_, month) = period_starts(Local::now());
        Ok(UsageReport {
            budget: self.budget_status()?,
            month: self.db.usage_summary(month)?,
            daily: self.db.daily_costs(30)?,
        })
    }

    /// Point d'accès du modèle distant configuré, s'il est utilisable.
    pub(crate) fn cloud_endpoint(&self) -> Result<Option<(ProviderConfig, Endpoint)>> {
        let cfg = self.config();
        let Some(m) = cfg.routing.cloud.as_ref() else {
            return Ok(None);
        };
        let Some(p) = cfg.provider(&m.provider_id).filter(|p| p.enabled) else {
            return Ok(None);
        };
        let key = self.provider_key(p);
        match providers::endpoint(p, &m.model, key) {
            Ok(ep) => Ok(Some((p.clone(), ep))),
            Err(e) => {
                tracing::debug!("modèle distant inutilisable : {e:#}");
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs() {
        assert_eq!(cost(None, 1000, 1000), None);
        let c = cost(Some((4.0, 20.0)), 1_000_000, 100_000).unwrap();
        assert!((c - 6.0).abs() < 1e-9);
    }
}
