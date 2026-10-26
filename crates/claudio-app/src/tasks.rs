//! Tâches de fond : un objectif est découpé par le planificateur en étapes
//! (graphe de dépendances), confiées à des agents et exécutées en parallèle dans
//! la limite de la machine. L'état est en base : une tâche interrompue reprend
//! là où elle s'était arrêtée.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use claudio_agents::{AgentDef, AgentSet, ModelPref, plan};
use claudio_core::config::RoutingMode;
use claudio_core::tasks::{NewStep, StepStatus, Task, TaskStatus, TaskStep};
use claudio_llm::chat::ChatMessage;
use claudio_models::CancelToken;
use claudio_perms::{Answer, Approval};
use claudio_tools::{Approver, BoxFuture};
use serde::Serialize;
use tokio::task::JoinSet;

use crate::Claudio;
use crate::conversation::{LoopCtx, StreamEvent, clip, model_label, title_from, wait_answer};
use crate::router::Target;

/// Taille maximale du résultat d'une étape transmis aux étapes suivantes.
const DEP_OUTPUT_CHARS: usize = 8000;

/// Événements globaux (hors conversation) envoyés à l'interface.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    /// L'état d'une tâche a changé : la recharger.
    TaskUpdated { task_id: String },
    /// Texte produit par une étape en cours.
    StepOutput {
        task_id: String,
        step_id: String,
        text: String,
    },
    /// Autorisation demandée par une tâche de fond.
    Approval {
        id: String,
        task_id: String,
        task_title: String,
        step_title: String,
        tool: String,
        #[serde(flatten)]
        approval: Approval,
    },
    TaskFinished {
        task_id: String,
        title: String,
        status: TaskStatus,
    },
}

pub type EventSink = Arc<dyn Fn(AppEvent) + Send + Sync>;

#[derive(Debug, Clone, Serialize)]
pub struct TaskDetail {
    #[serde(flatten)]
    pub task: Task,
    pub steps: Vec<TaskStep>,
    /// Niveaux du graphe (identifiants d'étapes), pour l'affichage.
    pub levels: Vec<Vec<String>>,
}

/// Mode d'une tâche : planification automatique, ou un seul agent.
fn parse_task_mode(mode: &str) -> Option<Option<String>> {
    match mode {
        "plan" => Some(None),
        m => m.strip_prefix("agent:").map(|a| Some(a.to_owned())),
    }
}

/// Demandes d'autorisation d'une étape : remontées à l'interface globale.
struct TaskApprover {
    app: Arc<Claudio>,
    task_id: String,
    task_title: String,
    step_id: String,
    step_title: String,
    cancel: CancelToken,
}

impl Approver for TaskApprover {
    fn ask<'a>(&'a self, tool: &'a str, approval: &'a Approval) -> BoxFuture<'a, Answer> {
        Box::pin(async move {
            let (id, rx) = self.app.register_approval();
            let db = &self.app.db;
            let _ = db.set_step_status(
                &self.task_id,
                &self.step_id,
                StepStatus::Waiting,
                None,
                None,
                None,
            );
            self.app.emit_event(AppEvent::TaskUpdated {
                task_id: self.task_id.clone(),
            });
            self.app.emit_event(AppEvent::Approval {
                id: id.clone(),
                task_id: self.task_id.clone(),
                task_title: self.task_title.clone(),
                step_title: self.step_title.clone(),
                tool: tool.to_owned(),
                approval: approval.clone(),
            });
            let answer = wait_answer(&self.app, &id, rx, &self.cancel).await;
            let _ = db.set_step_status(
                &self.task_id,
                &self.step_id,
                StepStatus::Running,
                None,
                None,
                None,
            );
            self.app.emit_event(AppEvent::TaskUpdated {
                task_id: self.task_id.clone(),
            });
            answer
        })
    }
}

impl Claudio {
    pub fn set_event_sink(&self, sink: EventSink) {
        *self.events.write().unwrap() = Some(sink);
    }

    pub(crate) fn emit_event(&self, ev: AppEvent) {
        if let Some(sink) = self.events.read().unwrap().clone() {
            sink(ev);
        }
    }

    pub fn agents(&self) -> AgentSet {
        self.agents.read().unwrap().clone()
    }

