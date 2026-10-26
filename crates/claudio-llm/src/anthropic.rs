//! API Messages d'Anthropic (Claude), en flux SSE, par HTTP direct.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use claudio_models::CancelToken;

use crate::chat::{
    ChatEvent, ChatMessage, Completion, Endpoint, Options, ToolCall, http_error, read_sse, sse_data,
};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const API_VERSION: &str = "2026-10-21";
/// Plafond de sortie par défaut en flux (les modèles actuels acceptent bien plus).
const DEFAULT_MAX_TOKENS: u32 = 32_000;

/// Modèles pour lesquels Claudio active le repli côté serveur en cas de refus.
const SERVER_FALLBACK_MODELS: &[&str] = &[
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
];

/// Prix publics en dollars par million de jetons (entrée, sortie).
pub const PRICES: &[(&str, f64, f64)] = &[
    ("claude-fable-5-1", 10.0, 50.0),
    ("claude-fable-5", 10.0, 50.0),
    ("claude-opus-5-5", 4.0, 20.0),
    ("claude-opus-5", 5.0, 25.0),
    ("claude-opus-4-8", 5.0, 25.0),
    ("claude-opus-4-7", 5.0, 25.0),
    ("claude-opus-4-6", 5.0, 25.0),
    ("claude-sonnet-5-5", 2.0, 10.0),
    ("claude-sonnet-5", 2.0, 10.0),
    ("claude-sonnet-4-6", 3.0, 15.0),
    ("claude-haiku-5-5", 0.10, 0.50),
    ("claude-haiku-4-5", 1.0, 5.0),
];

pub fn price(model: &str) -> Option<(f64, f64)> {
    PRICES
        .iter()
        .find(|(m, ..)| *m == model)
        .map(|&(_, i, o)| (i, o))
}

/// Les modèles Claude récents réfléchissent de façon adaptative.
fn supports_adaptive_thinking(model: &str) -> bool {
    !(model.starts_with("claude-haiku-4") || model.starts_with("claude-3"))
}

pub(crate) fn request_body(endpoint: &Endpoint, messages: &[ChatMessage], opts: &Options) -> Value {
    // Le prompt système est un champ à part dans l'API Messages.
    let system: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .collect();
    let mut turns: Vec<Value> = Vec::new();
    for m in messages.iter().filter(|m| m.role != "system") {
        match m.role.as_str() {
            "tool" => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id,
                    "content": m.content,
                    "is_error": m.is_error,
                });
                // Tous les résultats d'un même tour vont dans un seul message utilisateur.
                let merge = turns.last().is_some_and(|t| {
                    t["role"] == "user"
                        && t["content"]
                            .as_array()
                            .is_some_and(|c| c.iter().all(|b| b["type"] == "tool_result"))
                });
                if merge {
                    let last = turns.last_mut().unwrap();
                    last["content"].as_array_mut().unwrap().push(block);
                } else {
                    turns.push(json!({"role": "user", "content": [block]}));
                }
            }
            "assistant" => {
                let content = m.raw.clone().unwrap_or_else(|| {
                    let mut blocks = Vec::new();
                    if !m.content.is_empty() || m.tool_calls.is_empty() {
                        blocks.push(json!({"type": "text", "text": m.content}));
                    }
                    for c in &m.tool_calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": c.id,
                            "name": c.name,
                            "input": serde_json::from_str::<Value>(&c.arguments).unwrap_or(json!({})),
                        }));
                    }
                    Value::Array(blocks)
                });
                turns.push(json!({"role": "assistant", "content": content}));
            }
            role => turns.push(json!({"role": role, "content": m.content})),
        }
    }
    let mut body = json!({
        "model": endpoint.model,
        "max_tokens": opts.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "messages": turns,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = system.join("\n\n").into();
    }
    if opts.thinking != Some(false) && supports_adaptive_thinking(&endpoint.model) {
        // Résumé lisible du raisonnement, affiché dans le bloc « Réflexion ».
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
    }
    if SERVER_FALLBACK_MODELS.contains(&endpoint.model.as_str()) {
        body["fallbacks"] = "default".into();
    }
    if !opts.tools.is_empty() {
        body["tools"] = opts
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                    // Les gros arguments (contenu de fichier) arrivent au fil de l'eau.
                    "eager_input_streaming": true,
                })
            })
            .collect();
    }
    body
}

