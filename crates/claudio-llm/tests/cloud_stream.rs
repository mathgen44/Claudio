//! Vérifie le parcours complet d'une demande en flux contre de faux serveurs
//! OpenAI et Anthropic (en-têtes, corps envoyé, lecture du flux).

use claudio_llm::chat::{self, Api, ChatEvent, ChatMessage, Endpoint, Options};
use claudio_models::CancelToken;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Répond une fois avec `sse` et renvoie la requête reçue (en-têtes, corps JSON).
async fn fake_server(sse: &'static str) -> (String, tokio::task::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        let (head, body) = loop {
            let n = sock.read(&mut buf).await.unwrap();
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw).to_string();
            if let Some(i) = text.find("\r\n\r\n") {
                let head = text[..i].to_ascii_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map(|v| v.trim().parse().unwrap())
                    .unwrap_or(0);
                if raw.len() >= i + 4 + len {
                    break (
                        head,
                        serde_json::from_slice(&raw[i + 4..i + 4 + len]).unwrap(),
                    );
                }
            }
        };
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
            sse.len()
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
        (head, body)
    });
    (base, handle)
}

fn messages() -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: "system".into(),
            content: "Tu es Claudio.".into(),
        },
        ChatMessage {
            role: "user".into(),
            content: "Salut".into(),
        },
    ]
}

#[tokio::test]
async fn openai_compatible_stream() {
    let (base, server) = fake_server(concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Bon\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"jour\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n"
    ))
    .await;
    let ep = Endpoint {
        api: Api::OpenAi { stream_usage: true },
        base_url: format!("{base}/v1"),
        model: "gpt-test".into(),
        api_key: Some("sk-test".into()),
    };
    let mut tokens = Vec::new();
    let done = chat::stream(
        &reqwest::Client::new(),
        &ep,
        &messages(),
        &Options::default(),
        &CancelToken::default(),
        |e| tokens.push(e),
    )
    .await
    .unwrap();
    let (head, body) = server.await.unwrap();
    assert!(head.starts_with("post /v1/chat/completions"));
    assert!(head.contains("authorization: bearer sk-test"));
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert!(body.get("chat_template_kwargs").is_none());
    assert_eq!(done.content, "Bonjour");
    assert_eq!(
        (done.prompt_tokens, done.completion_tokens),
        (Some(11), Some(2))
    );
    assert_eq!(tokens.len(), 2);
}

#[tokio::test]
async fn anthropic_stream() {
    let (base, server) = fake_server(concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Salutation.\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Bonjour !\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    ))
    .await;
    let ep = Endpoint {
        api: Api::Anthropic,
        base_url: base,
        model: "claude-opus-5-5".into(),
        api_key: Some("sk-ant-test".into()),
    };
    let mut events = Vec::new();
    let done = chat::stream(
        &reqwest::Client::new(),
        &ep,
        &messages(),
        &Options::default(),
        &CancelToken::default(),
        |e| events.push(e),
    )
    .await
    .unwrap();
    let (head, body) = server.await.unwrap();
    assert!(head.starts_with("post /v1/messages"));
    assert!(head.contains("x-api-key: sk-ant-test"));
    assert!(head.contains("anthropic-version: 2026-10-21"));
    assert!(head.contains("anthropic-beta: server-side-fallback-2026-07-01"));
    assert_eq!(body["system"], "Tu es Claudio.");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(done.content, "Bonjour !");
    assert_eq!(done.reasoning, "Salutation.");
    assert_eq!(
        (done.prompt_tokens, done.completion_tokens),
        (Some(9), Some(4))
    );
    assert_eq!(
        events[0],
        ChatEvent::Reasoning {
            text: "Salutation.".into()
        }
    );
}
