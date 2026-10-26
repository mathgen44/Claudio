//! Conversation en continu avec un modèle, local ou distant.
//!
//! Deux familles d'API : compatible OpenAI (`/chat/completions` : llama-server,
//! Ollama, OpenAI, Mistral, Gemini, OpenRouter…) et l'API Messages d'Anthropic.

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use claudio_models::CancelToken;

use crate::anthropic;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Famille d'API d'un point d'accès.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// Moteur local (llama-server, Ollama) : options de modèle de conversation acceptées.
    Local,
    /// Fournisseur compatible OpenAI ; `stream_usage` s'il accepte `stream_options`.
    OpenAi {
        stream_usage: bool,
    },
    Anthropic,
}

/// Point d'accès d'un modèle, qu'il soit local ou distant.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub api: Api,
    /// Base de l'API, ex. `http://127.0.0.1:8080/v1` ou `https://api.anthropic.com`.
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl Endpoint {
    pub fn is_local(&self) -> bool {
        self.api == Api::Local
    }
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub max_tokens: Option<u32>,
    /// Active ou coupe le raisonnement des modèles qui le proposent.
    pub thinking: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChatEvent {
    /// Fragment de la réponse.
    Token { text: String },
    /// Fragment du raisonnement (modèles « qui réfléchissent »).
    Reasoning { text: String },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Completion {
    pub content: String,
    pub reasoning: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub tokens_per_sec: Option<f64>,
    pub cancelled: bool,
    /// Le modèle a refusé de répondre (filtres de sécurité du fournisseur).
    pub refused: bool,
}

impl Completion {
    pub(crate) fn push(&mut self, ev: &ChatEvent) {
        match ev {
            ChatEvent::Token { text } => self.content.push_str(text),
            ChatEvent::Reasoning { text } => self.reasoning.push_str(text),
        }
    }
}

/// Interprète une ligne `data:` d'un flux compatible OpenAI.
fn parse_openai_line(line: &str, out: &mut Completion) -> Result<Vec<ChatEvent>> {
    let Some(v) = sse_data(line)? else {
        return Ok(vec![]);
    };
    if let Some(err) = v.get("error") {
        bail!(
            "erreur du modèle : {}",
            err.get("message")
                .and_then(Value::as_str)
                .unwrap_or(&err.to_string())
        );
    }
    let mut events = Vec::new();
    if let Some(delta) = v.pointer("/choices/0/delta") {
        for (key, is_reasoning) in [
            ("reasoning_content", true),
            ("reasoning", true),
            ("content", false),
        ] {
            if let Some(text) = delta
                .get(key)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                let text = text.to_owned();
                events.push(if is_reasoning {
                    ChatEvent::Reasoning { text }
                } else {
                    ChatEvent::Token { text }
                });
            }
        }
    }
    if v.pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        == Some("content_filter")
    {
        out.refused = true;
    }
    if let Some(n) = v.pointer("/usage/prompt_tokens").and_then(Value::as_u64) {
        out.prompt_tokens = Some(n);
    }
    if let Some(n) = v
        .pointer("/usage/completion_tokens")
        .and_then(Value::as_u64)
    {
        out.completion_tokens = Some(n);
    }
    // llama-server ajoute ses mesures de vitesse au dernier fragment.
    if let Some(tps) = v
        .pointer("/timings/predicted_per_second")
        .and_then(Value::as_f64)
    {
        out.tokens_per_sec = Some(tps);
    }
    if let Some(n) = v.pointer("/timings/predicted_n").and_then(Value::as_u64) {
        out.completion_tokens.get_or_insert(n);
    }
    if let Some(n) = v.pointer("/timings/prompt_n").and_then(Value::as_u64) {
        out.prompt_tokens.get_or_insert(n);
    }
    Ok(events)
}

/// Contenu JSON d'une ligne SSE `data:`, ou `None` pour les autres lignes.
pub(crate) fn sse_data(line: &str) -> Result<Option<Value>> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(None);
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_str(data).with_context(|| format!("réponse illisible : {data}"))?,
    ))
}