/// Termine les blocs reçus : JSON des outils analysé, appels extraits.
pub(crate) fn finish(out: &mut Completion) {
    let mut calls = Vec::new();
    for b in &mut out.blocks {
        if b["type"] == "tool_use" {
            let partial = b
                .as_object_mut()
                .and_then(|o| o.remove("_partial"))
                .and_then(|p| p.as_str().map(str::to_owned))
                .unwrap_or_default();
            let text = if partial.trim().is_empty() {
                "{}".to_owned()
            } else {
                partial
            };
            // Une entrée illisible est renvoyée telle quelle : l'outil répondra par une
            // erreur que le modèle pourra corriger.
            b["input"] = serde_json::from_str(&text).unwrap_or(json!({}));
            calls.push(ToolCall {
                id: b["id"].as_str().unwrap_or_default().to_owned(),
                name: b["name"].as_str().unwrap_or_default().to_owned(),
                arguments: text,
            });
        }
    }
    out.tool_calls = calls;
    out.raw = (!out.blocks.is_empty()).then(|| Value::Array(std::mem::take(&mut out.blocks)));
}

fn parse_line(line: &str, out: &mut Completion) -> Result<Vec<ChatEvent>> {
    let Some(v) = sse_data(line)? else {
        return Ok(vec![]);
    };
    match v.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            if let Some(n) = v
                .pointer("/message/usage/input_tokens")
                .and_then(Value::as_u64)
            {
                let cached = ["cache_read_input_tokens", "cache_creation_input_tokens"]
                    .iter()
                    .filter_map(|k| {
                        v.pointer(&format!("/message/usage/{k}"))
                            .and_then(Value::as_u64)
                    })
                    .sum::<u64>();
                out.prompt_tokens = Some(n + cached);
            }
        }
        Some("content_block_start") => {
            let idx = v["index"].as_u64().unwrap_or(0) as usize;
            let mut block = v["content_block"].clone();
            if block["type"] == "tool_use" {
                block["_partial"] = "".into();
            }
            while out.blocks.len() <= idx {
                out.blocks.push(Value::Null);
            }
            out.blocks[idx] = block;
        }
        Some("content_block_delta") => {
            let delta = &v["delta"];
            let idx = v["index"].as_u64().unwrap_or(0) as usize;
            if let Some(block) = out.blocks.get_mut(idx) {
                let append = |b: &mut Value, key: &str, piece: &str| {
                    let cur = b[key].as_str().unwrap_or("").to_owned();
                    b[key] = format!("{cur}{piece}").into();
                };
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        append(block, "text", delta["text"].as_str().unwrap_or(""))
                    }
                    Some("thinking_delta") => {
                        append(block, "thinking", delta["thinking"].as_str().unwrap_or(""))
                    }
                    Some("signature_delta") => append(
                        block,
                        "signature",
                        delta["signature"].as_str().unwrap_or(""),
                    ),
                    Some("input_json_delta") => append(
                        block,
                        "_partial",
                        delta["partial_json"].as_str().unwrap_or(""),
                    ),
                    _ => {}
                }
            }
            let event = match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => delta["text"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(|t| ChatEvent::Token { text: t.to_owned() }),
                Some("thinking_delta") => delta["thinking"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(|t| ChatEvent::Reasoning { text: t.to_owned() }),
                _ => None,
            };
            return Ok(event.into_iter().collect());
        }
        Some("message_delta") => {
            if let Some(n) = v.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                out.completion_tokens = Some(n);
            }
            if v.pointer("/delta/stop_reason").and_then(Value::as_str) == Some("refusal") {
                out.refused = true;
            }
        }
        Some("error") => bail!(
            "erreur de Claude : {}",
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("inconnue")
        ),
        _ => {}
    }
    Ok(vec![])
}

fn headers(req: reqwest::RequestBuilder, key: &str, beta: Option<&str>) -> reqwest::RequestBuilder {
    let req = req
        .header("x-api-key", key)
        .header("anthropic-version", API_VERSION);
    match beta {
        Some(b) => req.header("anthropic-beta", b),
        None => req,
    }
}

pub(crate) async fn stream(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    messages: &[ChatMessage],
    opts: &Options,
    cancel: &CancelToken,
    on_event: &mut impl FnMut(ChatEvent),
) -> Result<Completion> {
    let key = endpoint
        .api_key
        .as_deref()
        .context("clé d'API Anthropic manquante")?;
    let body = request_body(endpoint, messages, opts);
    let beta = body
        .get("fallbacks")
        .map(|_| "server-side-fallback-2026-07-01");
    let url = format!("{}/v1/messages", endpoint.base_url.trim_end_matches('/'));
    let resp = headers(http.post(url), key, beta)
        .json(&body)
        .send()
        .await
        .context("Anthropic ne répond pas")?;
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }
    let mut out = Completion::default();
    read_sse(resp, cancel, &mut out, parse_line, on_event).await?;
    finish(&mut out);
    Ok(out)
}