    /// Relit les définitions d'agents (après modification des fichiers YAML).
    pub fn reload_agents(&self) -> AgentSet {
        let set = AgentSet::load(Some(&self.paths.agents_dir()));
        *self.agents.write().unwrap() = set.clone();
        set
    }

    pub fn list_tasks(&self, limit: u32) -> Result<Vec<Task>> {
        self.db.list_tasks(limit.clamp(1, 500))
    }

    pub fn task_detail(&self, id: &str) -> Result<TaskDetail> {
        let (task, steps) = self.db.get_task(id)?.context("tâche introuvable")?;
        let plan = plan::Plan {
            steps: steps
                .iter()
                .map(|s| plan::PlanStep {
                    id: s.step_id.clone(),
                    agent: s.agent.clone(),
                    title: s.title.clone(),
                    instructions: String::new(),
                    depends_on: s.depends_on.clone(),
                })
                .collect(),
        };
        Ok(TaskDetail {
            levels: plan::levels(&plan).unwrap_or_default(),
            task,
            steps,
        })
    }

    pub fn cancel_task(&self, id: &str) {
        if let Some(t) = self.running_tasks.lock().unwrap().get(id) {
            t.cancel();
        }
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        if self.running_tasks.lock().unwrap().contains_key(id) {
            bail!("arrêtez la tâche avant de la supprimer");
        }
        self.db.delete_task(id)
    }

    /// Crée une tâche et la lance en arrière-plan.
    pub fn start_task(
        self: &Arc<Self>,
        objective: &str,
        mode: &str,
        schedule_id: Option<i64>,
    ) -> Result<Task> {
        let objective = objective.trim();
        if objective.is_empty() {
            bail!("décrivez l'objectif de la tâche");
        }
        if let Some(Some(agent)) = parse_task_mode(mode) {
            self.agents()
                .get(&agent)
                .with_context(|| format!("agent inconnu : {agent}"))?;
        } else if parse_task_mode(mode).is_none() {
            bail!("mode de tâche inconnu : {mode}");
        }
        let task = self
            .db
            .create_task(&title_from(objective), objective, mode, schedule_id)?;
        self.spawn_task(task.id.clone());
        Ok(task)
    }

    /// Relance une tâche interrompue, échouée ou annulée : les étapes terminées sont gardées.
    pub fn resume_task(self: &Arc<Self>, id: &str) -> Result<()> {
        let (task, _) = self.db.get_task(id)?.context("tâche introuvable")?;
        if task.status.is_active() || self.running_tasks.lock().unwrap().contains_key(id) {
            bail!("la tâche est déjà en cours");
        }
        self.db.reset_unfinished_steps(id)?;
        self.db
            .set_task_status(id, TaskStatus::Running, None, None)?;
        self.spawn_task(id.to_owned());
        Ok(())
    }

