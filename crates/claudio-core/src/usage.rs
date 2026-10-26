//! Consommation des modèles : jetons et coûts, pour les budgets et les statistiques.

use anyhow::Result;
use chrono::{DateTime, Datelike, Local, NaiveTime, TimeZone, Utc};
use rusqlite::params;
use serde::Serialize;

use crate::Database;

#[derive(Debug, Clone)]
pub struct UsageRecord<'a> {
    pub session_id: Option<&'a str>,
    /// Identifiant du fournisseur, ou `local`.
    pub provider: &'a str,
    pub model: &'a str,
    pub local: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `None` quand le prix du modèle est inconnu.
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageRow {
    pub provider: String,
    pub model: String,
    pub local: bool,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// Certaines demandes n'ont pas de prix connu : le coût est sous-estimé.
    pub unpriced: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyCost {
    pub day: String,
    pub cost_usd: f64,
    pub local_requests: u64,
    pub cloud_requests: u64,
}

/// Début du jour et du mois en cours, heure locale.
pub fn period_starts(now: DateTime<Local>) -> (DateTime<Utc>, DateTime<Utc>) {
    let day = now.date_naive().and_time(NaiveTime::MIN);
    let month = day.with_day(1).expect("premier du mois");
    let to_utc = |n: chrono::NaiveDateTime| {
        Local
            .from_local_datetime(&n)
            .earliest()
            .unwrap_or(now)
            .with_timezone(&Utc)
    };
    (to_utc(day), to_utc(month))
}

impl Database {
    pub fn record_usage(&self, r: &UsageRecord) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO usage (at, session_id, provider, model, local, input_tokens, output_tokens, cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    Utc::now(),
                    r.session_id,
                    r.provider,
                    r.model,
                    r.local,
                    r.input_tokens as i64,
                    r.output_tokens as i64,
                    r.cost_usd
                ],
            )
        })?;
        Ok(())
    }

    /// Dépense cloud depuis `since`.
    pub fn spend_since(&self, since: DateTime<Utc>) -> Result<f64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(cost_usd), 0) FROM usage WHERE at >= ?1",
                [since],
                |r| r.get(0),
            )
        })
    }

    pub fn usage_summary(&self, since: DateTime<Utc>) -> Result<Vec<UsageRow>> {
        self.with(|c| {
            c.prepare(
                "SELECT provider, model, local, COUNT(*), SUM(input_tokens), SUM(output_tokens),
                        COALESCE(SUM(cost_usd), 0), SUM(cost_usd IS NULL AND local = 0)
                 FROM usage WHERE at >= ?1
                 GROUP BY provider, model, local
                 ORDER BY 7 DESC, 4 DESC",
            )?
            .query_map([since], |r| {
                Ok(UsageRow {
                    provider: r.get(0)?,
                    model: r.get(1)?,
                    local: r.get(2)?,
                    requests: r.get::<_, i64>(3)? as u64,
                    input_tokens: r.get::<_, i64>(4)? as u64,
                    output_tokens: r.get::<_, i64>(5)? as u64,
                    cost_usd: r.get(6)?,
                    unpriced: r.get::<_, i64>(7)? as u64,
                })
            })?
            .collect()
        })
    }

    /// Coût par jour (UTC) sur les `days` derniers jours.
    pub fn daily_costs(&self, days: i64) -> Result<Vec<DailyCost>> {
        let since = Utc::now() - chrono::Duration::days(days);
        self.with(|c| {
            c.prepare(
                "SELECT substr(at, 1, 10) AS day, COALESCE(SUM(cost_usd), 0),
                        SUM(local = 1), SUM(local = 0)
                 FROM usage WHERE at >= ?1 GROUP BY day ORDER BY day",
            )?
            .query_map([since], |r| {
                Ok(DailyCost {
                    day: r.get(0)?,
                    cost_usd: r.get(1)?,
                    local_requests: r.get::<_, i64>(2)? as u64,
                    cloud_requests: r.get::<_, i64>(3)? as u64,
                })
            })?
            .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_sums() {
        let db = Database::open_in_memory().unwrap();
        let rec = |provider, model, local, cost| UsageRecord {
            session_id: None,
            provider,
            model,
            local,
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: cost,
        };
        db.record_usage(&rec("local", "qwen", true, Some(0.0)))
            .unwrap();
        db.record_usage(&rec("p1", "claude-opus-5-5", false, Some(0.25)))
            .unwrap();
        db.record_usage(&rec("p1", "claude-opus-5-5", false, Some(0.5)))
            .unwrap();
        db.record_usage(&rec("p2", "mystery", false, None)).unwrap();

        let start = Utc::now() - chrono::Duration::hours(1);
        assert!((db.spend_since(start).unwrap() - 0.75).abs() < 1e-9);
        assert_eq!(
            db.spend_since(Utc::now() + chrono::Duration::hours(1))
                .unwrap(),
            0.0
        );

        let rows = db.usage_summary(start).unwrap();
        assert_eq!(rows[0].model, "claude-opus-5-5");
        assert_eq!(rows[0].requests, 2);
        assert_eq!(
            rows.iter().find(|r| r.model == "mystery").unwrap().unpriced,
            1
        );
        let days = db.daily_costs(2).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!((days[0].local_requests, days[0].cloud_requests), (1, 3));
    }

    #[test]
    fn periods() {
        let now = Local::now();
        let (day, month) = period_starts(now);
        assert!(month <= day && day <= now.with_timezone(&Utc));
    }
}
