//! Tâches de fond (objectif → étapes confiées à des agents) et planifications.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Planning,
    Running,
    Done,
    /// Terminée, mais certaines étapes ont échoué.
    Partial,
    Failed,
    Cancelled,
    /// Claudio s'est arrêté pendant l'exécution : peut être reprise.
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Running,
    /// En attente d'une autorisation de l'utilisateur.
    Waiting,
    Done,
    Failed,
    /// Non exécutée car une étape dont elle dépend a échoué.
    Skipped,
    Cancelled,
}

macro_rules! text_enum {
    ($t:ty { $($v:ident => $s:literal),* $(,)? }) => {
        impl $t {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$v => $s),* }
            }
            pub fn parse(s: &str) -> Self {
                match s { $($s => Self::$v,)* _ => Self::Failed }
            }
        }
    };
}

text_enum!(TaskStatus { Planning => "planning", Running => "running", Done => "done", Partial => "partial",
    Failed => "failed", Cancelled => "cancelled", Interrupted => "interrupted" });
text_enum!(StepStatus { Pending => "pending", Running => "running", Waiting => "waiting", Done => "done",
    Failed => "failed", Skipped => "skipped", Cancelled => "cancelled" });

impl TaskStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Planning | Self::Running)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub objective: String,
    /// `plan` (le planificateur découpe) ou `agent:<id>` (un seul agent).
    pub mode: String,
    pub status: TaskStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub schedule_id: Option<i64>,
    pub tokens: i64,
    pub cost_usd: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Étapes terminées / total (pour les listes).
    pub steps_done: i64,
    pub steps_total: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskStep {
    pub step_id: String,
    pub position: i64,
    pub agent: String,
    pub title: String,
    pub instructions: String,
    pub depends_on: Vec<String>,
    pub status: StepStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub model: Option<String>,
    /// Journal d'exécution (appels d'outils…), une ligne par événement.
    pub log: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewStep<'a> {
    pub step_id: &'a str,
    pub agent: &'a str,
    pub title: &'a str,
    pub instructions: &'a str,
    pub depends_on: &'a [String],
}

#[derive(Debug, Clone, Serialize)]
pub struct Schedule {
    pub id: i64,
    pub title: String,
    pub objective: String,
    pub mode: String,
    /// Expression cron à 5 champs (minute heure jour mois jour-de-semaine).
    pub cron: String,
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const TASK_COLUMNS: &str =
    "t.id, t.title, t.objective, t.mode, t.status, t.result, t.error, t.schedule_id,
    t.tokens, t.cost_usd, t.created_at, t.updated_at,
    (SELECT COUNT(*) FROM task_steps s WHERE s.task_id = t.id AND s.status = 'done'),
    (SELECT COUNT(*) FROM task_steps s WHERE s.task_id = t.id)";

fn task_from_row(r: &Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        title: r.get(1)?,
        objective: r.get(2)?,
        mode: r.get(3)?,
        status: TaskStatus::parse(&r.get::<_, String>(4)?),
        result: r.get(5)?,
        error: r.get(6)?,
        schedule_id: r.get(7)?,
        tokens: r.get(8)?,
        cost_usd: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        steps_done: r.get(12)?,
        steps_total: r.get(13)?,
    })
}

fn step_from_row(r: &Row<'_>) -> rusqlite::Result<TaskStep> {
    let deps: String = r.get(5)?;
    Ok(TaskStep {
        step_id: r.get(0)?,
        position: r.get(1)?,
        agent: r.get(2)?,
        title: r.get(3)?,
        instructions: r.get(4)?,
        depends_on: serde_json::from_str(&deps).unwrap_or_default(),
        status: StepStatus::parse(&r.get::<_, String>(6)?),
        output: r.get(7)?,
        error: r.get(8)?,
        model: r.get(9)?,
        log: r.get(10)?,
        started_at: r.get(11)?,
        finished_at: r.get(12)?,
    })
}

fn schedule_from_row(r: &Row<'_>) -> rusqlite::Result<Schedule> {
    Ok(Schedule {
        id: r.get(0)?,
        title: r.get(1)?,
        objective: r.get(2)?,
        mode: r.get(3)?,
        cron: r.get(4)?,
        enabled: r.get(5)?,
        last_run: r.get(6)?,
        created_at: r.get(7)?,
    })
}