    fn spawn_task(self: &Arc<Self>, id: String) {
        let cancel = CancelToken::default();
        self.running_tasks
            .lock()
            .unwrap()
            .insert(id.clone(), cancel.clone());
        let app = self.clone();
        tokio::spawn(async move {
            let res = app.clone().run_task(&id, &cancel).await;
            app.running_tasks.lock().unwrap().remove(&id);
            let status = match res {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(task = %id, "tâche en échec : {e:#}");
                    let _ = app.db.set_task_status(
                        &id,
                        TaskStatus::Failed,
                        None,
                        Some(&format!("{e:#}")),
                    );
                    TaskStatus::Failed
                }
            };
            let title = app
                .db
                .get_task(&id)
                .ok()
                .flatten()
                .map(|(t, _)| t.title)
                .unwrap_or_default();
            app.emit_event(AppEvent::TaskUpdated {
                task_id: id.clone(),
            });
            app.emit_event(AppEvent::TaskFinished {
                task_id: id,
                title,
                status,
            });
        });
    }

    async fn run_task(self: Arc<Self>, id: &str, cancel: &CancelToken) -> Result<TaskStatus> {
        let (task, steps) = self.db.get_task(id)?.context("tâche introuvable")?;
        let agents = self.agents();
        if steps.is_empty() {
            let plan = match parse_task_mode(&task.mode).flatten() {
                Some(agent) => plan::single(&agent, &task.objective),
                None => self.plan_objective(&task, &agents, cancel).await,
            };
            let plan = plan::with_synthesis(plan);
            let new: Vec<NewStep> = plan
                .steps
                .iter()
                .map(|s| NewStep {
                    step_id: &s.id,
                    agent: &s.agent,
                    title: &s.title,
                    instructions: &s.instructions,
                    depends_on: &s.depends_on,
                })
                .collect();
            self.db.set_task_steps(id, &new)?;
        }
        self.db
            .set_task_status(id, TaskStatus::Running, None, None)?;
        self.emit_event(AppEvent::TaskUpdated {
            task_id: id.to_owned(),
        });
        self.execute(&task, &agents, cancel).await
    }

    /// Demande un plan au planificateur ; en cas d'échec, une seule étape pour l'assistant.
    async fn plan_objective(
        &self,
        task: &Task,
        agents: &AgentSet,
        cancel: &CancelToken,
    ) -> plan::Plan {
        let planner = agents.get("planner").cloned();
        let attempt = async {
            let planner = planner.context("agent planificateur absent")?;
            let messages = vec![
                ChatMessage::text(
                    "system",
                    self.system_prompt(&planner.system_prompt, "", false),
                ),
                ChatMessage::text("user", plan::planner_prompt(&task.objective, agents)),
            ];
            let decision = self.route(mode_for(&planner, self.config().routing.mode), &messages)?;
            let noop = |_: StreamEvent| {};
            let approver = DenyAll;
            let lc = LoopCtx {
                scope: "",
                approver: &approver,
                emit: &noop,
                cancel,
                max_steps: 1,
                thinking: Some(false),
                json_schema: Some(plan::schema(agents)),
            };
            let run = self.answer(&decision, &messages, &[], &lc).await?;
            let cost = self.record_run(&run, None)?;
            self.db.add_task_usage(
                &task.id,
                (run.prompt_tokens + run.completion_tokens) as i64,
                cost.unwrap_or(0.0),
            )?;
            plan::parse(&run.content, agents)
        };
        match attempt.await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(task = %task.id, "planification impossible, un seul agent : {e:#}");
                plan::single("assistant", &task.objective)
            }
        }
    }

    /// Exécute les étapes prêtes en parallèle jusqu'à ce qu'il n'en reste plus.
    async fn execute(
        self: &Arc<Self>,
        task: &Task,
        agents: &AgentSet,
        cancel: &CancelToken,
    ) -> Result<TaskStatus> {
        let mut running: JoinSet<(String, Result<(String, String)>)> = JoinSet::new();
        let mut in_flight: Vec<String> = Vec::new();
        loop {
            let (_, steps) = self.db.get_task(&task.id)?.context("tâche supprimée")?;
            let status: HashMap<&str, StepStatus> = steps
                .iter()
                .map(|s| (s.step_id.as_str(), s.status))
                .collect();
            let mut changed = false;
            for s in steps.iter().filter(|s| s.status == StepStatus::Pending) {
                if in_flight.contains(&s.step_id) {
                    continue;
                }
                let deps: Vec<StepStatus> = s
                    .depends_on
                    .iter()
                    .map(|d| status.get(d.as_str()).copied().unwrap_or(StepStatus::Done))
                    .collect();
                let blocked = deps.iter().any(|d| {
                    matches!(
                        d,
                        StepStatus::Failed | StepStatus::Skipped | StepStatus::Cancelled
                    )
                });
                // La synthèse attend la fin de toutes les étapes, puis se fait avec ce qui a
                // réussi (sauf si rien n'a réussi).
                let is_synthesis = s.step_id == plan::SYNTHESIS;
                let unfinished = deps.iter().any(|d| {
                    matches!(
                        d,
                        StepStatus::Pending | StepStatus::Running | StepStatus::Waiting
                    )
                });
                if is_synthesis && unfinished && !cancel.is_cancelled() {
                    continue;
                }
                let synthesis_ok = is_synthesis && deps.contains(&StepStatus::Done);
                if cancel.is_cancelled() {
                    self.db.set_step_status(
                        &task.id,
                        &s.step_id,
                        StepStatus::Cancelled,
                        None,
                        None,
                        None,
                    )?;
                    changed = true;
                } else if blocked && !synthesis_ok {
                    let why = "une étape dont elle dépend n'a pas abouti";
                    self.db.set_step_status(
                        &task.id,
                        &s.step_id,
                        StepStatus::Skipped,
                        None,
                        Some(why),
                        None,
                    )?;
                    changed = true;
                } else if deps.iter().all(|d| *d == StepStatus::Done) || synthesis_ok {
                    let inputs: Vec<(String, String, String)> = steps
                        .iter()
                        .filter(|d| {
                            s.depends_on.contains(&d.step_id) && d.status == StepStatus::Done
                        })
                        .map(|d| {
                            (
                                d.title.clone(),
                                d.agent.clone(),
                                d.output.clone().unwrap_or_default(),
                            )
                        })
                        .collect();
                    let agent = agents
                        .get(&s.agent)
                        .or_else(|| agents.get("assistant"))
                        .cloned()
                        .context("aucun agent disponible")?;
                    self.db.set_step_status(
                        &task.id,
                        &s.step_id,
                        StepStatus::Running,
                        None,
                        None,
                        None,
                    )?;
                    changed = true;
                    in_flight.push(s.step_id.clone());
                    let app = self.clone();
                    let (task, step, cancel) = (task.clone(), s.clone(), cancel.clone());
                    running.spawn(async move {
                        let id = step.step_id.clone();
                        (
                            id,
                            app.run_step(&task, &step, &agent, &inputs, &cancel).await,
                        )
                    });
                }
            }
            if changed {
                self.emit_event(AppEvent::TaskUpdated {
                    task_id: task.id.clone(),
                });
            }
            let Some(joined) = running.join_next().await else {
                break;
            };
            let (step_id, res) = joined.context("étape interrompue")?;
            in_flight.retain(|s| *s != step_id);
            match res {
                Ok((output, model)) => self.db.set_step_status(
                    &task.id,
                    &step_id,
                    if cancel.is_cancelled() {
                        StepStatus::Cancelled
                    } else {
                        StepStatus::Done
                    },
                    Some(&output),
                    None,
                    Some(&model),
                )?,
                Err(e) => self.db.set_step_status(
                    &task.id,
                    &step_id,
                    if cancel.is_cancelled() {
                        StepStatus::Cancelled
                    } else {
                        StepStatus::Failed
                    },
                    None,
                    Some(&format!("{e:#}")),
                    None,
                )?,
            }
            self.emit_event(AppEvent::TaskUpdated {
                task_id: task.id.clone(),
            });
        }

        // Bilan : le résultat est la synthèse, sinon la dernière étape réussie.
        let (_, steps) = self.db.get_task(&task.id)?.context("tâche supprimée")?;
        let done: Vec<&TaskStep> = steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .collect();
        let result = steps
            .iter()
            .find(|s| s.step_id == plan::SYNTHESIS && s.status == StepStatus::Done)
            .or(done.last().copied())
            .and_then(|s| s.output.clone());
        let status = if cancel.is_cancelled() {
            TaskStatus::Cancelled
        } else if done.len() == steps.len() {
            TaskStatus::Done
        } else if done.is_empty() {
            TaskStatus::Failed
        } else {
            TaskStatus::Partial
        };
        let error = (status == TaskStatus::Failed).then(|| {
            steps
                .iter()
                .find_map(|s| s.error.clone())
                .unwrap_or_else(|| "aucune étape n'a abouti".into())
        });
        self.db
            .set_task_status(&task.id, status, result.as_deref(), error.as_deref())?;
        Ok(status)
    }

    /// Exécute une étape avec son agent ; renvoie (résultat, modèle utilisé).
    async fn run_step(
        self: &Arc<Self>,
        task: &Task,
        step: &TaskStep,
        agent: &AgentDef,
        inputs: &[(String, String, String)],
        cancel: &CancelToken,
    ) -> Result<(String, String)> {
        let scope = format!("task:{}", task.id);
        let tools = self.tool_defs(|name| agent.allows_tool(name)).await;
        let mut user = format!(
            "Objectif global de la tâche :\n{}\n\nTa mission (étape « {} ») :\n{}",
            task.objective, step.title, step.instructions
        );
        if !inputs.is_empty() {
            user.push_str("\n\nRésultats des étapes précédentes :");
            for (title, agent, output) in inputs {
                user.push_str(&format!(
                    "\n\n### {title} ({agent})\n{}",
                    clip(output, DEP_OUTPUT_CHARS)
                ));
            }
        }
        user.push_str(
            "\n\nTu travailles en autonomie, sans pouvoir poser de question : fais au mieux \
             puis rends ton résultat complet, en français.",
        );
        let messages = vec![
            ChatMessage::text(
                "system",
                self.system_prompt(&agent.system_prompt, &scope, !tools.is_empty()),
            ),
            ChatMessage::text("user", user),
        ];
        let decision = self.route(mode_for(agent, self.config().routing.mode), &messages)?;

        // Une étape attend une place libre selon où elle s'exécute.
        let slots = match decision.target {
            Target::Local => self.local_slots.clone(),
            Target::Cloud => self.cloud_slots.clone(),
        };
        let _permit = tokio::select! {
            p = slots.acquire_owned() => p?,
            _ = wait_cancel(cancel) => bail!("annulée"),
        };

        let log = |line: String| {
            let _ = self.db.append_step_log(&task.id, &step.step_id, &line);
            self.emit_event(AppEvent::TaskUpdated {
                task_id: task.id.clone(),
            });
        };
        let emit = |ev: StreamEvent| match ev {
            StreamEvent::Token { text } => self.emit_event(AppEvent::StepOutput {
                task_id: task.id.clone(),
                step_id: step.step_id.clone(),
                text,
            }),
            StreamEvent::Route {
                target,
                model,
                reason,
            } => log(format!(
                "▶ {} · {model} · {reason}",
                if target == Target::Cloud {
                    "cloud"
                } else {
                    "local"
                }
            )),
            StreamEvent::ToolCall {
                name, arguments, ..
            } => log(format!("🔧 {name} {}", clip(&arguments, 300))),
            StreamEvent::ToolResult {
                is_error, content, ..
            } => log(format!(
                "{} {}",
                if is_error { "✗" } else { "✓" },
                clip(content.lines().last().unwrap_or(""), 200)
            )),
            _ => {}
        };
        let approver = TaskApprover {
            app: self.clone(),
            task_id: task.id.clone(),
            task_title: task.title.clone(),
            step_id: step.step_id.clone(),
            step_title: step.title.clone(),
            cancel: cancel.clone(),
        };
        let lc = LoopCtx {
            scope: &scope,
            approver: &approver,
            emit: &emit,
            cancel,
            max_steps: agent.max_steps,
            thinking: agent.thinking,
            json_schema: None,
        };
        let run = self.answer(&decision, &messages, &tools, &lc).await?;
        let cost = self.record_run(&run, None)?;
        self.db.add_task_usage(
            &task.id,
            (run.prompt_tokens + run.completion_tokens) as i64,
            if run.provider.is_some() {
                cost.unwrap_or(0.0)
            } else {
                0.0
            },
        )?;
        if run.cancelled {
            bail!("annulée");
        }
        if run.refused {
            bail!("le fournisseur a refusé de répondre");
        }
        if run.content.trim().is_empty() {
            bail!("l'agent n'a rien répondu");
        }
        Ok((run.content.clone(), model_label(&run)))
    }
}

/// Mode de routage d'un agent.
fn mode_for(agent: &AgentDef, default: RoutingMode) -> RoutingMode {
    match agent.model {
        ModelPref::Auto => default,
        ModelPref::Local => RoutingMode::Local,
        ModelPref::Cloud => RoutingMode::Cloud,
    }
}

async fn wait_cancel(cancel: &CancelToken) {
    while !cancel.is_cancelled() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Le planificateur n'a pas d'outils : toute demande est refusée.
struct DenyAll;

impl Approver for DenyAll {
    fn ask<'a>(&'a self, _: &'a str, _: &'a Approval) -> BoxFuture<'a, Answer> {
        Box::pin(async { Answer::Deny })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(parse_task_mode("plan"), Some(None));
        assert_eq!(parse_task_mode("agent:coder"), Some(Some("coder".into())));
        assert_eq!(parse_task_mode("autre"), None);
    }
}
