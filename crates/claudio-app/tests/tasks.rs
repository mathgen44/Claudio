//! Tâches de fond de bout en bout contre un faux modèle : planification en JSON,
//! étapes en parallèle, synthèse, échec partiel, autorisation, reprise.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use claudio_app::claudio_core::AppPaths;
use claudio_app::claudio_core::config::{CloudModel, ProviderKind, RoutingMode};
use claudio_app::claudio_core::tasks::{StepStatus, TaskStatus};
use claudio_app::claudio_perms::Answer;
use claudio_app::{AppEvent, Claudio};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Responder = Arc<dyn Fn(&Value) -> Option<String> + Send + Sync>;

struct Fake {
    base: String,
    max_concurrent: Arc<AtomicUsize>,
}

/// Faux modèle : chaque connexion est servie en parallèle, avec un délai, par `respond`
/// (`None` = erreur 500).
async fn fake_model(respond: Responder) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let current = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let (cur, max) = (current.clone(), max_concurrent.clone());
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let (respond, cur, max) = (respond.clone(), cur.clone(), max.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1 << 20];
                let mut got = 0;
                let body = loop {
                    let n = sock.read(&mut buf[got..]).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    got += n;
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
                            break serde_json::from_slice::<Value>(&buf[i + 4..i + 4 + len])
                                .unwrap();
                        }
                    }
                };
                let now = cur.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(300)).await;
                cur.fetch_sub(1, Ordering::SeqCst);
                let resp = match respond(&body) {
                    Some(sse) => format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                        sse.len()
                    ),
                    None => "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 4\r\nconnection: close\r\n\r\npanne".into(),
                };
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    Fake {
        base,
        max_concurrent,
    }
}

fn text_sse(text: &str) -> String {
    let chunk = serde_json::json!({"choices": [{"delta": {"content": text}}]});
    format!(
        "data: {chunk}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":50,\"completion_tokens\":10}}}}\n\ndata: [DONE]\n\n"
    )
}

fn user_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].as_str().unwrap_or("").to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn setup(
    respond: Responder,
) -> (
    tempfile::TempDir,
    Arc<Claudio>,
    Fake,
    Arc<Mutex<Vec<AppEvent>>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_model(respond).await;
    let app = Claudio::open(AppPaths::in_dir(dir.path().join("claudio"))).unwrap();
    let p = app
        .add_provider(ProviderKind::Custom, "Faux", Some(&fake.base), None)
        .unwrap();
    app.set_cloud_model(
        Some(CloudModel {
            provider_id: p.config.id,
            model: "faux".into(),
        }),
        None,
    )
    .unwrap();
    app.set_routing_mode(RoutingMode::Cloud).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let app2 = app.clone();
    app.set_event_sink(Arc::new(move |e: AppEvent| {
        // L'utilisateur autorise une fois toute demande des tâches.
        if let AppEvent::Approval { id, .. } = &e {
            app2.answer_approval(id, Answer::Once).unwrap();
        }
        ev.lock().unwrap().push(e);
    }));
    (dir, app, fake, events)
}

