//! Routage de bout en bout contre un faux fournisseur compatible OpenAI :
//! choix de la cible, repli quand le local échoue, coût et budget.

use claudio_app::claudio_core::AppPaths;
use claudio_app::claudio_core::config::{
    ActiveModel, Budget, CloudModel, ProviderKind, RoutingMode,
};
use claudio_app::router::Target;
use claudio_app::{Claudio, StreamEvent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Faux serveur : répond `n` fois avec la même réponse en flux.
async fn fake_provider(n: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move {
        for _ in 0..n {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let mut got = 0;
            loop {
                got += sock.read(&mut buf[got..]).await.unwrap();
                let text = String::from_utf8_lossy(&buf[..got]);
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
                        break;
                    }
                }
            }
            let sse = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Réponse du cloud\"}}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000000,\"completion_tokens\":100000}}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                sse.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        }
    });
    base
}

fn setup(dir: &std::path::Path, base: &str) -> std::sync::Arc<Claudio> {
    let app = Claudio::open(AppPaths::in_dir(dir.to_path_buf())).unwrap();
    let p = app
        .add_provider(ProviderKind::Custom, "Faux", Some(base), None)
        .unwrap();
    app.set_cloud_model(
        Some(CloudModel {
            provider_id: p.config.id.clone(),
            model: "faux-modele".into(),
        }),
        Some((4.0, 20.0)),
    )
    .unwrap();
    app
}

#[tokio::test]
async fn cloud_mode_streams_and_records_cost() {
    let dir = tempfile::tempdir().unwrap();
    let app = setup(dir.path(), &fake_provider(1).await);
    app.set_routing_mode(RoutingMode::Cloud).unwrap();
    let s = app.create_session().unwrap();
    let mut events = Vec::new();
    let reply = app
        .send_message(&s.id, "Bonjour", |e| events.push(e))
        .await
        .unwrap();
    assert!(matches!(
        &events[0],
        StreamEvent::Route {
            target: Target::Cloud,
            ..
        }
    ));
    assert_eq!(reply.message.content, "Réponse du cloud");
    // 1 M jetons en entrée à 4 $ + 100 k en sortie à 20 $ = 6 $.
    assert!((reply.message.cost_usd.unwrap() - 6.0).abs() < 1e-9);
    let report = app.usage_report().unwrap();
    assert!((report.budget.today_usd - 6.0).abs() < 1e-9);
}

#[tokio::test]
async fn auto_mode_falls_back_to_cloud_when_local_fails() {
    let dir = tempfile::tempdir().unwrap();
    let app = setup(dir.path(), &fake_provider(1).await);
    // Modèle local sélectionné mais absent : le local échoue avant de répondre.
    app.set_active_model(ActiveModel::Local {
        id: "absent:Q4".into(),
    })
    .unwrap();
    let s = app.create_session().unwrap();
    let mut routes = Vec::new();
    let reply = app
        .send_message(&s.id, "Bonjour", |e| {
            if let StreamEvent::Route { target, reason, .. } = e {
                routes.push((target, reason));
            }
        })
        .await
        .unwrap();
    assert_eq!(routes.len(), 1, "le local n'a même pas démarré");
    assert_eq!(routes[0].0, Target::Cloud);
    assert!(routes[0].1.starts_with("repli cloud"));
    assert_eq!(reply.message.content, "Réponse du cloud");
}

#[tokio::test]
async fn budget_blocks_the_cloud() {
    let dir = tempfile::tempdir().unwrap();
    let app = setup(dir.path(), &fake_provider(1).await);
    app.set_routing_mode(RoutingMode::Cloud).unwrap();
    let s = app.create_session().unwrap();
    app.send_message(&s.id, "Bonjour", |_| {}).await.unwrap();
    app.set_budget(Budget {
        daily_usd: Some(5.0),
        monthly_usd: None,
    })
    .unwrap();
    assert!(app.budget_status().unwrap().exceeded);
    // Ni cloud (budget) ni local (aucun modèle) : refus explicite.
    let err = app.send_message(&s.id, "Encore", |_| {}).await.unwrap_err();
    assert!(format!("{err:#}").contains("budget"));
}

#[tokio::test]
async fn local_mode_never_calls_the_cloud() {
    let dir = tempfile::tempdir().unwrap();
    let app = setup(dir.path(), &fake_provider(0).await);
    app.set_routing_mode(RoutingMode::Local).unwrap();
    let s = app.create_session().unwrap();
    let err = app
        .send_message(&s.id, "Bonjour", |_| {})
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("mode local"));
}
