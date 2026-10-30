//! Outils de bout en bout : permissions, demandes à l'utilisateur, journal,
//! bac à sable, serveur MCP.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use claudio_core::Database;
use claudio_core::config::{McpServerConfig, McpTransport, Permissions, Policy};
use claudio_perms::{Answer, Approval, Engine, Protected};
use claudio_tools::mcp::McpManager;
use claudio_tools::{Approver, BoxFuture, Context, Toolbox};

/// Répond aux demandes selon une liste préparée et garde les questions posées.
#[derive(Default)]
struct Scripted {
    answers: Mutex<VecDeque<Answer>>,
    asked: Mutex<Vec<Approval>>,
}

impl Approver for Scripted {
    fn ask<'a>(&'a self, _tool: &'a str, approval: &'a Approval) -> BoxFuture<'a, Answer> {
        self.asked.lock().unwrap().push(approval.clone());
        let a = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Answer::Deny);
        Box::pin(async move { a })
    }
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db: Database,
    engine: Engine,
    http: reqwest::Client,
    approver: Scripted,
    toolbox: Toolbox,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce_canon(dir.path());
        for d in ["ws", "docs", "backups"] {
            std::fs::create_dir(root.join(d)).unwrap();
        }
        std::fs::write(
            root.join("docs").join("notes.txt"),
            "ligne 1\nBonjour Claudio\nligne 3\n",
        )
        .unwrap();
        claudio_perms::sandbox::label_low_integrity(&root.join("ws")).unwrap();
        let db = Database::open_in_memory().unwrap();
        let engine = Engine::new(
            db.clone(),
            Permissions::default(),
            Protected::default(),
            root.join("ws"),
        );
        Self {
            _dir: dir,
            root,
            db,
            engine,
            http: reqwest::Client::new(),
            approver: Scripted::default(),
            toolbox: Toolbox::new(Arc::new(McpManager::default())),
        }
    }

    fn answer(&self, a: &[Answer]) {
        self.approver
            .answers
            .lock()
            .unwrap()
            .extend(a.iter().copied());
    }

    fn asked(&self) -> usize {
        self.approver.asked.lock().unwrap().len()
    }

    async fn call(&self, name: &str, args: serde_json::Value) -> claudio_tools::ToolOutput {
        let ctx = Context {
            engine: &self.engine,
            db: &self.db,
            http: &self.http,
            approver: &self.approver,
            session: "s1",
            backups: self.root.join("backups"),
        };
        self.toolbox.call(&ctx, name, &args.to_string()).await
    }
}

