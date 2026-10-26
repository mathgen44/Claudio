//! Base SQLite : sessions de conversation et messages.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

/// Migrations appliquées dans l'ordre ; l'index suivant est stocké dans `user_version`.
const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE sessions (
        id          TEXT PRIMARY KEY,
        title       TEXT NOT NULL,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL
    );
    CREATE TABLE messages (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        role        TEXT NOT NULL,
        content     TEXT NOT NULL,
        model       TEXT,
        tokens      INTEGER,
        duration_ms INTEGER,
        created_at  TEXT NOT NULL
    );
    CREATE INDEX messages_session ON messages(session_id, id);
"#];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "system" => Role::System,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            other => bail!("rôle inconnu : {other}"),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub session_id: String,
    pub role: Role,
    pub content: String,
    pub model: Option<String>,
    pub tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    pub created_at: DateTime<Utc>,
}

/// Métadonnées facultatives d'un message produit par un modèle.
#[derive(Debug, Clone, Default)]
pub struct MessageMeta {
    pub model: Option<String>,
    pub tokens: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

fn session_from_row(r: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        title: r.get(1)?,
        created_at: r.get(2)?,
        updated_at: r.get(3)?,
    })
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("ouverture de {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            conn.execute_batch(&format!(
                "BEGIN; {sql}; PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))
            .with_context(|| format!("migration {}", i + 1))?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let conn = self.conn.lock().expect("verrou de la base empoisonné");
        Ok(f(&conn)?)
    }

    pub fn create_session(&self, title: &str) -> Result<Session> {
        let now = Utc::now();
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.to_owned(),
            created_at: now,
            updated_at: now,
        };
        self.with(|c| {
            c.execute(
                "INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
                params![session.id, session.title, now],
            )
        })?;
        Ok(session)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        self.with(|c| {
            c.prepare(
                "SELECT id, title, created_at, updated_at FROM sessions ORDER BY updated_at DESC",
            )?
            .query_map([], session_from_row)?
            .collect()
        })
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, title, created_at, updated_at FROM sessions WHERE id = ?1",
                [id],
                session_from_row,
            )
            .optional()
        })
    }

    pub fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET title = ?2 WHERE id = ?1",
                params![id, title],
            )
        })?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.with(|c| c.execute("DELETE FROM sessions WHERE id = ?1", [id]))?;
        Ok(())
    }

    pub fn add_message(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        meta: MessageMeta,
    ) -> Result<Message> {
        let now = Utc::now();
        let id = self.with(|c| {
            c.execute(
                "INSERT INTO messages (session_id, role, content, model, tokens, duration_ms, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![session_id, role.as_str(), content, meta.model, meta.tokens, meta.duration_ms, now],
            )?;
            c.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![session_id, now],
            )?;
            Ok(c.last_insert_rowid())
        })?;
        Ok(Message {
            id,
            session_id: session_id.to_owned(),
            role,
            content: content.to_owned(),
            model: meta.model,
            tokens: meta.tokens,
            duration_ms: meta.duration_ms,
            created_at: now,
        })
    }

    pub fn list_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let rows = self.with(|c| {
            c.prepare(
                "SELECT id, session_id, role, content, model, tokens, duration_ms, created_at
                 FROM messages WHERE session_id = ?1 ORDER BY id",
            )?
            .query_map([session_id], |r| {
                Ok((
                    r.get::<_, String>(2)?,
                    Message {
                        id: r.get(0)?,
                        session_id: r.get(1)?,
                        role: Role::User,
                        content: r.get(3)?,
                        model: r.get(4)?,
                        tokens: r.get(5)?,
                        duration_ms: r.get(6)?,
                        created_at: r.get(7)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        rows.into_iter()
            .map(|(role, mut m)| {
                m.role = Role::parse(&role)?;
                Ok(m)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_and_messages() {
        let db = Database::open_in_memory().unwrap();
        let s = db.create_session("Nouvelle conversation").unwrap();
        db.add_message(&s.id, Role::User, "Bonjour", MessageMeta::default())
            .unwrap();
        db.add_message(
            &s.id,
            Role::Assistant,
            "Salut !",
            MessageMeta {
                model: Some("m".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let msgs = db.list_messages(&s.id).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].role, Role::Assistant);
        assert_eq!(msgs[1].model.as_deref(), Some("m"));
        db.rename_session(&s.id, "Salutations").unwrap();
        assert_eq!(db.list_sessions().unwrap()[0].title, "Salutations");
        db.delete_session(&s.id).unwrap();
        assert!(db.list_messages(&s.id).unwrap().is_empty());
        assert!(db.get_session(&s.id).unwrap().is_none());
    }

    #[test]
    fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        Database::open(&path).unwrap();
        Database::open(&path).unwrap();
    }
}
