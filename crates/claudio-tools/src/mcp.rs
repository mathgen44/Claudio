//! Client MCP (Model Context Protocol) : relie Claudio à des serveurs d'outils
//! externes, lancés localement (stdio) ou distants (« Streamable HTTP »).
//!
//! Seul ce dont Claudio a besoin est implémenté : initialisation, liste des outils
//! et appel d'outil. Chaque appel passe par le moteur de permissions.

use std::collections::HashMap;
use std::process::Stdio as Pipe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use claudio_core::config::{McpServerConfig, McpTransport};
use claudio_perms::Action;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

use crate::{Context, Done, ToolOutput, ToolSpec, untrusted};

const PROTOCOL_VERSION: &str = "2025-06-18";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const PREFIX: &str = "mcp__";

#[derive(Debug, Clone, Serialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// Transport stdio : JSON-RPC, un message par ligne.
struct Stdio {
    _child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
}

/// Transport HTTP : une requête POST par message, réponse en JSON ou en flux SSE.
struct Http {
    http: reqwest::Client,
    url: String,
    session: std::sync::Mutex<Option<String>>,
    protocol: std::sync::Mutex<Option<String>>,
    /// En-têtes secrets (authentification), jamais journalisés.
    headers: Vec<(String, String)>,
}

enum Transport {
    Stdio(Stdio),
    Http(Http),
}

pub struct Client {
    transport: Transport,
    next_id: AtomicU64,
}

/// Trouve l'exécutable (Windows : `npx` → `npx.cmd`, lancé via `cmd /c`).
fn command_for(program: &str, args: &[String]) -> Command {
    let path = std::path::Path::new(program);
    let resolved = if path.extension().is_some() || path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                ["exe", "cmd", "bat"]
                    .iter()
                    .map(|ext| dir.join(format!("{program}.{ext}")))
                    .find(|c| c.is_file())
            })
        })
    };
    let exe = resolved.unwrap_or_else(|| path.to_path_buf());
    let is_script = exe
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    let mut cmd = if is_script {
        let mut c = Command::new("cmd.exe");
        c.arg("/d").arg("/c").arg(&exe);
        c
    } else {
        Command::new(&exe)
    };
    cmd.args(args);
    cmd
}

fn rpc_error(v: &Value) -> anyhow::Error {
    anyhow!(
        "erreur du serveur MCP : {}",
        v.get("message")
            .and_then(Value::as_str)
            .unwrap_or("inconnue")
    )
}

impl Client {
    pub async fn connect(cfg: &McpServerConfig) -> Result<Self> {
        let secrets = cfg
            .secrets
            .iter()
            .map(|key| {
                let value =
                    claudio_core::secrets::get(&cfg.secret_name(key))?.with_context(|| {
                        format!("la clé « {key} » n'est plus enregistrée : saisissez-la de nouveau")
                    })?;
                Ok((key.clone(), value))
            })
            .collect::<Result<Vec<_>>>()?;
        let transport = match &cfg.transport {
            McpTransport::Stdio { command, args, env } => {
                let mut cmd = command_for(command, args);
                cmd.envs(env.iter().cloned())
                    .envs(secrets)
                    .stdin(Pipe::piped())
                    .stdout(Pipe::piped())
                    .stderr(Pipe::piped())
                    .kill_on_drop(true);
                #[cfg(windows)]
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                let mut child = cmd
                    .spawn()
                    .with_context(|| format!("lancement de « {command} »"))?;
                let stdin = child.stdin.take().context("entrée du serveur MCP")?;
                let stdout = child.stdout.take().context("sortie du serveur MCP")?;
                let stderr = child.stderr.take().context("erreurs du serveur MCP")?;
                let pending: Pending = Arc::default();
                let name = cfg.name.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        tracing::debug!(target: "mcp", server = %name, "{l}");
                    }
                });
                let stdin = Arc::new(Mutex::new(stdin));
                spawn_reader(stdout, pending.clone(), stdin.clone());
                Transport::Stdio(Stdio {
                    _child: child,
                    stdin,
                    pending,
                })
            }
            McpTransport::Http { url } => Transport::Http(Http {
                http: reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(15))
                    .build()?,
                url: url.clone(),
                session: Default::default(),
                protocol: Default::default(),
                headers: secrets,
            }),
        };
        let client = Self {
            transport,
            next_id: AtomicU64::new(1),
        };
        let init = tokio::time::timeout(
            CONNECT_TIMEOUT,
            client.request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "Claudio", "version": env!("CARGO_PKG_VERSION")}
                }),
            ),
        )
        .await
        .context("le serveur MCP ne répond pas")??;
        if let Transport::Http(h) = &client.transport {
            *h.protocol.lock().unwrap() = init
                .get("protocolVersion")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        client
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match &self.transport {
            Transport::Stdio(s) => s.send(&msg).await,
            Transport::Http(h) => h.post(&msg, None).await.map(|_| ()),
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let fut = async {
            match &self.transport {
                Transport::Stdio(s) => {
                    let (tx, rx) = oneshot::channel();
                    s.pending.lock().unwrap().insert(id, tx);
                    s.send(&msg).await?;
                    rx.await.context("serveur MCP arrêté")?
                }
                Transport::Http(h) => h.post(&msg, Some(id)).await?.context("réponse MCP vide"),
            }
        };
        tokio::time::timeout(REQUEST_TIMEOUT, fut)
            .await
            .with_context(|| format!("délai dépassé pour {method}"))?
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = self.request("tools/list", params).await?;
            for t in r["tools"].as_array().into_iter().flatten() {
                let Some(name) = t["name"].as_str() else {
                    continue;
                };
                out.push(McpTool {
                    name: name.to_owned(),
                    description: t["description"].as_str().unwrap_or("").to_owned(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                });
            }
            cursor = r["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() || out.len() > 500 {
                break;
            }
        }
        Ok(out)
    }

    pub async fn call_tool(&self, name: &str, arguments: &Value) -> Result<ToolOutput> {
        let r = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        let mut parts = Vec::new();
        for c in r["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("text") => parts.push(c["text"].as_str().unwrap_or("").to_owned()),
                Some("resource") => parts.push(
                    c.pointer("/resource/text")
                        .and_then(Value::as_str)
                        .unwrap_or("[ressource]")
                        .to_owned(),
                ),
                Some(other) => parts.push(format!("[contenu {other} non affiché]")),
                None => {}
            }
        }
        if parts.is_empty()
            && let Some(s) = r.get("structuredContent")
        {
            parts.push(s.to_string());
        }
        let text = parts.join("\n");
        Ok(if r["isError"].as_bool().unwrap_or(false) {
            ToolOutput::error(text)
        } else {
            ToolOutput::ok(text)
        })
    }
}

