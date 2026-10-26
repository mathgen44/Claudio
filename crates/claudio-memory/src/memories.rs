//! Souvenirs à long terme : faits et préférences, visibles, modifiables et
//! supprimables par l'utilisateur.

use std::sync::atomic::AtomicBool;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use claudio_core::Database;
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};

use crate::vectors::{self, MEMORIES};
use crate::{Embedder, fts_query, rrf};

/// Au-delà, un nouveau souvenir est considéré comme un doublon.
pub const DUPLICATE_SIMILARITY: f32 = 0.9;
/// En dessous, un souvenir n'est pas jugé pertinent pour la question.
const RECALL_SIMILARITY: f32 = 0.35;

pub const CATEGORIES: &[&str] = &["préférence", "personnel", "travail", "projet", "autre"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub category: String,
    /// `manuel`, `assistant` (outil remember) ou `auto` (extraction).
    pub source: String,
    pub session_id: Option<String>,
    /// Toujours rappelé.
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, content, category, source, session_id, pinned, created_at, updated_at";

fn from_row(r: &Row<'_>) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: r.get(0)?,
        content: r.get(1)?,
        category: r.get(2)?,
        source: r.get(3)?,
        session_id: r.get(4)?,
        pinned: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

fn clean(content: &str, category: &str) -> Result<(String, String)> {
    let content = content.trim();
    if content.is_empty() {
        bail!("souvenir vide");
    }
    if content.chars().count() > 1000 {
        bail!("souvenir trop long (1000 caractères au plus)");
    }
    let category = if CATEGORIES.contains(&category) {
        category
    } else {
        "autre"
    };
    Ok((content.to_owned(), category.to_owned()))
}

pub fn get(db: &Database, id: i64) -> Result<Option<Memory>> {
    use rusqlite::OptionalExtension;
    db.with(|c| {
        c.query_row(
            &format!("SELECT {COLUMNS} FROM memories WHERE id = ?1"),
            [id],
            from_row,
        )
        .optional()
    })
}

/// Ajoute un souvenir (son vecteur est calculé si un embedder est fourni).
pub async fn add(
    db: &Database,
    embedder: Option<&dyn Embedder>,
    content: &str,
    category: &str,
    source: &str,
    session_id: Option<&str>,
) -> Result<Memory> {
    let (content, category) = clean(content, category)?;
    let now = Utc::now();
    let id = db.with(|c| {
        c.execute(
            "INSERT INTO memories (content, category, source, session_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![content, category, source, session_id, now],
        )?;
        Ok(c.last_insert_rowid())
    })?;
    if let Some(e) = embedder {
        embed_one(db, e, id, &content).await?;
    }
    Ok(get(db, id)?.expect("souvenir créé"))
}

async fn embed_one(db: &Database, e: &dyn Embedder, id: i64, content: &str) -> Result<()> {
    vectors::ensure_vectors(db, &e.model(), e.dims())?;
    if let Some(v) = e.embed(&[content.to_owned()], false).await?.first() {
        vectors::insert(db, MEMORIES, id, v)?;
        db.with(|c| c.execute("UPDATE memories SET embedded = 1 WHERE id = ?1", [id]))?;
    }
    Ok(())
}

pub async fn update(
    db: &Database,
    embedder: Option<&dyn Embedder>,
    id: i64,
    content: &str,
    category: &str,
    pinned: bool,
) -> Result<Memory> {
    let (content, category) = clean(content, category)?;
    db.with(|c| {
        c.execute(
            "UPDATE memories SET content = ?2, category = ?3, pinned = ?4, embedded = 0, updated_at = ?5
             WHERE id = ?1",
            params![id, content, category, pinned, Utc::now()],
        )
    })?;
    vectors::delete(db, MEMORIES, &[id])?;
    if let Some(e) = embedder {
        embed_one(db, e, id, &content).await?;
    }
    get(db, id)?.ok_or_else(|| anyhow::anyhow!("souvenir introuvable"))
}

pub fn delete(db: &Database, id: i64) -> Result<()> {
    vectors::delete(db, MEMORIES, &[id])?;
    db.with(|c| c.execute("DELETE FROM memories WHERE id = ?1", [id]))?;
    Ok(())
}

pub fn delete_all(db: &Database) -> Result<usize> {
    let ids: Vec<i64> = list(db, None)?.iter().map(|m| m.id).collect();
    vectors::delete(db, MEMORIES, &ids)?;
    db.with(|c| c.execute("DELETE FROM memories", []))
}

/// Tous les souvenirs (filtrés par texte si `filter`), les plus récents d'abord.
pub fn list(db: &Database, filter: Option<&str>) -> Result<Vec<Memory>> {
    let filter = filter
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(|f| format!("%{f}%"));
    db.with(|c| {
        c.prepare(&format!(
            "SELECT {COLUMNS} FROM memories WHERE ?1 IS NULL OR content LIKE ?1 OR category LIKE ?1
             ORDER BY pinned DESC, updated_at DESC"
        ))?
        .query_map([filter], from_row)?
        .collect()
    })
}

/// Souvenirs utiles pour cette question : épinglés, puis les plus proches.
pub async fn recall(
    db: &Database,
    embedder: Option<&dyn Embedder>,
    query: &str,
    k: usize,
) -> Result<Vec<Memory>> {
    let mut lists = Vec::new();
    if let Some(q) = fts_query(query) {
        lists.push(db.with(|c| {
            c.prepare("SELECT rowid FROM memories_fts WHERE memories_fts MATCH ?1 ORDER BY bm25(memories_fts) LIMIT 20")?
                .query_map([q], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()
        })?);
    }
    if let Some(e) = embedder
        && vectors::table_exists(db, MEMORIES)?
        && let Some(v) = e.embed(&[query.to_owned()], true).await?.first()
    {
        lists.push(
            vectors::knn(db, MEMORIES, v, 20)?
                .into_iter()
                .filter(|(_, sim)| *sim >= RECALL_SIMILARITY)
                .map(|(id, _)| id)
                .collect(),
        );
    }
    let mut out: Vec<Memory> = list(db, None)?.into_iter().filter(|m| m.pinned).collect();
    for (id, _) in rrf(&lists) {
        if out.len() >= k + out.iter().filter(|m| m.pinned).count() {
            break;
        }
        if !out.iter().any(|m| m.id == id)
            && let Some(m) = get(db, id)?
        {
            out.push(m);
        }
    }
    Ok(out)
}

/// Souvenir existant presque identique, s'il y en a un.
pub async fn find_duplicate(
    db: &Database,
    embedder: &dyn Embedder,
    content: &str,
) -> Result<Option<Memory>> {
    if !vectors::table_exists(db, MEMORIES)? {
        return Ok(None);
    }
    let Some(v) = embedder
        .embed(&[content.to_owned()], false)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    match vectors::knn(db, MEMORIES, &v, 1)?.first() {
        Some((id, sim)) if *sim >= DUPLICATE_SIMILARITY => get(db, *id),
        _ => Ok(None),
    }
}

/// Calcule les vecteurs des souvenirs qui n'en ont pas (après changement de modèle).
pub async fn embed_pending(
    db: &Database,
    embedder: &dyn Embedder,
    _cancel: &AtomicBool,
) -> Result<usize> {
    vectors::ensure_vectors(db, &embedder.model(), embedder.dims())?;
    let todo: Vec<(i64, String)> = db.with(|c| {
        c.prepare("SELECT id, content FROM memories WHERE embedded = 0")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    })?;
    for (id, content) in &todo {
        embed_one(db, embedder, *id, content).await?;
    }
    Ok(todo.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Hashing;

    #[tokio::test]
    async fn add_recall_update_delete() {
        let db = Database::open_in_memory().unwrap();
        let e: &dyn Embedder = &Hashing;
        let chat = add(
            &db,
            Some(e),
            "Hervé a un chat nommé Pixel",
            "personnel",
            "auto",
            None,
        )
        .await
        .unwrap();
        add(
            &db,
            Some(e),
            "Hervé préfère les réponses courtes",
            "préférence",
            "manuel",
            None,
        )
        .await
        .unwrap();
        let pinned = add(
            &db,
            Some(e),
            "Hervé habite à Nantes",
            "pas-une-catégorie",
            "manuel",
            None,
        )
        .await
        .unwrap();
        assert_eq!(pinned.category, "autre");
        update(
            &db,
            Some(e),
            pinned.id,
            "Hervé habite à Nantes",
            "personnel",
            true,
        )
        .await
        .unwrap();

        let r = recall(&db, Some(e), "Comment s'appelle mon chat ?", 2)
            .await
            .unwrap();
        assert!(r[0].pinned, "les souvenirs épinglés viennent toujours");
        assert!(r.iter().any(|m| m.id == chat.id));

        // Rappel sans embeddings : plein texte.
        let r = recall(&db, None, "chat Pixel", 3).await.unwrap();
        assert!(r.iter().any(|m| m.id == chat.id));

        assert!(
            find_duplicate(&db, e, "Hervé a un chat nommé Pixel")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_duplicate(&db, e, "Le train part à midi")
                .await
                .unwrap()
                .is_none()
        );

        assert_eq!(list(&db, Some("courtes")).unwrap().len(), 1);
        delete(&db, chat.id).unwrap();
        assert!(
            recall(&db, None, "chat Pixel", 3)
                .await
                .unwrap()
                .iter()
                .all(|m| m.id != chat.id)
        );
        assert!(add(&db, None, "  ", "autre", "manuel", None).await.is_err());
        assert_eq!(delete_all(&db).unwrap(), 2);
    }
}