fn dunce_canon(p: &Path) -> PathBuf {
    let s = std::fs::canonicalize(p).unwrap().display().to_string();
    PathBuf::from(s.trim_start_matches(r"\\?\"))
}

#[tokio::test]
async fn reading_requires_approval_and_is_audited() {
    let env = Env::new();
    let notes = env.root.join("docs").join("notes.txt");
    let args = serde_json::json!({"path": notes});

    env.answer(&[Answer::Deny]);
    let out = env.call("read_file", args.clone()).await;
    assert!(
        out.is_error && out.content.contains("refusé"),
        "{}",
        out.content
    );

    env.answer(&[Answer::Session]);
    let out = env.call("read_file", args.clone()).await;
    assert!(
        !out.is_error && out.content.contains("Bonjour Claudio"),
        "{}",
        out.content
    );
    assert!(
        out.content.starts_with("[Contenu externe"),
        "contenu marqué non fiable"
    );

    // Retenu pour la conversation : plus de question.
    let before = env.asked();
    let out = env
        .call(
            "search_files",
            serde_json::json!({"path": env.root.join("docs"), "text": "bonjour"}),
        )
        .await;
    assert!(out.content.contains("notes.txt:2"), "{}", out.content);
    assert_eq!(env.asked(), before);

    let log = env.db.audit_entries(10, None).unwrap();
    assert_eq!(log.len(), 3);
    assert!(log[2].decision.contains("refusé"));
    assert!(log[1].decision.contains("conversation"));
    assert_eq!(env.db.verify_audit().unwrap(), None);
}

#[tokio::test]
async fn writing_backups_and_workspace() {
    let env = Env::new();
    // L'espace de travail est libre d'accès.
    let out = env
        .call(
            "write_file",
            serde_json::json!({"path": env.root.join("ws/a.md"), "content": "# A"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(env.asked(), 0);

    // Remplacer un fichier ailleurs : question « dangereuse » et sauvegarde.
    let notes = env.root.join("docs").join("notes.txt");
    env.answer(&[Answer::Once]);
    let out = env
        .call(
            "write_file",
            serde_json::json!({"path": notes, "content": "nouveau", "mode": "overwrite"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(env.approver.asked.lock().unwrap()[0].dangerous);
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "nouveau");
    let backup_dir = std::fs::read_dir(env.root.join("backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(
        std::fs::read_to_string(backup_dir.join("notes.txt"))
            .unwrap()
            .contains("Bonjour")
    );

    // Créer un fichier qui existe déjà sans le dire : refusé sans rien demander.
    let out = env
        .call(
            "write_file",
            serde_json::json!({"path": notes, "content": "x"}),
        )
        .await;
    assert!(out.is_error && out.content.contains("existe déjà"));

    // Chemin relatif : refusé.
    let out = env
        .call("read_file", serde_json::json!({"path": "notes.txt"}))
        .await;
    assert!(out.is_error);
}

#[tokio::test]
async fn powershell_runs_in_sandbox_after_confirmation() {
    let env = Env::new();
    env.answer(&[Answer::Once]);
    let out = env
        .call(
            "run_powershell",
            serde_json::json!({"command": "Write-Output (6*7)"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("42"), "{}", out.content);
    let q = env.approver.asked.lock().unwrap()[0].clone();
    assert!(q.title.contains("bac à sable") && q.detail.contains("Write-Output"));

    // Politique « refuser » : aucune question, refus direct.
    let mut p = env.engine.policy();
    p.shell = Policy::Deny;
    env.engine.set_policy(p);
    let out = env
        .call("run_powershell", serde_json::json!({"command": "dir"}))
        .await;
    assert!(out.is_error && out.content.contains("désactivées"));
}

#[tokio::test]
async fn web_refuses_local_network() {
    let env = Env::new();
    let mut p = env.engine.policy();
    p.network = Policy::Allow;
    env.engine.set_policy(p);
    let out = env
        .call(
            "web_fetch",
            serde_json::json!({"url": "http://127.0.0.1:9/admin"}),
        )
        .await;
    assert!(
        out.is_error && out.content.contains("réseau local"),
        "{}",
        out.content
    );
    let out = env
        .call(
            "web_fetch",
            serde_json::json!({"url": "file:///C:/Windows/win.ini"}),
        )
        .await;
    assert!(out.is_error);
}

#[tokio::test]
async fn mcp_server_tools() {
    let env = Env::new();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_mcp.py");
    let cfg = McpServerConfig {
        id: "faux-1".into(),
        name: "Faux".into(),
        transport: McpTransport::Stdio {
            command: "python".into(),
            args: vec!["-I".into(), script.display().to_string()],
            env: vec![],
        },
        enabled: true,
        secrets: vec![],
    };
    let tools = env
        .toolbox
        .mcp
        .test(&cfg)
        .await
        .expect("connexion au faux serveur");
    assert_eq!(tools.len(), 4);
    env.toolbox.mcp.configure(vec![cfg]).await;
    let specs = env.toolbox.specs().await;
    assert!(specs.iter().any(|s| s.name == "mcp__faux-1__echo"));

    env.answer(&[Answer::Session]);
    let out = env
        .call("mcp__faux-1__echo", serde_json::json!({"text": "salut"}))
        .await;
    assert!(
        !out.is_error && out.content.contains("écho : salut"),
        "{}",
        out.content
    );
    let out = env
        .call("mcp__faux-1__echo", serde_json::json!({"text": "encore"}))
        .await;
    assert!(out.content.contains("encore"));
    assert_eq!(env.asked(), 1, "retenu pour la conversation");

    env.answer(&[Answer::Once]);
    let out = env.call("mcp__faux-1__fail", serde_json::json!({})).await;
    assert!(out.is_error);
}

#[tokio::test]
async fn mcp_secrets_reach_the_server() {
    let env = Env::new();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_mcp.py");
    let cfg = McpServerConfig {
        id: format!("test{}", std::process::id()),
        name: "Faux à clé".into(),
        transport: McpTransport::Stdio {
            command: "python".into(),
            args: vec!["-I".into(), script.display().to_string()],
            env: vec![],
        },
        enabled: true,
        secrets: vec!["FAKE_TOKEN".into()],
    };
    let secret = cfg.secret_name("FAKE_TOKEN");
    // Sans valeur enregistrée : erreur claire.
    claudio_core::secrets::delete(&secret).unwrap();
    let err = env.toolbox.mcp.test(&cfg).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("saisissez-la de nouveau"),
        "{err:#}"
    );

    claudio_core::secrets::set(&secret, "jeton-très-secret").unwrap();
    env.toolbox.mcp.test(&cfg).await.unwrap();
    env.toolbox.mcp.configure(vec![cfg.clone()]).await;
    env.answer(&[Answer::Once]);
    let tool = format!("mcp__{}__token", cfg.id);
    let out = env.call(&tool, serde_json::json!({})).await;
    claudio_core::secrets::delete(&secret).unwrap();
    assert!(
        out.content.contains("jeton : jeton-très-secret"),
        "{}",
        out.content
    );
}

/// Fermer la connexion tue aussi les petits-enfants (ici `cmd /c python …`).
#[cfg(windows)]
#[tokio::test]
async fn mcp_disconnect_kills_process_tree() {
    use claudio_tools::mcp::Client;

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_mcp.py");
    let cfg = McpServerConfig {
        id: "arbre".into(),
        name: "Arbre".into(),
        transport: McpTransport::Stdio {
            command: "cmd".into(),
            args: vec![
                "/c".into(),
                "python".into(),
                "-I".into(),
                script.display().to_string(),
            ],
            env: vec![],
        },
        enabled: true,
        secrets: vec![],
    };
    let alive = |pid: &str| {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(pid)
    };
    let client = Client::connect(&cfg).await.unwrap();
    let pid = client
        .call_tool("pid", &serde_json::json!({}))
        .await
        .unwrap()
        .content;
    assert!(alive(&pid), "le serveur {pid} devrait tourner");
    drop(client);
    for _ in 0..20 {
        if !alive(&pid) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("le processus {pid} a survécu à la déconnexion");
}
