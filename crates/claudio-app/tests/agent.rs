//! Boucle d'agent de bout en bout : un faux modèle (compatible OpenAI) demande un
//! outil, l'utilisateur autorise, le résultat revient au modèle qui conclut.

use std::sync::{Arc, Mutex};

use claudio_app::claudio_core::AppPaths;
use claudio_app::claudio_core::config::{CloudModel, ProviderKind, RoutingMode};
use claudio_app::claudio_perms::Answer;
use claudio_app::{Claudio, StreamEvent};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Faux fournisseur : répond successivement avec chaque flux SSE donné et garde
/// les corps de requête reçus.
async fn fake_model(replies: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = bodies.clone();
    tokio::spawn(async move {
        for sse in replies {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 1 << 20];
            let mut got = 0;
            let body = loop {
                got += sock.read(&mut buf[got..]).await.unwrap();
                let text = String::from_utf8_lossy(&buf[..got]).to_string();
                if let Some(i) = text.find("\r\n\r\n") {
                    let len: usize = text[..i]
                        .to_ascii_lowercase()
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if got >= i + 4 + len {
                        break serde_json::from_slice::<Value>(&buf[i + 4..i + 4 + len]).unwrap();
                    }
                }
            };
            seen.lock().unwrap().push(body);
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                sse.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        }
    });
    (base, bodies)
}

fn tool_call_sse(path: &str) -> String {
    let args = serde_json::to_string(&serde_json::json!({ "path": path })).unwrap();
    let chunk = serde_json::json!({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "id": "call_1", "function": {"name": "read_file", "arguments": args}}
    ]}}]});
    format!(
        "data: {chunk}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":10}}}}\n\ndata: [DONE]\n\n"
    )
}

fn text_sse(text: &str) -> String {
    let chunk = serde_json::json!({"choices": [{"delta": {"content": text}}]});
    format!(
        "data: {chunk}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":200,\"completion_tokens\":20}}}}\n\ndata: [DONE]\n\n"
    )
}

async fn setup(replies: Vec<String>) -> (tempfile::TempDir, Arc<Claudio>, Arc<Mutex<Vec<Value>>>) {
    let dir = tempfile::tempdir().unwrap();
    let (base, bodies) = fake_model(replies).await;
    let app = Claudio::open(AppPaths::in_dir(dir.path().join("claudio"))).unwrap();
    let p = app
        .add_provider(ProviderKind::Custom, "Faux", Some(&base), None)
        .unwrap();
    app.set_cloud_model(
        Some(CloudModel {
            provider_id: p.config.id,
            model: "faux".into(),
        }),
        Some((1.0, 1.0)),
    )
    .unwrap();
    app.set_routing_mode(RoutingMode::Cloud).unwrap();
    (dir, app, bodies)
}

#[tokio::test]
async fn tool_call_with_approval() {
    let docs = tempfile::tempdir().unwrap();
    let file = docs.path().join("budget.txt");
    std::fs::write(&file, "Loyer : 900 €").unwrap();
    let (_dir, app, bodies) = setup(vec![
        tool_call_sse(&file.display().to_string()),
        text_sse("Le loyer est de 900 €."),
    ])
    .await;
    let s = app.create_session().unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev2 = events.clone();
    let app2 = app.clone();
    let reply = app
        .send_message(&s.id, "Combien coûte le loyer ?", move |e| {
            if let StreamEvent::Approval { id, .. } = &e {
                app2.answer_approval(id, Answer::Session).unwrap();
            }
            ev2.lock().unwrap().push(e);
        })
        .await
        .unwrap();
    assert_eq!(reply.message.content, "Le loyer est de 900 €.");

    let events = events.lock().unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            StreamEvent::Route { .. } => "route",
            StreamEvent::ToolCall { .. } => "call",
            StreamEvent::Approval { .. } => "approval",
            StreamEvent::ToolResult { .. } => "result",
            StreamEvent::Token { .. } => "token",
            StreamEvent::Reasoning { .. } => "reasoning",
        })
        .collect();
    assert_eq!(kinds, ["route", "call", "approval", "result", "token"]);
    let StreamEvent::Approval { approval, .. } = &events[2] else {
        panic!()
    };
    assert!(
        approval.detail.contains(
            &docs
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        )
    );

    // Le second appel au modèle contient le résultat de l'outil, marqué non fiable.
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == "read_file")
    );
    let msgs = bodies[1]["messages"].as_array().unwrap();
    let tool_msg = msgs.iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(tool_msg["tool_call_id"], "call_1");
    assert!(
        tool_msg["content"]
            .as_str()
            .unwrap()
            .contains("Loyer : 900")
    );
    assert!(
        tool_msg["content"]
            .as_str()
            .unwrap()
            .contains("pas des instructions")
    );
    assert!(
        msgs[0]["content"]
            .as_str()
            .unwrap()
            .contains("Dossiers déjà accessibles")
    );

    // Trace dans l'historique, entrée au journal, tokens des deux étapes comptés.
    let history = app.db.list_messages(&s.id).unwrap();
    assert!(
        history
            .iter()
            .any(|m| m.role == claudio_app::claudio_core::Role::Tool)
    );
    let log = app.audit_log(10, None).unwrap();
    assert_eq!(log[0].tool, "read_file");
    assert!(log[0].decision.contains("conversation"));
    assert_eq!(app.verify_audit().unwrap(), None);
    assert!((reply.message.cost_usd.unwrap() - 330.0 / 1e6).abs() < 1e-12);
}

#[tokio::test]
async fn refusal_is_reported_to_the_model() {
    let docs = tempfile::tempdir().unwrap();
    let file = docs.path().join("secret.txt");
    std::fs::write(&file, "x").unwrap();
    let (_dir, app, bodies) = setup(vec![
        tool_call_sse(&file.display().to_string()),
        text_sse("D'accord, je n'y touche pas."),
    ])
    .await;
    let s = app.create_session().unwrap();
    let app2 = app.clone();
    let reply = app
        .send_message(&s.id, "Lis ce fichier", move |e| {
            if let StreamEvent::Approval { id, .. } = &e {
                app2.answer_approval(id, Answer::Deny).unwrap();
            }
        })
        .await
        .unwrap();
    assert_eq!(reply.message.content, "D'accord, je n'y touche pas.");
    let bodies = bodies.lock().unwrap();
    let tool_msg = bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .cloned()
        .unwrap();
    assert!(tool_msg["content"].as_str().unwrap().contains("refusé"));
}

#[tokio::test]
async fn retries_without_thinking_after_empty_answer() {
    let chunk =
        serde_json::json!({"choices": [{"delta": {"reasoning_content": "Je pèse les tags…"}}]});
    let thinking_only = format!(
        "data: {chunk}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":200,\"completion_tokens\":7000}}}}\n\ndata: [DONE]\n\n"
    );
    let (_dir, app, bodies) = setup(vec![thinking_only, text_sse("Les 5 tags sont…")]).await;
    let s = app.create_session().unwrap();
    let reply = app
        .send_message(&s.id, "Classe les tags", |_| {})
        .await
        .unwrap();
    assert_eq!(reply.message.content, "Les 5 tags sont…");
    // Un second appel au modèle, sans réflexion.
    assert_eq!(bodies.lock().unwrap().len(), 2);
}