async fn wait_finished(app: &Claudio, id: &str) -> TaskStatus {
    for _ in 0..200 {
        let d = app.task_detail(id).unwrap();
        if !d.task.status.is_active() && app.list_tasks(50).unwrap().iter().any(|t| t.id == id) {
            // Laisse la tâche de fond terminer son bilan.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let d = app.task_detail(id).unwrap();
            if !d.task.status.is_active() {
                return d.task.status;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("la tâche ne s'est pas terminée");
}

const PLAN: &str = r#"{"steps": [
  {"id": "e1", "agent": "researcher", "title": "Chercher les prix", "instructions": "Trouve les prix.", "depends_on": []},
  {"id": "e2", "agent": "analyst", "title": "Lire le budget", "instructions": "Analyse le budget.", "depends_on": []}
]}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planned_task_runs_steps_in_parallel_then_synthesis() {
    let respond: Responder = Arc::new(|body| {
        let u = user_text(body);
        Some(if user_text(body).contains("Agents disponibles") {
            text_sse(PLAN)
        } else if u.contains("Vérification et synthèse") {
            assert!(
                u.contains("prix : 42 €") && u.contains("budget : 100 €"),
                "résultats transmis"
            );
            text_sse("Synthèse : tout est cohérent.")
        } else if u.contains("Chercher les prix") {
            text_sse("prix : 42 €")
        } else {
            text_sse("budget : 100 €")
        })
    });
    let (_dir, app, fake, events) = setup(respond).await;
    let task = app
        .start_task("Comparer prix et budget", "plan", None)
        .unwrap();
    assert_eq!(wait_finished(&app, &task.id).await, TaskStatus::Done);

    let d = app.task_detail(&task.id).unwrap();
    assert_eq!(d.steps.len(), 3);
    assert!(d.steps.iter().all(|s| s.status == StepStatus::Done));
    assert_eq!(
        d.task.result.as_deref(),
        Some("Synthèse : tout est cohérent.")
    );
    assert_eq!(d.levels.len(), 2, "deux étapes parallèles puis la synthèse");
    assert!(d.steps[0].log.contains("cloud"));
    assert!(d.task.tokens > 0);
    assert!(
        fake.max_concurrent.load(Ordering::SeqCst) >= 2,
        "les étapes indépendantes tournent en même temps"
    );
    assert!(events.lock().unwrap().iter().any(|e| matches!(
        e,
        AppEvent::TaskFinished {
            status: TaskStatus::Done,
            ..
        }
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_step_gives_partial_result() {
    let respond: Responder = Arc::new(|body| {
        let u = user_text(body);
        if user_text(body).contains("Agents disponibles") {
            Some(text_sse(PLAN))
        } else if u.contains("Vérification et synthèse") {
            assert!(
                !u.contains("Lire le budget"),
                "seule l'étape réussie est transmise"
            );
            Some(text_sse("Synthèse partielle."))
        } else if u.contains("Lire le budget") {
            None
        } else {
            Some(text_sse("prix : 42 €"))
        }
    });
    let (_dir, app, _fake, _) = setup(respond).await;
    let task = app.start_task("Comparer", "plan", None).unwrap();
    assert_eq!(wait_finished(&app, &task.id).await, TaskStatus::Partial);
    let d = app.task_detail(&task.id).unwrap();
    let budget = d.steps.iter().find(|s| s.step_id == "e2").unwrap();
    assert_eq!(budget.status, StepStatus::Failed);
    assert!(budget.error.as_deref().unwrap().contains("500"));
    assert_eq!(d.task.result.as_deref(), Some("Synthèse partielle."));

    // Reprise : seule l'étape échouée (et la synthèse) repart.
    assert!(app.resume_task(&task.id).is_ok());
    let status = wait_finished(&app, &task.id).await;
    assert_eq!(
        status,
        TaskStatus::Partial,
        "le faux modèle échoue toujours sur cette étape"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_agent_task_with_tool_approval() {
    let docs = tempfile::tempdir().unwrap();
    let file = docs.path().join("note.txt");
    std::fs::write(&file, "code secret : 1234").unwrap();
    let path = file.display().to_string();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let respond: Responder = Arc::new(move |body| {
        let tools = body["tools"].as_array().cloned().unwrap_or_default();
        // Le documentaliste n'a pas PowerShell.
        assert!(
            !tools
                .iter()
                .any(|t| t["function"]["name"] == "run_powershell")
        );
        Some(if c.fetch_add(1, Ordering::SeqCst) == 0 {
            let args = serde_json::to_string(&serde_json::json!({"path": path})).unwrap();
            let chunk = serde_json::json!({"choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "c1", "function": {"name": "read_file", "arguments": args}}]}}]});
            format!("data: {chunk}\n\ndata: [DONE]\n\n")
        } else {
            assert!(
                body.to_string().contains("1234"),
                "résultat de l'outil renvoyé"
            );
            text_sse("Le code est 1234.")
        })
    });
    let (_dir, app, _fake, events) = setup(respond).await;
    let task = app
        .start_task("Lis la note", "agent:researcher", None)
        .unwrap();
    assert_eq!(wait_finished(&app, &task.id).await, TaskStatus::Done);
    let d = app.task_detail(&task.id).unwrap();
    assert_eq!(d.steps.len(), 1, "un seul agent : pas de synthèse");
    assert_eq!(d.task.result.as_deref(), Some("Le code est 1234."));
    assert!(d.steps[0].log.contains("read_file"));
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, AppEvent::Approval { .. }))
    );
    assert!(app.start_task("x", "agent:inconnu", None).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn schedules_start_tasks_when_due() {
    let respond: Responder = Arc::new(|_| Some(text_sse("Rapport du jour.")));
    let (_dir, app, _fake, _) = setup(respond).await;
    assert!(
        app.add_schedule("", "Rapport", "agent:assistant", "toutes les heures")
            .is_err()
    );
    let s = app
        .add_schedule(
            "",
            "Fais le rapport du jour",
            "agent:assistant",
            "* * * * *",
        )
        .unwrap();
    assert!(app.schedules().unwrap()[0].next_run.is_some());
    assert!(
        app.tick_schedules(chrono::Utc::now()).unwrap().is_empty(),
        "pas encore l'heure"
    );
    let started = app
        .tick_schedules(chrono::Utc::now() + chrono::Duration::minutes(2))
        .unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(wait_finished(&app, &started[0]).await, TaskStatus::Done);
    assert_eq!(
        app.task_detail(&started[0]).unwrap().task.schedule_id,
        Some(s.id)
    );
    app.set_schedule_enabled(s.id, false).unwrap();
    assert!(app.schedules().unwrap()[0].next_run.is_none());
}