/// Lit les messages du serveur ; répond à ses éventuelles requêtes (ping…).
fn spawn_reader(
    stdout: tokio::process::ChildStdout,
    pending: Pending,
    stdin: Arc<Mutex<ChildStdin>>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                tracing::debug!(target: "mcp", "ligne ignorée : {line}");
                continue;
            };
            handle_incoming(msg, &pending, &tx);
        }
        // Fin du processus : toutes les requêtes en attente échouent.
        for (_, s) in pending.lock().unwrap().drain() {
            let _ = s.send(Err(anyhow!("le serveur MCP s'est arrêté")));
        }
    });
    tokio::spawn(async move {
        while let Some(reply) = rx.recv().await {
            let mut w = stdin.lock().await;
            let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
            let _ = w.flush().await;
        }
    });
}

fn handle_incoming(
    msg: Value,
    pending: &Pending,
    replies: &tokio::sync::mpsc::UnboundedSender<Value>,
) {
    let id = msg.get("id").cloned();
    if let Some(method) = msg.get("method").and_then(Value::as_str) {
        // Requête du serveur : on répond au minimum ; une notification est ignorée.
        if let Some(id) = id {
            let reply = match method {
                "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
                "roots/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"roots": []}}),
                _ => json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": "non pris en charge par Claudio"}}),
            };
            let _ = replies.send(reply);
        }
        return;
    }
    let Some(id) = id.as_ref().and_then(Value::as_u64) else {
        return;
    };
    if let Some(s) = pending.lock().unwrap().remove(&id) {
        let res = match msg.get("error") {
            Some(e) => Err(rpc_error(e)),
            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = s.send(res);
    }
}

impl Stdio {
    async fn send(&self, msg: &Value) -> Result<()> {
        let mut w = self.stdin.lock().await;
        w.write_all(format!("{msg}\n").as_bytes()).await?;
        w.flush().await?;
        Ok(())
    }
}

impl Http {
    /// Envoie un message ; pour une requête, renvoie la réponse correspondant à `id`.
    async fn post(&self, msg: &Value, id: Option<u64>) -> Result<Option<Value>> {
        let mut req = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .json(msg);
        if let Some(s) = self.session.lock().unwrap().clone() {
            req = req.header("Mcp-Session-Id", s);
        }
        if let Some(p) = self.protocol.lock().unwrap().clone() {
            req = req.header("MCP-Protocol-Version", p);
        }
        for (name, value) in &self.headers {
            let mut value = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
                anyhow!("la clé « {name} » contient des caractères interdits dans un en-tête")
            })?;
            value.set_sensitive(true);
            req = req.header(name.as_str(), value);
        }
        let resp = req.send().await.context("serveur MCP injoignable")?;
        if let Some(s) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session.lock().unwrap() = Some(s.to_owned());
        }
        if !resp.status().is_success() {
            bail!("le serveur MCP a répondu {}", resp.status());
        }
        let Some(id) = id else { return Ok(None) };
        let is_sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.contains("text/event-stream"));
        let take = |v: Value| -> Option<Result<Value>> {
            (v.get("id").and_then(Value::as_u64) == Some(id)).then(|| match v.get("error") {
                Some(e) => Err(rpc_error(e)),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            })
        };
        if !is_sse {
            let v: Value = resp.json().await?;
            return take(v).transpose().map(|o| o.or(Some(Value::Null)));
        }
        let mut buf = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                if let Some(data) = line.trim_end().strip_prefix("data:")
                    && let Ok(v) = serde_json::from_str::<Value>(data.trim())
                    && let Some(r) = take(v)
                {
                    return r.map(Some);
                }
            }
        }
        bail!("le flux MCP s'est terminé sans réponse")
    }
}