/// Modèles disponibles pour cette clé.
pub async fn list_models(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
) -> Result<Vec<(String, String)>> {
    let url = format!("{}/v1/models?limit=100", base_url.trim_end_matches('/'));
    let resp = headers(http.get(url), key, None).send().await?;
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }
    let v: Value = resp.json().await?;
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_owned();
            let name = m["display_name"].as_str().unwrap_or(&id).to_owned();
            Some((id, name))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Api;

    fn ep(model: &str) -> Endpoint {
        Endpoint {
            api: Api::Anthropic,
            base_url: DEFAULT_BASE_URL.into(),
            model: model.into(),
            api_key: Some("k".into()),
        }
    }

    fn msgs() -> Vec<ChatMessage> {
        vec![
            ChatMessage::text("system", "Tu es Claudio."),
            ChatMessage::text("user", "Bonjour"),
        ]
    }

    #[test]
    fn body_moves_system_and_sets_thinking_and_fallbacks() {
        let b = request_body(&ep("claude-opus-5-5"), &msgs(), &Options::default());
        assert_eq!(b["system"], "Tu es Claudio.");
        assert_eq!(b["messages"].as_array().unwrap().len(), 1);
        assert_eq!(b["thinking"]["type"], "adaptive");
        assert_eq!(b["fallbacks"], "default");
        assert!(b.get("temperature").is_none());

        let haiku = request_body(&ep("claude-haiku-4-5"), &msgs(), &Options::default());
        assert!(haiku.get("thinking").is_none());
        assert!(haiku.get("fallbacks").is_none());

        let off = request_body(
            &ep("claude-sonnet-5-5"),
            &msgs(),
            &Options {
                thinking: Some(false),
                ..Default::default()
            },
        );
        assert!(off.get("thinking").is_none());
    }

    #[test]
    fn parses_stream() {
        let mut c = Completion::default();
        let lines = [
            "event: message_start",
            r#"data: {"type":"message_start","message":{"id":"m","usage":{"input_tokens":12,"cache_read_input_tokens":100}}}"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Je salue."}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Bonjour"}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":" !"}}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
            r#"data: {"type":"message_stop"}"#,
        ];
        for l in lines {
            for e in parse_line(l, &mut c).unwrap() {
                c.push(&e);
            }
        }
        assert_eq!(c.content, "Bonjour !");
        assert_eq!(c.reasoning, "Je salue.");
        assert_eq!(c.prompt_tokens, Some(112));
        assert_eq!(c.completion_tokens, Some(7));
        assert!(!c.refused);
        parse_line(r#"data: {"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":0}}"#, &mut c).unwrap();
        assert!(c.refused);
        assert!(parse_line(r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#, &mut c).is_err());
    }

    #[test]
    fn tool_use_round_trip() {
        let mut c = Completion::default();
        let lines = [
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Je lis."}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig=="}}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu_1","name":"read_file","input":{}}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"C:/a"}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":".txt\"}"}}"#,
        ];
        for l in lines {
            for e in parse_line(l, &mut c).unwrap() {
                c.push(&e);
            }
        }
        finish(&mut c);
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].id, "tu_1");
        assert_eq!(c.tool_calls[0].arguments, r#"{"path": "C:/a.txt"}"#);
        let raw = c.raw.clone().unwrap();
        assert_eq!(raw[0]["signature"], "sig==");
        assert_eq!(raw[1]["input"]["path"], "C:/a.txt");
        assert!(raw[1].get("_partial").is_none());

        // Tour suivant : blocs natifs renvoyés tels quels, résultats regroupés.
        let msgs = vec![
            ChatMessage::text("user", "Lis a.txt et b.txt"),
            ChatMessage::assistant(String::new(), c.tool_calls.clone(), c.raw.clone()),
            ChatMessage::tool_result("tu_1", "contenu a".into(), false),
            ChatMessage::tool_result("tu_2", "introuvable".into(), true),
        ];
        let opts = Options {
            tools: vec![crate::chat::ToolDef {
                name: "read_file".into(),
                description: "Lit".into(),
                parameters: json!({"type": "object"}),
            }],
            ..Default::default()
        };
        let b = request_body(&ep("claude-opus-5-5"), &msgs, &opts);
        let turns = b["messages"].as_array().unwrap();
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[1]["content"][0]["type"], "thinking");
        assert_eq!(turns[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(turns[2]["content"][1]["is_error"], true);
        assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(b["tools"][0]["eager_input_streaming"], true);
    }

    #[test]
    fn prices() {
        assert_eq!(price("claude-opus-5-5"), Some((4.0, 20.0)));
        assert_eq!(price("inconnu"), None);
    }
}
