//! Conversations : routage, réponse en continu, repli et comptabilité.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use claudio_core::config::{ActiveModel, RoutingMode};
use claudio_core::usage::UsageRecord;
use claudio_core::{Message, MessageMeta, Role, Session};
use claudio_llm::chat::{self, ChatEvent, ChatMessage, Completion, Endpoint, Options};
use claudio_llm::providers;
use claudio_models::CancelToken;
use claudio_models::select::DEFAULT_CONTEXT;
use serde::Serialize;

use crate::cloud::cost;
use crate::router::{self, Decision, RouteInput, Target};
use crate::{Claudio, DEFAULT_SESSION_TITLE};

/// Événements envoyés à l'interface pendant une réponse.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StreamEvent {
    /// Qui répond et pourquoi ; un nouvel événement annule le texte reçu jusque-là.
    Route {
        target: Target,
        model: String,
        reason: String,
    },
    Token {
        text: String,
    },
    Reasoning {
        text: String,
    },
}

/// Fin d'une réponse : le message enregistré et quelques mesures.
#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub message: Message,
    pub tokens_per_sec: Option<f64>,
    pub cancelled: bool,
}

/// Estimation grossière : ~3,5 caractères par jeton, plus l'enveloppe de chaque message.
pub(crate) fn estimate_tokens(messages: &[ChatMessage]) -> u32 {
    messages
        .iter()
        .map(|m| m.content.chars().count() as u32 * 2 / 7 + 4)
        .sum()
}

/// Garde le prompt système et les messages les plus récents qui tiennent dans `budget`.
pub(crate) fn trim_history(messages: &[ChatMessage], budget: u32) -> Vec<ChatMessage> {
    let (system, rest): (Vec<_>, Vec<_>) = messages.iter().partition(|m| m.role == "system");
    let mut used = estimate_tokens(&system.iter().map(|m| (*m).clone()).collect::<Vec<_>>());
    let mut kept = Vec::new();
    for m in rest.iter().rev() {
        let t = estimate_tokens(std::slice::from_ref(*m));
        // Le dernier message est toujours gardé, même s'il dépasse.
        if used + t > budget && !kept.is_empty() {
            break;
        }
        used += t;
        kept.push((*m).clone());
    }
    kept.reverse();
    system.into_iter().cloned().chain(kept).collect()
}

fn parse_mode(s: &str) -> Option<RoutingMode> {
    match s {
        "local" => Some(RoutingMode::Local),
        "auto" => Some(RoutingMode::Auto),
        "cloud" => Some(RoutingMode::Cloud),
        _ => None,
    }
}

impl Claudio {
    pub fn create_session(&self) -> Result<Session> {
        self.db.create_session(DEFAULT_SESSION_TITLE)
    }

    /// Mode de routage propre à une conversation (`None` = réglage général).
    pub fn set_session_route(&self, session_id: &str, mode: Option<&str>) -> Result<()> {
        if let Some(m) = mode
            && parse_mode(m).is_none()
        {
            bail!("mode inconnu : {m}");
        }
        self.db.set_session_route(session_id, mode)
    }

    pub fn cancel_chat(&self, session_id: &str) {
        if let Some(t) = self.chats.lock().unwrap().get(session_id) {
            t.cancel();
        }
    }

    fn local_context(&self) -> u32 {
        let cfg = self.config();
        let model_max = match &cfg.active_model {
            Some(ActiveModel::Local { id }) => self
                .registry()
                .get(id)
                .ok()
                .flatten()
                .and_then(|m| m.context_max),
            _ => None,
        };
        cfg.context_size
            .unwrap_or(DEFAULT_CONTEXT)
            .min(model_max.unwrap_or(u32::MAX))
    }