impl Database {
    pub fn create_task(
        &self,
        title: &str,
        objective: &str,
        mode: &str,
        schedule_id: Option<i64>,
    ) -> Result<Task> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        self.with(|c| {
            c.execute(
                "INSERT INTO tasks (id, title, objective, mode, status, schedule_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'planning', ?5, ?6, ?6)",
                params![id, title, objective, mode, schedule_id, now],
            )
        })?;
        Ok(self.get_task(&id)?.expect("tâche créée").0)
    }

    pub fn get_task(&self, id: &str) -> Result<Option<(Task, Vec<TaskStep>)>> {
        let task = self.with(|c| {
            c.query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks t WHERE t.id = ?1"),
                [id],
                task_from_row,
            )
            .optional()
        })?;
        let Some(task) = task else { return Ok(None) };
        let steps = self.with(|c| {
            c.prepare(
                "SELECT step_id, position, agent, title, instructions, depends_on, status, output,
                        error, model, log, started_at, finished_at
                 FROM task_steps WHERE task_id = ?1 ORDER BY position",
            )?
            .query_map([id], step_from_row)?
            .collect()
        })?;
        Ok(Some((task, steps)))
    }

    pub fn list_tasks(&self, limit: u32) -> Result<Vec<Task>> {
        self.with(|c| {
            c.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks t ORDER BY t.created_at DESC LIMIT ?1"
            ))?
            .query_map([limit], task_from_row)?
            .collect()
        })
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        self.with(|c| c.execute("DELETE FROM tasks WHERE id = ?1", [id]))?;
        Ok(())
    }

    pub fn set_task_status(
        &self,
        id: &str,
        status: TaskStatus,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE tasks SET status = ?2, result = COALESCE(?3, result), error = ?4, updated_at = ?5
                 WHERE id = ?1",
                params![id, status.as_str(), result, error, Utc::now()],
            )
        })?;
        Ok(())
    }

    pub fn add_task_usage(&self, id: &str, tokens: i64, cost: f64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE tasks SET tokens = tokens + ?2, cost_usd = cost_usd + ?3 WHERE id = ?1",
                params![id, tokens, cost],
            )
        })?;
        Ok(())
    }

    /// Remplace les étapes d'une tâche (après planification).
    pub fn set_task_steps(&self, task_id: &str, steps: &[NewStep]) -> Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("DELETE FROM task_steps WHERE task_id = ?1", [task_id])?;
            for (i, s) in steps.iter().enumerate() {
                tx.execute(
                    "INSERT INTO task_steps (task_id, step_id, position, agent, title, instructions,
                                             depends_on, status, log)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', '')",
                    params![
                        task_id,
                        s.step_id,
                        i as i64,
                        s.agent,
                        s.title,
                        s.instructions,
                        serde_json::to_string(s.depends_on).unwrap_or_default()
                    ],
                )?;
            }
            tx.commit()
        })
    }

    pub fn set_step_status(
        &self,
        task_id: &str,
        step_id: &str,
        status: StepStatus,
        output: Option<&str>,
        error: Option<&str>,
        model: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();
        self.with(|c| {
            c.execute(
                "UPDATE task_steps SET status = ?3,
                    output = COALESCE(?4, output), error = ?5, model = COALESCE(?6, model),
                    started_at = CASE WHEN ?3 = 'running' AND started_at IS NULL THEN ?7 ELSE started_at END,
                    finished_at = CASE WHEN ?3 IN ('done','failed','skipped','cancelled') THEN ?7 ELSE finished_at END
                 WHERE task_id = ?1 AND step_id = ?2",
                params![task_id, step_id, status.as_str(), output, error, model, now],
            )?;
            c.execute("UPDATE tasks SET updated_at = ?2 WHERE id = ?1", params![task_id, now])
        })?;
        Ok(())
    }

    pub fn append_step_log(&self, task_id: &str, step_id: &str, line: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE task_steps SET log = log || ?3 || char(10) WHERE task_id = ?1 AND step_id = ?2",
                params![task_id, step_id, line],
            )
        })?;
        Ok(())
    }

    /// Au démarrage : les tâches qui tournaient lors de l'arrêt deviennent « interrompues »
    /// et leurs étapes en cours repassent « à faire ». Renvoie leur nombre.
    pub fn mark_interrupted_tasks(&self) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE task_steps SET status = 'pending', started_at = NULL
                 WHERE status IN ('running', 'waiting')
                   AND task_id IN (SELECT id FROM tasks WHERE status IN ('planning', 'running'))",
                [],
            )?;
            c.execute(
                "UPDATE tasks SET status = 'interrupted' WHERE status IN ('planning', 'running')",
                [],
            )
        })
    }

    /// Prépare une reprise : étapes non terminées remises « à faire ».
    pub fn reset_unfinished_steps(&self, task_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE task_steps SET status = 'pending', error = NULL, started_at = NULL, finished_at = NULL
                 WHERE task_id = ?1 AND status != 'done'",
                [task_id],
            )
        })?;
        Ok(())
    }

    pub fn add_schedule(
        &self,
        title: &str,
        objective: &str,
        mode: &str,
        cron: &str,
    ) -> Result<Schedule> {
        if objective.trim().is_empty() {
            bail!("objectif vide");
        }
        let now = Utc::now();
        let id = self.with(|c| {
            c.execute(
                "INSERT INTO schedules (title, objective, mode, cron, enabled, created_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                params![title, objective, mode, cron, now],
            )?;
            Ok(c.last_insert_rowid())
        })?;
        Ok(self
            .schedules()?
            .into_iter()
            .find(|s| s.id == id)
            .expect("planification créée"))
    }

    pub fn schedules(&self) -> Result<Vec<Schedule>> {
        self.with(|c| {
            c.prepare(
                "SELECT id, title, objective, mode, cron, enabled, last_run, created_at
                 FROM schedules ORDER BY id",
            )?
            .query_map([], schedule_from_row)?
            .collect()
        })
    }

    pub fn set_schedule_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE schedules SET enabled = ?2 WHERE id = ?1",
                params![id, enabled],
            )
        })?;
        Ok(())
    }

    pub fn delete_schedule(&self, id: i64) -> Result<()> {
        self.with(|c| c.execute("DELETE FROM schedules WHERE id = ?1", [id]))?;
        Ok(())
    }

    pub fn mark_schedule_run(&self, id: i64, at: DateTime<Utc>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE schedules SET last_run = ?2 WHERE id = ?1",
                params![id, at],
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        let t = db
            .create_task("Veille", "Fais une veille", "plan", None)
            .unwrap();
        assert_eq!(t.status, TaskStatus::Planning);
        let deps = vec!["e1".to_string()];
        db.set_task_steps(
            &t.id,
            &[
                NewStep {
                    step_id: "e1",
                    agent: "researcher",
                    title: "A",
                    instructions: "a",
                    depends_on: &[],
                },
                NewStep {
                    step_id: "e2",
                    agent: "reviewer",
                    title: "B",
                    instructions: "b",
                    depends_on: &deps,
                },
            ],
        )
        .unwrap();
        db.set_task_status(&t.id, TaskStatus::Running, None, None)
            .unwrap();
        db.set_step_status(&t.id, "e1", StepStatus::Running, None, None, Some("gemma"))
            .unwrap();
        db.append_step_log(&t.id, "e1", "outil web_search").unwrap();
        db.set_step_status(&t.id, "e1", StepStatus::Done, Some("trouvé"), None, None)
            .unwrap();
        db.set_step_status(&t.id, "e2", StepStatus::Running, None, None, None)
            .unwrap();
        db.add_task_usage(&t.id, 100, 0.5).unwrap();

        let (task, steps) = db.get_task(&t.id).unwrap().unwrap();
        assert_eq!((task.steps_done, task.steps_total), (1, 2));
        assert_eq!(task.tokens, 100);
        assert_eq!(steps[0].log, "outil web_search\n");
        assert_eq!(steps[0].model.as_deref(), Some("gemma"));
        assert!(steps[0].finished_at.is_some());
        assert_eq!(steps[1].depends_on, deps);

        // Arrêt brutal puis redémarrage.
        assert_eq!(db.mark_interrupted_tasks().unwrap(), 1);
        let (task, steps) = db.get_task(&t.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Interrupted);
        assert_eq!(
            steps[0].status,
            StepStatus::Done,
            "le travail fait est gardé"
        );
        assert_eq!(steps[1].status, StepStatus::Pending);

        db.delete_task(&t.id).unwrap();
        assert!(db.get_task(&t.id).unwrap().is_none());
    }

    #[test]
    fn schedules() {
        let db = Database::open_in_memory().unwrap();
        let s = db
            .add_schedule(
                "Matin",
                "Résume mes mails",
                "agent:assistant",
                "0 8 * * 1-5",
            )
            .unwrap();
        assert!(s.enabled && s.last_run.is_none());
        db.set_schedule_enabled(s.id, false).unwrap();
        db.mark_schedule_run(s.id, Utc::now()).unwrap();
        let all = db.schedules().unwrap();
        assert!(!all[0].enabled && all[0].last_run.is_some());
        db.delete_schedule(s.id).unwrap();
        assert!(db.schedules().unwrap().is_empty());
        assert!(db.add_schedule("x", " ", "plan", "* * * * *").is_err());
    }
}