/// Lit un flux SSE ligne par ligne jusqu'à sa fin ou son annulation.
pub(crate) async fn read_sse(
    resp: reqwest::Response,
    cancel: &CancelToken,
    out: &mut Completion,
    mut on_line: impl FnMut(&str, &mut Completion) -> Result<Vec<ChatEvent>>,
    on_event: &mut impl FnMut(ChatEvent),
) -> Result<()> {
    let mut buf = Vec::<u8>::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            out.cancelled = true;
            break;
        }
        buf.extend_from_slice(&chunk.context("flux interrompu")?);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            for ev in on_line(line.trim_end(), out)? {
                out.push(&ev);
                on_event(ev);
            }
        }
    }
    Ok(())
}

/// Message d'erreur lisible pour une réponse HTTP en échec.
pub(crate) async fn http_error(resp: reqwest::Response) -> anyhow::Error {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or(text);
    let hint = match status.as_u16() {
        401 | 403 => " — clé d'API refusée",
        429 => " — trop de demandes ou quota épuisé",
        _ => "",
    };
    anyhow::anyhow!("le modèle a refusé la demande ({status}{hint}) : {detail}")
}

/// Envoie la conversation et transmet la réponse au fil de l'eau.
pub async fn stream(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    messages: &[ChatMessage],
    opts: &Options,
    cancel: &CancelToken,
    mut on_event: impl FnMut(ChatEvent),
) -> Result<Completion> {
    if endpoint.api == Api::Anthropic {
        return anthropic::stream(http, endpoint, messages, opts, cancel, &mut on_event).await;
    }
    let mut body = serde_json::json!({
        "model": endpoint.model,
        "messages": messages,
        "stream": true,
    });
    match endpoint.api {
        Api::Local => {
            body["stream_options"] = serde_json::json!({ "include_usage": true });
            if let Some(t) = opts.thinking {
                // Reconnu par les modèles de conversation intégrés aux GGUF récents.
                body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": t });
            }
        }
        Api::OpenAi { stream_usage: true } => {
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        _ => {}
    }
    if let Some(m) = opts.max_tokens {
        body["max_tokens"] = m.into();
    }
    let mut req = http
        .post(format!(
            "{}/chat/completions",
            endpoint.base_url.trim_end_matches('/')
        ))
        .json(&body);
    if let Some(key) = &endpoint.api_key {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.context("le modèle ne répond pas")?;
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }
    let mut out = Completion::default();
    read_sse(resp, cancel, &mut out, parse_openai_line, &mut on_event).await?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(c: &mut Completion, line: &str) -> Vec<ChatEvent> {
        let ev = parse_openai_line(line, c).unwrap();
        ev.iter().for_each(|e| c.push(e));
        ev
    }

    #[test]
    fn parses_deltas_usage_and_timings() {
        let mut c = Completion::default();
        let ev = feed(
            &mut c,
            r#"data: {"choices":[{"delta":{"reasoning_content":"Hmm"}}]}"#,
        );
        assert_eq!(ev, vec![ChatEvent::Reasoning { text: "Hmm".into() }]);
        let ev = feed(&mut c, r#"data: {"choices":[{"delta":{"content":"Bon"}}]}"#);
        assert_eq!(ev, vec![ChatEvent::Token { text: "Bon".into() }]);
        feed(
            &mut c,
            r#"data: {"choices":[{"delta":{"content":"jour"}}]}"#,
        );
        feed(
            &mut c,
            r#"data: {"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":3},"timings":{"predicted_per_second":91.5}}"#,
        );
        assert!(feed(&mut c, "data: [DONE]").is_empty());
        assert!(feed(&mut c, ": keep-alive").is_empty());
        assert_eq!(c.content, "Bonjour");
        assert_eq!(c.reasoning, "Hmm");
        assert_eq!(c.prompt_tokens, Some(20));
        assert_eq!(c.completion_tokens, Some(3));
        assert_eq!(c.tokens_per_sec, Some(91.5));
    }

    #[test]
    fn surfaces_errors() {
        let mut c = Completion::default();
        assert!(
            parse_openai_line(r#"data: {"error":{"message":"context full"}}"#, &mut c).is_err()
        );
    }
}