    /// Ajoute le message de l'utilisateur, fait répondre le modèle choisi par le
    /// routeur en continu et enregistre la réponse et sa consommation.
    pub async fn send_message(
        &self,
        session_id: &str,
        text: &str,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<Reply> {
        let session = self
            .db
            .get_session(session_id)?
            .context("conversation introuvable")?;
        let text = text.trim();
        if text.is_empty() {
            bail!("message vide");
        }
        self.db
            .add_message(session_id, Role::User, text, MessageMeta::default())?;
        if session.title == DEFAULT_SESSION_TITLE {
            self.db.rename_session(session_id, &title_from(text))?;
        }

        let cfg = self.config();
        let mut messages = vec![ChatMessage {
            role: "system".into(),
            content: cfg.system_prompt.clone(),
        }];
        messages.extend(
            self.db
                .list_messages(session_id)?
                .into_iter()
                .map(|m| ChatMessage {
                    role: m.role.as_str().to_owned(),
                    content: m.content,
                }),
        );

        let cloud = self.cloud_endpoint()?;
        let budget = self.budget_status()?;
        let local_context = self.local_context();
        let decision = router::decide(&RouteInput {
            mode: session
                .route
                .as_deref()
                .and_then(parse_mode)
                .unwrap_or(cfg.routing.mode),
            local_ready: cfg.active_model.is_some(),
            local_context,
            prompt_tokens: estimate_tokens(&messages),
            cloud_ready: cloud.is_some(),
            budget_ok: !budget.exceeded,
        })?;
        tracing::info!(target = ?decision.target, reason = %decision.reason, "routage");

        let cancel = CancelToken::default();
        self.chats
            .lock()
            .unwrap()
            .insert(session_id.to_owned(), cancel.clone());
        let started = Instant::now();
        let res = self
            .answer(&decision, &messages, local_context, &cancel, &mut on_event)
            .await;
        self.chats.lock().unwrap().remove(session_id);
        let (endpoint, provider, reason, done) = res?;

        let input = done
            .prompt_tokens
            .unwrap_or_else(|| estimate_tokens(&messages) as u64);
        let output = done.completion_tokens.unwrap_or(0);
        let price = match &provider {
            None => Some((0.0, 0.0)),
            Some(p) => providers::price(p, &endpoint.model),
        };
        let cost_usd = cost(price, input, output);
        self.db.record_usage(&UsageRecord {
            session_id: Some(session_id),
            provider: provider.as_ref().map_or("local", |p| p.id.as_str()),
            model: &endpoint.model,
            local: provider.is_none(),
            input_tokens: input,
            output_tokens: output,
            cost_usd,
        })?;

        let content = match (done.content.is_empty(), done.cancelled, done.refused) {
            (_, _, true) => "*(le fournisseur a refusé de répondre à cette demande)*".to_owned(),
            (true, true, _) => "*(réponse interrompue)*".to_owned(),
            _ => done.content,
        };
        let message = self.db.add_message(
            session_id,
            Role::Assistant,
            &content,
            MessageMeta {
                model: Some(match &provider {
                    Some(p) => format!("{} · {}", endpoint.model, p.name),
                    None => endpoint.model.clone(),
                }),
                tokens: done.completion_tokens.map(|n| n as i64),
                duration_ms: Some(started.elapsed().as_millis() as i64),
                reasoning: Some(done.reasoning).filter(|r| !r.is_empty()),
                route: Some(reason),
                cost_usd: provider.as_ref().and(cost_usd),
            },
        )?;
        Ok(Reply {
            message,
            tokens_per_sec: done.tokens_per_sec,
            cancelled: done.cancelled,
        })
    }

    /// Fait répondre la cible choisie, puis la cible de secours si la première
    /// échoue avant d'avoir produit du texte.
    async fn answer(
        &self,
        decision: &Decision,
        messages: &[ChatMessage],
        local_context: u32,
        cancel: &CancelToken,
        on_event: &mut impl FnMut(StreamEvent),
    ) -> Result<(
        Endpoint,
        Option<claudio_core::config::ProviderConfig>,
        String,
        Completion,
    )> {
        let mut attempts = vec![(decision.target, decision.reason.clone())];
        if let Some(f) = decision.fallback {
            attempts.push((f, String::new()));
        }
        let mut last_err = None;
        for (i, (target, reason)) in attempts.into_iter().enumerate() {
            let reason = if i == 0 {
                reason
            } else {
                format!(
                    "repli {} : {}",
                    if target == Target::Cloud {
                        "cloud"
                    } else {
                        "local"
                    },
                    last_err
                        .as_ref()
                        .map_or(String::new(), |e: &anyhow::Error| format!("{e:#}"))
                )
            };
            let prepared = match target {
                Target::Local => self.active_endpoint().await.map(|ep| (ep, None)),
                Target::Cloud => self
                    .cloud_endpoint()
                    .and_then(|c| c.context("modèle distant indisponible"))
                    .map(|(p, ep)| (ep, Some(p))),
            };
            let (endpoint, provider) = match prepared {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("préparation de {target:?} impossible : {e:#}");
                    last_err = Some(e);
                    continue;
                }
            };
            on_event(StreamEvent::Route {
                target,
                model: endpoint.model.clone(),
                reason: reason.clone(),
            });
            let msgs = if target == Target::Local {
                trim_history(messages, router::local_prompt_budget(local_context))
            } else {
                messages.to_vec()
            };
            let opts = Options {
                thinking: self.thinking_for(target),
                ..Default::default()
            };
            let mut produced = false;
            let res = chat::stream(&self.http, &endpoint, &msgs, &opts, cancel, |ev| {
                produced = true;
                on_event(match ev {
                    ChatEvent::Token { text } => StreamEvent::Token { text },
                    ChatEvent::Reasoning { text } => StreamEvent::Reasoning { text },
                })
            })
            .await;
            match res {
                Ok(done) => return Ok((endpoint, provider, reason, done)),
                // Une réponse déjà commencée n'est pas rejouée ailleurs.
                Err(e) if produced || cancel.is_cancelled() => return Err(e),
                Err(e) => {
                    tracing::warn!("{target:?} a échoué : {e:#}");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("aucun modèle n'a pu répondre")))
    }

    /// Raisonnement : choix de l'utilisateur ; sinon, en local, activé pour les
    /// modèles assez grands (les petits tournent en rond quand ils réfléchissent).
    fn thinking_for(&self, target: Target) -> Option<bool> {
        let cfg = self.config();
        if target == Target::Cloud {
            return cfg.thinking;
        }
        match &cfg.active_model {
            Some(ActiveModel::Local { id }) => {
                let params = self
                    .registry()
                    .get(id)
                    .ok()
                    .flatten()
                    .and_then(|m| m.params_b)
                    .or_else(|| {
                        claudio_models::catalog::split_key(id)
                            .and_then(|(m, _)| self.catalog.get(m))
                            .map(|m| m.params_b)
                    })
                    .unwrap_or(0.0);
                Some(cfg.thinking.unwrap_or(params >= 9.0))
            }
            _ => cfg.thinking,
        }
    }
}

/// Titre court tiré du premier message.
fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    let mut title: String = line.chars().take(48).collect();
    if line.chars().count() > 48 {
        title.push('…');
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, n: usize) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: "x".repeat(n),
        }
    }

    #[test]
    fn titles() {
        assert_eq!(title_from("Bonjour\nsuite"), "Bonjour");
        assert!(title_from(&"a".repeat(100)).ends_with('…'));
    }

    #[test]
    fn trims_oldest_messages_first() {
        let msgs = vec![
            msg("system", 70),
            msg("user", 700),
            msg("assistant", 700),
            msg("user", 70),
        ];
        // système ≈ 24, chaque gros message ≈ 204, le dernier ≈ 24.
        let kept = trim_history(&msgs, 300);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].role, "system");
        assert_eq!(kept[1].role, "assistant");
        assert_eq!(kept.last().unwrap().content.len(), 70);
        assert_eq!(trim_history(&msgs, 10_000).len(), 4);
        // Le dernier message passe toujours.
        assert_eq!(trim_history(&msgs, 1).len(), 2);
    }
}
