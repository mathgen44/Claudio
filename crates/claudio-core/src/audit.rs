//! Journal d'audit des actions des outils et autorisations permanentes.
//!
//! Le journal est en ajout seul (des déclencheurs SQL refusent modification et
//! suppression) et chaîné : chaque entrée contient l'empreinte de la précédente,
//! ce qui rend visible toute altération du fichier.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Database;

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone)]
pub struct AuditRecord<'a> {
    pub session_id: Option<&'a str>,
    /// Outil appelé, ex. `read_file`, `mcp__github__search`.
    pub tool: &'a str,
    /// Ce qui a été demandé (chemin, adresse, commande…).
    pub detail: &'a str,
    /// Comment l'action a été autorisée : `auto`, `approuvé`, `refusé`…
    pub decision: &'a str,
    /// Résultat : `ok`, ou le message d'erreur.
    pub outcome: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub session_id: Option<String>,
    pub tool: String,
    pub detail: String,
    pub decision: String,
    pub outcome: String,
}

/// Autorisation permanente enregistrée.
#[derive(Debug, Clone, Serialize)]
pub struct StoredGrant {
    pub id: i64,
    /// `folder`, `host`, `mcp`.
    pub kind: String,
    pub target: String,
    pub write: bool,
    pub created_at: DateTime<Utc>,
}

fn entry_hash(prev: &str, at: &str, r: &AuditRecord) -> String {
    let mut h = Sha256::new();
    for part in [
        prev,
        at,
        r.session_id.unwrap_or(""),
        r.tool,
        r.detail,
        r.decision,
        r.outcome,
    ] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    hex::encode(h.finalize())
}

impl Database {
    pub fn audit(&self, r: &AuditRecord) -> Result<()> {
        // Les champs trop longs (sorties de commandes…) sont résumés.
        let detail: String = r.detail.chars().take(2000).collect();
        let outcome: String = r.outcome.chars().take(1000).collect();
        let r = AuditRecord {
            detail: &detail,
            outcome: &outcome,
            ..r.clone()
        };
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let prev: String = tx
                .query_row("SELECT hash FROM audit ORDER BY id DESC LIMIT 1", [], |row| {
                    row.get(0)
                })
                .optional()?
                .unwrap_or_else(|| GENESIS.to_owned());
            let at = Utc::now().to_rfc3339();
            let hash = entry_hash(&prev, &at, &r);
            tx.execute(
                "INSERT INTO audit (at, session_id, tool, detail, decision, outcome, prev_hash, hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![at, r.session_id, r.tool, r.detail, r.decision, r.outcome, prev, hash],
            )?;
            tx.commit()
        })
    }

    /// Dernières entrées, de la plus récente à la plus ancienne.
    pub fn audit_entries(&self, limit: u32, before_id: Option<i64>) -> Result<Vec<AuditEntry>> {
        self.with(|c| {
            c.prepare(
                "SELECT id, at, session_id, tool, detail, decision, outcome FROM audit
                 WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
            )?
            .query_map(params![before_id.unwrap_or(i64::MAX), limit], |r| {
                let at: String = r.get(1)?;
                Ok(AuditEntry {
                    id: r.get(0)?,
                    at: DateTime::parse_from_rfc3339(&at)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_default(),
                    session_id: r.get(2)?,
                    tool: r.get(3)?,
                    detail: r.get(4)?,
                    decision: r.get(5)?,
                    outcome: r.get(6)?,
                })
            })?
            .collect()
        })
    }

    /// Recalcule la chaîne d'empreintes ; renvoie l'identifiant de la première
    /// entrée altérée, ou `None` si le journal est intact.
    pub fn verify_audit(&self) -> Result<Option<i64>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, at, session_id, tool, detail, decision, outcome, prev_hash, hash
                 FROM audit ORDER BY id",
            )?;
            let mut rows = stmt.query([])?;
            let mut prev = GENESIS.to_owned();
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let at: String = row.get(1)?;
                let session: Option<String> = row.get(2)?;
                let (tool, detail, decision, outcome): (String, String, String, String) =
                    (row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?);
                let (stored_prev, stored): (String, String) = (row.get(7)?, row.get(8)?);
                let expected = entry_hash(
                    &prev,
                    &at,
                    &AuditRecord {
                        session_id: session.as_deref(),
                        tool: &tool,
                        detail: &detail,
                        decision: &decision,
                        outcome: &outcome,
                    },
                );
                if stored_prev != prev || stored != expected {
                    return Ok(Some(id));
                }
                prev = stored;
            }
            Ok(None)
        })
    }

    pub fn add_grant(&self, kind: &str, target: &str, write: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO grants (kind, target, write, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(kind, target) DO UPDATE SET write = MAX(write, excluded.write)",
                params![kind, target, write, Utc::now()],
            )
        })?;
        Ok(())
    }

    pub fn grants(&self) -> Result<Vec<StoredGrant>> {
        self.with(|c| {
            c.prepare(
                "SELECT id, kind, target, write, created_at FROM grants ORDER BY kind, target",
            )?
            .query_map([], |r| {
                Ok(StoredGrant {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    target: r.get(2)?,
                    write: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect()
        })
    }

    pub fn remove_grant(&self, id: i64) -> Result<()> {
        self.with(|c| c.execute("DELETE FROM grants WHERE id = ?1", [id]))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec<'a>(tool: &'a str, detail: &'a str) -> AuditRecord<'a> {
        AuditRecord {
            session_id: Some("s"),
            tool,
            detail,
            decision: "auto",
            outcome: "ok",
        }
    }

    #[test]
    fn chain_detects_tampering_and_refuses_edits() {
        let db = Database::open_in_memory().unwrap();
        db.audit(&rec("read_file", "C:/a.txt")).unwrap();
        db.audit(&rec("write_file", "C:/b.txt")).unwrap();
        db.audit(&rec("shell", "dir")).unwrap();
        assert_eq!(db.verify_audit().unwrap(), None);
        let entries = db.audit_entries(10, None).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].tool, "shell");
        assert_eq!(db.audit_entries(10, Some(entries[0].id)).unwrap().len(), 2);

        // Le journal refuse modification et suppression.
        assert!(
            db.with(|c| c.execute("UPDATE audit SET detail = 'x' WHERE id = 2", []))
                .is_err()
        );
        assert!(db.with(|c| c.execute("DELETE FROM audit", [])).is_err());

        // Une altération directe (déclencheurs retirés) est détectée.
        db.with(|c| {
            c.execute_batch(
                "DROP TRIGGER audit_no_update; UPDATE audit SET detail = 'C:/autre.txt' WHERE id = 2;",
            )
        })
        .unwrap();
        assert_eq!(db.verify_audit().unwrap(), Some(2));
    }

    #[test]
    fn grants_upgrade_to_write() {
        let db = Database::open_in_memory().unwrap();
        db.add_grant("folder", "C:/Docs", false).unwrap();
        db.add_grant("folder", "C:/Docs", true).unwrap();
        db.add_grant("folder", "C:/Docs", false).unwrap();
        let g = db.grants().unwrap();
        assert_eq!(g.len(), 1);
        assert!(g[0].write);
        db.remove_grant(g[0].id).unwrap();
        assert!(db.grants().unwrap().is_empty());
    }
}
