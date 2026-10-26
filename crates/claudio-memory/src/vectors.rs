//! Tables vectorielles (sqlite-vec) et requêtes plein texte (FTS5).

use anyhow::Result;
use claudio_core::Database;
use rusqlite::{OptionalExtension, params};

pub(crate) const CHUNKS: &str = "chunks_vec";
pub(crate) const MEMORIES: &str = "memories_vec";

fn blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Crée les tables vectorielles pour ce modèle ; si le modèle ou la dimension a
/// changé, les anciens vecteurs sont effacés et seront recalculés.
pub fn ensure_vectors(db: &Database, model: &str, dims: usize) -> Result<()> {
    let wanted = format!("{model}:{dims}");
    if db.kv_get("embed_model")?.as_deref() == Some(wanted.as_str()) && table_exists(db, CHUNKS)? {
        return Ok(());
    }
    tracing::info!(model, dims, "initialisation des vecteurs");
    db.with(|c| {
        c.execute_batch(&format!(
            "DROP TABLE IF EXISTS {CHUNKS}; DROP TABLE IF EXISTS {MEMORIES};
             CREATE VIRTUAL TABLE {CHUNKS} USING vec0(embedding float[{dims}]);
             CREATE VIRTUAL TABLE {MEMORIES} USING vec0(embedding float[{dims}]);
             UPDATE chunks SET embedded = 0; UPDATE memories SET embedded = 0;"
        ))
    })?;
    db.kv_set("embed_model", &wanted)
}

pub(crate) fn table_exists(db: &Database, table: &str) -> Result<bool> {
    db.with(|c| {
        c.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|o| o.is_some())
    })
}

pub(crate) fn insert(db: &Database, table: &str, id: i64, v: &[f32]) -> Result<()> {
    db.with(|c| {
        c.execute(&format!("DELETE FROM {table} WHERE rowid = ?1"), [id])?;
        c.execute(
            &format!("INSERT INTO {table}(rowid, embedding) VALUES (?1, ?2)"),
            params![id, blob(v)],
        )
    })?;
    Ok(())
}

pub(crate) fn delete(db: &Database, table: &str, ids: &[i64]) -> Result<()> {
    if ids.is_empty() || !table_exists(db, table)? {
        return Ok(());
    }
    db.with(|c| {
        let mut stmt = c.prepare(&format!("DELETE FROM {table} WHERE rowid = ?1"))?;
        for id in ids {
            stmt.execute([id])?;
        }
        Ok(())
    })
}

/// Plus proches voisins : (identifiant, similarité cosinus).
pub(crate) fn knn(db: &Database, table: &str, query: &[f32], k: usize) -> Result<Vec<(i64, f32)>> {
    if !table_exists(db, table)? {
        return Ok(Vec::new());
    }
    db.with(|c| {
        c.prepare(&format!(
            "SELECT rowid, distance FROM {table} WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance"
        ))?
        .query_map(params![blob(query), k as i64], |r| {
            let d: f64 = r.get(1)?;
            // Vecteurs normalisés : distance² = 2 − 2·cos.
            Ok((r.get(0)?, (1.0 - d * d / 2.0) as f32))
        })?
        .collect()
    })
}

/// Requête FTS5 sûre : mots significatifs entre guillemets, reliés par OR.
pub fn fts_query(text: &str) -> Option<String> {
    const STOP: &[&str] = &[
        "les", "des", "une", "est", "que", "qui", "dans", "pour", "par", "sur", "avec", "mon",
        "mes", "ton", "tes", "son", "ses", "the", "and", "pas", "plus", "quel", "quelle", "quels",
        "comment",
    ];
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.to_lowercase().as_str()))
        .take(16)
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_queries_are_sanitized() {
        assert_eq!(
            fts_query("Quel est le loyer ?").as_deref(),
            Some("\"loyer\"")
        );
        assert_eq!(fts_query("a \"OR\" b").as_deref(), None);
        assert!(
            fts_query("facture d'électricité 2026")
                .unwrap()
                .contains("\"électricité\"")
        );
    }

    #[test]
    fn vectors_roundtrip_and_model_change() {
        let db = Database::open_in_memory().unwrap();
        assert!(knn(&db, CHUNKS, &[1.0, 0.0], 3).unwrap().is_empty());
        ensure_vectors(&db, "m", 2).unwrap();
        insert(&db, CHUNKS, 1, &[1.0, 0.0]).unwrap();
        insert(&db, CHUNKS, 2, &[0.0, 1.0]).unwrap();
        let r = knn(&db, CHUNKS, &[0.8, 0.6], 2).unwrap();
        assert_eq!(r[0].0, 1);
        assert!((r[0].1 - 0.8).abs() < 1e-3);
        delete(&db, CHUNKS, &[1]).unwrap();
        assert_eq!(knn(&db, CHUNKS, &[1.0, 0.0], 2).unwrap().len(), 1);
        // Autre modèle : table recréée, vide.
        ensure_vectors(&db, "autre", 3).unwrap();
        assert!(knn(&db, CHUNKS, &[1.0, 0.0, 0.0], 2).unwrap().is_empty());
    }
}