/// Nom exposé au modèle : `mcp__<serveur>__<outil>`, limité aux caractères admis.
fn exposed_name(server_id: &str, tool: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let mut n = format!("{PREFIX}{}__{}", clean(server_id), clean(tool));
    n.truncate(64);
    n
}

pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with(PREFIX)
}

struct Connected {
    config: McpServerConfig,
    client: Arc<Client>,
    tools: Vec<McpTool>,
}

/// Serveurs MCP configurés ; connexion à la demande, gardée ouverte.
#[derive(Default)]
pub struct McpManager {
    servers: Mutex<Vec<McpServerConfig>>,
    connected: Mutex<HashMap<String, Connected>>,
}

impl McpManager {
    /// Met à jour la configuration ; les serveurs modifiés ou retirés sont déconnectés.
    pub async fn configure(&self, servers: Vec<McpServerConfig>) {
        let mut conn = self.connected.lock().await;
        conn.retain(|id, c| {
            servers.iter().any(|s| {
                s.id == *id
                    && s.enabled
                    && s.transport == c.config.transport
                    && s.name == c.config.name
                    && s.secrets == c.config.secrets
            })
        });
        *self.servers.lock().await = servers;
    }

    async fn ensure(&self, cfg: &McpServerConfig) -> Result<()> {
        if self.connected.lock().await.contains_key(&cfg.id) {
            return Ok(());
        }
        let client = Arc::new(Client::connect(cfg).await?);
        let tools = client.list_tools().await?;
        tracing::info!(server = %cfg.name, tools = tools.len(), "serveur MCP connecté");
        self.connected.lock().await.insert(
            cfg.id.clone(),
            Connected {
                config: cfg.clone(),
                client,
                tools,
            },
        );
        Ok(())
    }

    /// Ferme la connexion d'un serveur (ses clés ont changé, par exemple).
    pub async fn disconnect(&self, id: &str) {
        self.connected.lock().await.remove(id);
    }

    /// Connexion de test : renvoie les outils proposés par le serveur.
    pub async fn test(&self, cfg: &McpServerConfig) -> Result<Vec<McpTool>> {
        self.connected.lock().await.remove(&cfg.id);
        self.ensure(cfg).await?;
        Ok(self.connected.lock().await[&cfg.id].tools.clone())
    }

    pub async fn specs(&self) -> Vec<ToolSpec> {
        let servers: Vec<_> = self
            .servers
            .lock()
            .await
            .iter()
            .filter(|s| s.enabled)
            .cloned()
            .collect();
        let mut out = Vec::new();
        for s in servers {
            if let Err(e) = self.ensure(&s).await {
                tracing::warn!(server = %s.name, "serveur MCP indisponible : {e:#}");
                continue;
            }
            let conn = self.connected.lock().await;
            for t in &conn[&s.id].tools {
                out.push(ToolSpec {
                    name: exposed_name(&s.id, &t.name),
                    description: format!("[MCP {}] {}", s.name, t.description),
                    parameters: t.input_schema.clone(),
                });
            }
        }
        out
    }

    pub(crate) async fn call(&self, ctx: &Context<'_>, name: &str, args: &Value) -> Result<Done> {
        let (server, client, tool) = {
            let conn = self.connected.lock().await;
            conn.values()
                .find_map(|c| {
                    c.tools
                        .iter()
                        .find(|t| exposed_name(&c.config.id, &t.name) == name)
                        .map(|t| (c.config.name.clone(), c.client.clone(), t.name.clone()))
                })
                .with_context(|| format!("outil MCP inconnu ou serveur déconnecté : {name}"))?
        };
        let args_text = serde_json::to_string_pretty(args)?;
        let decision = ctx
            .authorize(
                name,
                Action::Mcp {
                    server: server.clone(),
                    tool: tool.clone(),
                    args: args_text.clone(),
                },
            )
            .await?;
        let mut out = client.call_tool(&tool, args).await?;
        out.content = untrusted(&format!("serveur MCP {server}"), &out.content);
        Ok(Done {
            output: out,
            detail: format!("{server} / {tool} {args_text}"),
            decision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            exposed_name("mcp-1", "search.repos"),
            "mcp__mcp-1__search_repos"
        );
        assert!(exposed_name("x", &"a".repeat(100)).len() <= 64);
        assert!(is_mcp_tool("mcp__x__y"));
        assert!(!is_mcp_tool("read_file"));
    }
}
