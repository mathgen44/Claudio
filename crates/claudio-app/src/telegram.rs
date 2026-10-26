//! Canal Telegram : parler à Claudio depuis son téléphone. Le robot ne répond
//! qu'aux conversations jumelées avec un code affiché dans l'application ; les
//! demandes d'autorisation arrivent sous forme de boutons.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use claudio_core::config::TelegramChat;
use claudio_core::secrets;
use claudio_models::CancelToken;
use claudio_perms::Answer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use crate::{Claudio, StreamEvent};

const API: &str = "https://api.telegram.org";
/// Taille maximale d'un message Telegram (4096), avec une marge.
const CHUNK: usize = 4000;
const PAIRING_TTL: Duration = Duration::from_secs(600);
const PAIRING_ATTEMPTS: u32 = 5;

#[derive(Default)]
pub(crate) struct State {
    pairing: Mutex<Option<Pairing>>,
    run: Mutex<Option<CancelToken>>,
    info: Mutex<Info>,
}

struct Pairing {
    code: String,
    created: Instant,
    attempts: u32,
}

#[derive(Default, Clone)]
struct Info {
    bot: Option<String>,
    error: Option<String>,
    running: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TelegramStatus {
    pub enabled: bool,
    pub has_token: bool,
    /// Nom du robot (@…), une fois le jeton vérifié.
    pub bot: Option<String>,
    pub running: bool,
    pub error: Option<String>,
    pub chats: Vec<TelegramChat>,
    /// Code de jumelage en cours de validité.
    pub pairing_code: Option<String>,
}

// ---- API Telegram (sous-ensemble) ---------------------------------------------

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct Update {
    update_id: i64,
    message: Option<TgMessage>,
    callback_query: Option<Callback>,
}

#[derive(Deserialize)]
struct TgMessage {
    message_id: i64,
    chat: Chat,
    from: Option<User>,
    text: Option<String>,
    voice: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Chat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct User {
    first_name: String,
    username: Option<String>,
}

#[derive(Deserialize)]
struct Callback {
    id: String,
    data: Option<String>,
    message: Option<TgMessage>,
}

#[derive(Deserialize)]
struct Me {
    username: String,
}

/// Client de l'API des robots ; le jeton n'apparaît jamais dans les erreurs.
#[derive(Clone)]
struct Bot {
    http: reqwest::Client,
    token: String,
}

impl Bot {
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        body: serde_json::Value,
        timeout: Duration,
    ) -> Result<T> {
        let r = self
            .http
            .post(format!("{API}/bot{}/{method}", self.token))
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Telegram injoignable : {}", e.without_url()))?;
        let env: Envelope<T> = r
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("réponse de Telegram illisible : {}", e.without_url()))?;
        match (env.ok, env.result) {
            (true, Some(v)) => Ok(v),
            _ => bail!(
                "Telegram : {}",
                env.description.unwrap_or_else(|| "erreur inconnue".into())
            ),
        }
    }

    async fn send(&self, chat: i64, text: &str) -> Result<i64> {
        let mut last = 0;
        for part in split(text, CHUNK) {
            let m: TgMessage = self
                .call(
                    "sendMessage",
                    json!({ "chat_id": chat, "text": part }),
                    Duration::from_secs(30),
                )
                .await?;
            last = m.message_id;
        }
        Ok(last)
    }

    async fn typing(&self, chat: i64) {
        let _ = self
            .call::<bool>(
                "sendChatAction",
                json!({ "chat_id": chat, "action": "typing" }),
                Duration::from_secs(10),
            )
            .await;
    }
}

