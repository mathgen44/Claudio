//! Tâches planifiées (cron) : relancent un objectif à heures fixes.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, Utc};
use claudio_core::tasks::Schedule;
use croner::Cron;
use serde::Serialize;

use crate::Claudio;

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleView {
    #[serde(flatten)]
    pub schedule: Schedule,
    /// Prochaine exécution (si activée).
    pub next_run: Option<DateTime<Utc>>,
}

/// Lit une expression cron à 5 champs (minute heure jour mois jour-de-semaine).
pub fn parse_cron(expr: &str) -> Result<Cron> {
    let expr = expr.trim();
    if expr.split_whitespace().count() != 5 {
        bail!("l'expression doit avoir 5 champs : minute heure jour mois jour-de-la-semaine");
    }
    Cron::from_str(expr).map_err(|e| anyhow::anyhow!("expression cron invalide : {e}"))
}

/// Prochaine échéance après `after`, en heure locale.
pub fn next_after(cron: &Cron, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    cron.find_next_occurrence(&after.with_timezone(&Local), false)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Échéance due : la prochaine après la dernière exécution (ou la création) est passée.
fn due(s: &Schedule, now: DateTime<Utc>) -> bool {
    let Ok(cron) = parse_cron(&s.cron) else {
        return false;
    };
    let since = s.last_run.unwrap_or(s.created_at);
    next_after(&cron, since).is_some_and(|n| n <= now)
}

impl Claudio {
    pub fn schedules(&self) -> Result<Vec<ScheduleView>> {
        let now = Utc::now();
        Ok(self
            .db
            .schedules()?
            .into_iter()
            .map(|s| {
                let next_run = s
                    .enabled
                    .then(|| parse_cron(&s.cron).ok())
                    .flatten()
                    .and_then(|c| next_after(&c, now));
                ScheduleView {
                    schedule: s,
                    next_run,
                }
            })
            .collect())
    }

    pub fn add_schedule(
        &self,
        title: &str,
        objective: &str,
        mode: &str,
        cron: &str,
    ) -> Result<Schedule> {
        parse_cron(cron)?;
        if mode != "plan" && !mode.starts_with("agent:") {
            bail!("mode inconnu : {mode}");
        }
        let title = match title.trim() {
            "" => crate::conversation::title_from(objective),
            t => t.to_owned(),
        };
        self.db
            .add_schedule(&title, objective.trim(), mode, cron.trim())
    }

    pub fn set_schedule_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.db.set_schedule_enabled(id, enabled)
    }

    pub fn delete_schedule(&self, id: i64) -> Result<()> {
        self.db.delete_schedule(id)
    }

    /// Lance tout de suite la tâche d'une planification.
    pub fn run_schedule_now(self: &Arc<Self>, id: i64) -> Result<String> {
        let s = self
            .db
            .schedules()?
            .into_iter()
            .find(|s| s.id == id)
            .context("planification introuvable")?;
        self.db.mark_schedule_run(id, Utc::now())?;
        Ok(self.start_task(&s.objective, &s.mode, Some(s.id))?.id)
    }

    /// Vérifie les échéances ; lance les tâches dues. Renvoie les tâches lancées.
    pub fn tick_schedules(self: &Arc<Self>, now: DateTime<Utc>) -> Result<Vec<String>> {
        let mut started = Vec::new();
        let running: Vec<Option<i64>> = self
            .db
            .list_tasks(100)?
            .into_iter()
            .filter(|t| t.status.is_active())
            .map(|t| t.schedule_id)
            .collect();
        for s in self.db.schedules()?.into_iter().filter(|s| s.enabled) {
            if !due(&s, now) {
                continue;
            }
            // Pas d'empilement : une échéance manquée pendant l'exécution précédente est sautée.
            self.db.mark_schedule_run(s.id, now)?;
            if running.contains(&Some(s.id)) {
                tracing::info!(schedule = %s.title, "exécution précédente encore en cours : échéance sautée");
                continue;
            }
            tracing::info!(schedule = %s.title, "tâche planifiée lancée");
            started.push(self.start_task(&s.objective, &s.mode, Some(s.id))?.id);
        }
        Ok(started)
    }

    /// Boucle du planificateur (à lancer une fois, dans le runtime asynchrone).
    pub async fn scheduler_loop(self: Arc<Self>) {
        loop {
            if let Err(e) = self.tick_schedules(Utc::now()) {
                tracing::warn!("planificateur : {e:#}");
            }
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn cron_parsing_and_next_run() {
        assert!(parse_cron("0 8 * * 1-5").is_ok());
        assert!(parse_cron("0 8 * *").is_err());
        assert!(parse_cron("99 8 * * *").is_err());
        let c = parse_cron("30 7 * * *").unwrap();
        let start = Local
            .with_ymd_and_hms(2026, 10, 26, 8, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let next = next_after(&c, start).unwrap().with_timezone(&Local);
        assert_eq!(
            next.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-27 07:30"
        );
    }

    #[test]
    fn due_after_last_run() {
        let created = Local
            .with_ymd_and_hms(2026, 10, 26, 6, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let s = Schedule {
            id: 1,
            title: "t".into(),
            objective: "o".into(),
            mode: "plan".into(),
            cron: "30 7 * * *".into(),
            enabled: true,
            last_run: None,
            created_at: created,
        };
        let at = |h, m| {
            Local
                .with_ymd_and_hms(2026, 10, 26, h, m, 0)
                .unwrap()
                .with_timezone(&Utc)
        };
        assert!(!due(&s, at(7, 29)));
        assert!(due(&s, at(7, 30)));
        let ran = Schedule {
            last_run: Some(at(7, 30)),
            ..s
        };
        assert!(!due(&ran, at(12, 0)), "déjà exécutée aujourd'hui");
    }
}
