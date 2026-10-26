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

/// Message d'une conversation, y compris les appels d'outils et leurs résultats.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatMessage {
    /// `system`, `user`, `assistant` ou `tool` (résultat d'outil).
    pub role: String,
    pub content: String,
    /// Outils demandés par le modèle (rôle `assistant`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Appel auquel répond ce résultat (rôle `tool`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    /// Blocs natifs du fournisseur (Anthropic : réflexion signée), renvoyés tels quels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

impl ChatMessage {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_owned(),
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn assistant(content: String, tool_calls: Vec<ToolCall>, raw: Option<Value>) -> Self {
        Self {
            role: "assistant".into(),
            content,
            tool_calls,
            raw,
            ..Default::default()
        }
    }

    pub fn tool_result(call_id: &str, content: String, is_error: bool) -> Self {
        Self {
            role: "tool".into(),
            content,
            tool_call_id: Some(call_id.to_owned()),
            is_error,
            ..Default::default()
        }
    }
}

/// Appel d'outil demandé par le modèle ; `arguments` est le JSON brut reçu.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Outil proposé au modèle.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// Schéma JSON des paramètres.
    pub parameters: Value,
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
    pub tools: Vec<ToolDef>,
    /// Réponse contrainte à ce schéma JSON (sortie structurée), si le fournisseur le permet.
    pub json_schema: Option<Value>,
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
    pub tool_calls: Vec<ToolCall>,
    /// Contenu natif de la réponse (Anthropic), à renvoyer tel quel au tour suivant.
    #[serde(skip)]
    pub raw: Option<Value>,
    /// Blocs en cours de construction (Anthropic).
    #[serde(skip)]
    pub(crate) blocks: Vec<Value>,
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
        // Les appels d'outils arrivent en morceaux, repérés par leur index.
        for tc in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            while out.tool_calls.len() <= idx {
                out.tool_calls.push(ToolCall::default());
            }
            let call = &mut out.tool_calls[idx];
            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                call.id = id.to_owned();
            }
            if let Some(n) = tc.pointer("/function/name").and_then(Value::as_str) {
                call.name.push_str(n);
            }
            if let Some(a) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                call.arguments.push_str(a);
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
        "messages": messages.iter().map(openai_message).collect::<Vec<_>>(),
        "stream": true,
    });
    if !opts.tools.is_empty() {
        body["tools"] = opts
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.parameters
                }})
            })
            .collect();
    }
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
    // llama-server en fait une grammaire ; OpenAI et OpenRouter l'acceptent tel quel.
    if let Some(schema) = &opts.json_schema
        && matches!(
            endpoint.api,
            Api::Local | Api::OpenAi { stream_usage: true }
        )
    {
        body["response_format"] = serde_json::json!({
            "type": "json_schema",
            "json_schema": {"name": "reponse", "schema": schema, "strict": true}
        });
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
    out.tool_calls.retain(|c| !c.name.is_empty());
    for (i, c) in out.tool_calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{i}");
        }
    }
    Ok(out)
}

fn openai_message(m: &ChatMessage) -> Value {
    match m.role.as_str() {
        "tool" => serde_json::json!({
            "role": "tool",
            "tool_call_id": m.tool_call_id,
            "content": m.content,
        }),
        "assistant" if !m.tool_calls.is_empty() => serde_json::json!({
            "role": "assistant",
            "content": if m.content.is_empty() { Value::Null } else { m.content.clone().into() },
            "tool_calls": m.tool_calls.iter().map(|c| serde_json::json!({
                "id": c.id,
                "type": "function",
                "function": {"name": c.name, "arguments": c.arguments},
            })).collect::<Vec<_>>(),
        }),
        _ => serde_json::json!({"role": m.role, "content": m.content}),
    }
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
    fn accumulates_tool_calls() {
        let mut c = Completion::default();
        feed(
            &mut c,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":""}}]}}]}"#,
        );
        feed(
            &mut c,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}"#,
        );
        feed(
            &mut c,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"C:/a\"}"}}]}}]}"#,
        );
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].name, "read_file");
        assert_eq!(c.tool_calls[0].arguments, r#"{"path":"C:/a"}"#);
        let m = openai_message(&ChatMessage::assistant(
            String::new(),
            c.tool_calls.clone(),
            None,
        ));
        assert_eq!(m["tool_calls"][0]["function"]["name"], "read_file");
        assert!(m["content"].is_null());
        let r = openai_message(&ChatMessage::tool_result("c1", "ok".into(), false));
        assert_eq!(r["tool_call_id"], "c1");
    }

    #[test]
    fn surfaces_errors() {
        let mut c = Completion::default();
        assert!(
            parse_openai_line(r#"data: {"error":{"message":"context full"}}"#, &mut c).is_err()
        );
    }
}