/// Découpe un texte en morceaux d'au plus `max` caractères, de préférence aux
/// fins de ligne.
fn split(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.split_inclusive('\n') {
        if cur.chars().count() + line.chars().count() > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if line.chars().count() > max {
            let chars: Vec<char> = line.chars().collect();
            for c in chars.chunks(max) {
                out.push(c.iter().collect());
            }
        } else {
            cur.push_str(line);
        }
    }
    if !cur.trim().is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_answer(data: &str) -> Option<(&str, Answer)> {
    let rest = data.strip_prefix("ap|")?;
    let (id, a) = rest.rsplit_once('|')?;
    let answer = match a {
        "once" => Answer::Once,
        "session" => Answer::Session,
        "deny" => Answer::Deny,
        _ => return None,
    };
    Some((id, answer))
}

fn display_name(from: Option<&User>) -> String {
    match from {
        Some(u) => match &u.username {
            Some(n) => format!("{} (@{n})", u.first_name),
            None => u.first_name.clone(),
        },
        None => "inconnu".into(),
    }
}

impl Claudio {
    fn telegram_token(&self) -> Option<String> {
        secrets::get(secrets::TELEGRAM_TOKEN).ok().flatten()
    }

    pub fn telegram_status(&self) -> TelegramStatus {
        let cfg = self.config().telegram;
        let info = self.telegram.info.lock().unwrap().clone();
        let pairing = self.telegram.pairing.lock().unwrap();
        TelegramStatus {
            enabled: cfg.enabled,
            has_token: self.telegram_token().is_some(),
            bot: info.bot,
            running: info.running,
            error: info.error,
            chats: cfg.chats,
            pairing_code: pairing
                .as_ref()
                .filter(|p| p.created.elapsed() < PAIRING_TTL)
                .map(|p| p.code.clone()),
        }
    }

    /// Enregistre le jeton du robot après l'avoir vérifié auprès de Telegram.
    pub async fn set_telegram_token(self: &Arc<Self>, token: Option<&str>) -> Result<()> {
        self.stop_telegram();
        match token.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => {
                let bot = Bot {
                    http: self.http.clone(),
                    token: t.to_owned(),
                };
                let me: Me = bot
                    .call("getMe", json!({}), Duration::from_secs(20))
                    .await
                    .context("jeton refusé par Telegram")?;
                secrets::set(secrets::TELEGRAM_TOKEN, t)?;
                self.telegram.info.lock().unwrap().bot = Some(format!("@{}", me.username));
            }
            None => {
                secrets::delete(secrets::TELEGRAM_TOKEN)?;
                *self.telegram.info.lock().unwrap() = Info::default();
            }
        }
        self.start_telegram();
        Ok(())
    }

    pub fn set_telegram_enabled(self: &Arc<Self>, enabled: bool) -> Result<()> {
        self.update_config(|c| c.telegram.enabled = enabled)?;
        self.stop_telegram();
        self.start_telegram();
        Ok(())
    }

    /// Nouveau code de jumelage, valable 10 minutes, à envoyer au robot.
    pub fn telegram_pairing_code(&self) -> String {
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).expect("générateur aléatoire du système");
        let code = format!("{:06}", u32::from_le_bytes(bytes) % 1_000_000);
        *self.telegram.pairing.lock().unwrap() = Some(Pairing {
            code: code.clone(),
            created: Instant::now(),
            attempts: 0,
        });
        code
    }

    pub fn remove_telegram_chat(&self, id: i64) -> Result<()> {
        self.update_config(|c| c.telegram.chats.retain(|x| x.id != id))?;
        self.db.kv_set(&format!("telegram_session:{id}"), "")?;
        Ok(())
    }

    pub(crate) fn stop_telegram(&self) {
        if let Some(t) = self.telegram.run.lock().unwrap().take() {
            t.cancel();
        }
        self.telegram.info.lock().unwrap().running = false;
    }

    /// Démarre l'écoute des messages si le canal est activé et configuré.
    pub fn start_telegram(self: &Arc<Self>) {
        let Some(token) = self.telegram_token() else {
            return;
        };
        if !self.config().telegram.enabled {
            return;
        }
        let cancel = CancelToken::default();
        *self.telegram.run.lock().unwrap() = Some(cancel.clone());
        let bot = Bot {
            http: self.http.clone(),
            token,
        };
        let app = self.clone();
        tokio::spawn(async move { app.poll(bot, cancel).await });
    }

    async fn poll(self: Arc<Self>, bot: Bot, cancel: CancelToken) {
        self.telegram.info.lock().unwrap().running = true;
        if let Ok(me) = bot
            .call::<Me>("getMe", json!({}), Duration::from_secs(20))
            .await
        {
            self.telegram.info.lock().unwrap().bot = Some(format!("@{}", me.username));
        }
        let mut offset: i64 = self
            .db
            .kv_get("telegram_offset")
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut backoff = Duration::from_secs(2);
        tracing::info!("canal Telegram démarré");
        while !cancel.is_cancelled() {
            let res: Result<Vec<Update>> = bot
                .call(
                    "getUpdates",
                    json!({ "offset": offset, "timeout": 25, "allowed_updates": ["message", "callback_query"] }),
                    Duration::from_secs(40),
                )
                .await;
            match res {
                Ok(updates) => {
                    backoff = Duration::from_secs(2);
                    self.telegram.info.lock().unwrap().error = None;
                    for u in updates {
                        offset = u.update_id + 1;
                        let _ = self.db.kv_set("telegram_offset", &offset.to_string());
                        if cancel.is_cancelled() {
                            break;
                        }
                        self.handle_update(&bot, u).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("Telegram : {e:#}");
                    self.telegram.info.lock().unwrap().error = Some(format!("{e:#}"));
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
        tracing::info!("canal Telegram arrêté");
    }

    fn chat_allowed(&self, id: i64) -> bool {
        self.config().telegram.chats.iter().any(|c| c.id == id)
    }

    async fn handle_update(self: &Arc<Self>, bot: &Bot, u: Update) {
        if let Some(cb) = u.callback_query {
            self.handle_callback(bot, cb).await;
            return;
        }
        let Some(m) = u.message else { return };
        // Uniquement les conversations privées : pas de groupes.
        if m.chat.kind != "private" {
            return;
        }
        let chat = m.chat.id;
        let text = m.text.clone().unwrap_or_default();
        let text = text.trim();
        if let Some(code) = text
            .strip_prefix("/jumeler")
            .or_else(|| text.strip_prefix("/start"))
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            let reply = self.pair(chat, code, display_name(m.from.as_ref()));
            let _ = bot.send(chat, &reply).await;
            return;
        }
        if !self.chat_allowed(chat) {
            let _ = bot
                .send(
                    chat,
                    "Ce robot est privé. Pour le relier à votre Claudio, ouvrez Claudio › Voix et Telegram, \
                     créez un code de jumelage, puis envoyez ici : /jumeler CODE",
                )
                .await;
            return;
        }
        match text {
            "/nouveau" | "/new" => {
                let _ = self.db.kv_set(&format!("telegram_session:{chat}"), "");
                let _ = bot.send(chat, "Nouvelle conversation.").await;
            }
            "/stop" => {
                if let Ok(sid) = self.telegram_session(chat) {
                    self.cancel_chat(&sid);
                }
            }
            "/start" | "/aide" | "/help" => {
                let _ = bot
                    .send(
                        chat,
                        "Écrivez votre demande, Claudio répond depuis votre ordinateur.\n\
                         /nouveau : nouvelle conversation\n/stop : interrompre la réponse en cours",
                    )
                    .await;
            }
            _ if m.voice.is_some() => {
                let _ = bot
                    .send(
                        chat,
                        "Les messages vocaux ne sont pas pris en charge ici : écrivez votre demande.",
                    )
                    .await;
            }
            "" => {}
            _ => {
                // En tâche à part : la réception continue (boutons d'autorisation).
                let (app, bot, text) = (self.clone(), bot.clone(), text.to_owned());
                tokio::spawn(async move {
                    if let Err(e) = app.answer_telegram(&bot, chat, &text).await {
                        tracing::warn!("Telegram : {e:#}");
                        let _ = bot.send(chat, &format!("Erreur : {e:#}")).await;
                    }
                });
            }
        }
    }

    fn pair(&self, chat: i64, code: &str, name: String) -> String {
        let mut pairing = self.telegram.pairing.lock().unwrap();
        let Some(p) = pairing.as_mut() else {
            return "Aucun jumelage en cours : créez un code dans Claudio.".into();
        };
        if p.created.elapsed() > PAIRING_TTL {
            *pairing = None;
            return "Ce code a expiré : créez-en un nouveau dans Claudio.".into();
        }
        if p.code != code {
            p.attempts += 1;
            if p.attempts >= PAIRING_ATTEMPTS {
                *pairing = None;
                return "Code incorrect. Trop d'essais : créez un nouveau code dans Claudio."
                    .into();
            }
            return "Code incorrect.".into();
        }
        *pairing = None;
        drop(pairing);
        let res = self.update_config(|c| {
            if !c.telegram.chats.iter().any(|x| x.id == chat) {
                c.telegram.chats.push(TelegramChat {
                    id: chat,
                    name: name.clone(),
                });
            }
        });
        match res {
            Ok(_) => {
                tracing::info!(chat, "conversation Telegram jumelée");
                "Jumelage réussi : vous pouvez parler à Claudio. Les messages transitent par les serveurs de Telegram."
                    .into()
            }
            Err(e) => format!("Jumelage impossible : {e:#}"),
        }
    }

    /// Conversation Claudio associée à cette conversation Telegram.
    fn telegram_session(&self, chat: i64) -> Result<String> {
        let key = format!("telegram_session:{chat}");
        if let Some(id) = self.db.kv_get(&key)?.filter(|s| !s.is_empty())
            && self.db.get_session(&id)?.is_some()
        {
            return Ok(id);
        }
        let s = self.create_session()?;
        self.db.kv_set(&key, &s.id)?;
        Ok(s.id)
    }

    async fn answer_telegram(self: &Arc<Self>, bot: &Bot, chat: i64, text: &str) -> Result<()> {
        let sid = self.telegram_session(chat)?;
        let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
        // Demandes d'autorisation : envoyées avec des boutons au fil de l'eau.
        let forward = {
            let bot = bot.clone();
            tokio::spawn(async move {
                while let Some(ev) = rx.recv().await {
                    if let StreamEvent::Approval { id, approval, .. } = ev {
                        let mut buttons = vec![
                            json!({ "text": "Autoriser une fois", "callback_data": format!("ap|{id}|once") }),
                        ];
                        if approval.options.contains(&Answer::Session) {
                            buttons.push(json!({ "text": "Pour cette conversation", "callback_data": format!("ap|{id}|session") }));
                        }
                        let text = format!(
                            "{}{}\n\n{}",
                            if approval.dangerous { "⚠️ " } else { "" },
                            approval.title,
                            approval.detail
                        );
                        let _ = bot
                            .call::<serde_json::Value>(
                                "sendMessage",
                                json!({
                                    "chat_id": chat,
                                    "text": text.chars().take(CHUNK).collect::<String>(),
                                    "reply_markup": { "inline_keyboard": [buttons, [{ "text": "Refuser", "callback_data": format!("ap|{id}|deny") }]] }
                                }),
                                Duration::from_secs(30),
                            )
                            .await;
                    }
                }
            })
        };
        let typing = {
            let bot = bot.clone();
            tokio::spawn(async move {
                loop {
                    bot.typing(chat).await;
                    tokio::time::sleep(Duration::from_secs(4)).await;
                }
            })
        };
        let reply = self
            .send_message(&sid, text, |ev| {
                let _ = tx.send(ev);
            })
            .await;
        typing.abort();
        drop(tx);
        let _ = forward.await;
        let reply = reply?;
        bot.send(chat, &reply.message.content).await?;
        Ok(())
    }

    async fn handle_callback(&self, bot: &Bot, cb: Callback) {
        let Some(msg) = cb.message else { return };
        let chat = msg.chat.id;
        let label = match cb.data.as_deref().and_then(parse_answer) {
            Some((id, answer)) if self.chat_allowed(chat) => {
                match self.answer_approval(id, answer) {
                    Ok(()) => match answer {
                        Answer::Deny => "Refusé",
                        Answer::Session => "Autorisé pour cette conversation",
                        _ => "Autorisé une fois",
                    },
                    Err(_) => "Demande expirée",
                }
            }
            _ => "Demande inconnue",
        };
        let _ = bot
            .call::<bool>(
                "answerCallbackQuery",
                json!({ "callback_query_id": cb.id, "text": label }),
                Duration::from_secs(10),
            )
            .await;
        // Retire les boutons et note la réponse sous la demande.
        let _ = bot
            .call::<serde_json::Value>(
                "editMessageText",
                json!({
                    "chat_id": chat,
                    "message_id": msg.message_id,
                    "text": format!("{}\n\n→ {label}", msg.text.unwrap_or_default()),
                }),
                Duration::from_secs(10),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_long_messages() {
        assert_eq!(split("court", 10), vec!["court"]);
        let parts = split("ligne une\nligne deux\nligne trois", 12);
        assert_eq!(parts, vec!["ligne une\n", "ligne deux\n", "ligne trois"]);
        let long = "x".repeat(25);
        assert_eq!(split(&long, 10).len(), 3);
        assert!(
            split(&"é".repeat(9000), CHUNK)
                .iter()
                .all(|p| p.chars().count() <= CHUNK)
        );
    }

    #[test]
    fn parses_button_answers() {
        assert_eq!(parse_answer("ap|ap-3|once"), Some(("ap-3", Answer::Once)));
        assert_eq!(parse_answer("ap|ap-3|deny"), Some(("ap-3", Answer::Deny)));
        assert_eq!(
            parse_answer("ap|ap-3|always"),
            None,
            "jamais « toujours » depuis Telegram"
        );
        assert_eq!(parse_answer("autre"), None);
    }

    #[test]
    fn pairing_codes() {
        let dir = tempfile::tempdir().unwrap();
        let app = Claudio::open(claudio_core::AppPaths::in_dir(dir.path().to_path_buf())).unwrap();
        assert!(app.pair(1, "123456", "A".into()).contains("Aucun"));
        let code = app.telegram_pairing_code();
        assert_eq!(code.len(), 6);
        assert_eq!(
            app.telegram_status().pairing_code.as_deref(),
            Some(code.as_str())
        );
        let wrong = if code == "000000" { "111111" } else { "000000" };
        assert_eq!(app.pair(1, wrong, "A".into()), "Code incorrect.");
        assert!(!app.chat_allowed(1));
        assert!(app.pair(1, &code, "Alice".into()).contains("réussi"));
        assert!(app.chat_allowed(1));
        assert!(
            app.pair(2, &code, "B".into()).contains("Aucun"),
            "code à usage unique"
        );
        // Trop d'essais : le code est annulé.
        let code = app.telegram_pairing_code();
        let wrong = if code == "000000" { "111111" } else { "000000" };
        for _ in 0..PAIRING_ATTEMPTS {
            app.pair(3, wrong, "C".into());
        }
        assert!(app.pair(3, &code, "C".into()).contains("Aucun"));
        app.remove_telegram_chat(1).unwrap();
        assert!(!app.chat_allowed(1));
    }
}
