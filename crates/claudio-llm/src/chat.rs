//! Client de conversation en continu pour les API compatibles OpenAI
//! (`/v1/chat/completions`) : llama-server, Ollama et la plupart des fournisseurs.

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use claudio_models::CancelToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Point d'accès d'un modèle, qu'il soit local ou distant.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// Base de l'API, ex. `http://127.0.0.1:8080/v1`.
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub temperature: Option<f32>,
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
    pub completion_tokens: Option<u64>,
    pub tokens_per_sec: Option<f64>,
    pub cancelled: bool,
}

/// Interprète une ligne d'un flux SSE. `Ok(None)` pour les lignes sans donnée utile.
fn parse_sse_line(line: &str, out: &mut Completion) -> Result<Vec<ChatEvent>> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(vec![]);
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(vec![]);
    }
    let v: Value =
        serde_json::from_str(data).with_context(|| format!("réponse illisible : {data}"))?;
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
                if is_reasoning {
                    out.reasoning.push_str(text);
                    events.push(ChatEvent::Reasoning {
                        text: text.to_owned(),
                    });
                } else {
                    out.content.push_str(text);
                    events.push(ChatEvent::Token {
                        text: text.to_owned(),
                    });
                }
            }
        }
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
    Ok(events)
}

/// Envoie la conversation et transmet la réponse au fil de l'eau.
pub async fn stream_chat(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    messages: &[ChatMessage],
    opts: &Options,
    cancel: &CancelToken,
    mut on_event: impl FnMut(ChatEvent),
) -> Result<Completion> {
    let mut body = serde_json::json!({
        "model": endpoint.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(t) = opts.temperature {
        body["temperature"] = t.into();
    }
    if let Some(m) = opts.max_tokens {
        body["max_tokens"] = m.into();
    }
    if let Some(t) = opts.thinking {
        // Reconnu par les modèles de conversation intégrés aux GGUF récents (llama-server).
        body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": t });
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
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        bail!("le modèle a refusé la demande ({status}) : {text}");
    }

    let mut out = Completion::default();
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
            for ev in parse_sse_line(line.trim_end(), &mut out)? {
                on_event(ev);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deltas_usage_and_timings() {
        let mut c = Completion::default();
        let ev = parse_sse_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"Hmm"}}]}"#,
            &mut c,
        )
        .unwrap();
        assert_eq!(ev, vec![ChatEvent::Reasoning { text: "Hmm".into() }]);
        let ev =
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Bon"}}]}"#, &mut c).unwrap();
        assert_eq!(ev, vec![ChatEvent::Token { text: "Bon".into() }]);
        parse_sse_line(
            r#"data: {"choices":[{"delta":{"content":"jour"}}]}"#,
            &mut c,
        )
        .unwrap();
        parse_sse_line(
            r#"data: {"choices":[],"usage":{"completion_tokens":3},"timings":{"predicted_per_second":91.5}}"#,
            &mut c,
        )
        .unwrap();
        assert!(parse_sse_line("data: [DONE]", &mut c).unwrap().is_empty());
        assert!(parse_sse_line(": keep-alive", &mut c).unwrap().is_empty());
        assert_eq!(c.content, "Bonjour");
        assert_eq!(c.reasoning, "Hmm");
        assert_eq!(c.completion_tokens, Some(3));
        assert_eq!(c.tokens_per_sec, Some(91.5));
    }

    #[test]
    fn surfaces_errors() {
        let mut c = Completion::default();
        assert!(parse_sse_line(r#"data: {"error":{"message":"context full"}}"#, &mut c).is_err());
    }
}
