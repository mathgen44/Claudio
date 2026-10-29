//! Index des dossiers autorisés : parcours incrémental, extraction, découpage,
//! embeddings, et recherche hybride.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use claudio_core::Database;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::vectors::{self, CHUNKS};
use crate::{Embedder, fts_query, rrf};

/// Dossiers ignorés lors du parcours.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    ".svn",
    "$recycle.bin",
    "__pycache__",
    ".venv",
    "venv",
    "dist",
    "build",
    ".cache",
];
const EMBED_BATCH: usize = 16;

#[derive(Debug, Clone, Serialize)]
pub struct SourceView {
    pub id: i64,
    pub path: String,
    pub enabled: bool,
    /// `idle`, `scanning`, `embedding`, `error`.
    pub status: String,
    pub error: Option<String>,
    pub last_scan: Option<DateTime<Utc>>,
    pub files: i64,
    pub failed_files: i64,
    pub chunks: i64,
    pub embedded_chunks: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanStats {
    pub seen: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub path: String,
    pub location: Option<String>,
    pub text: String,
    pub score: f64,
}

pub fn add_source(db: &Database, path: &Path) -> Result<i64> {
    let p = path.display().to_string();
    db.with(|c| {
        c.execute(
            "INSERT INTO index_sources (path, created_at) VALUES (?1, ?2) ON CONFLICT(path) DO NOTHING",
            params![p, Utc::now()],
        )?;
        c.query_row("SELECT id FROM index_sources WHERE path = ?1", [&p], |r| r.get(0))
    })
}

pub fn source_path(db: &Database, id: i64) -> Result<PathBuf> {
    let p: Option<String> = db.with(|c| {
        c.query_row("SELECT path FROM index_sources WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()
    })?;
    Ok(PathBuf::from(p.context("dossier indexé introuvable")?))
}

fn chunk_ids(db: &Database, filter: &str, id: i64) -> Result<Vec<i64>> {
    db.with(|c| {
        c.prepare(&format!(
            "SELECT c.id FROM chunks c JOIN index_files f ON f.id = c.file_id WHERE {filter}"
        ))?
        .query_map([id], |r| r.get(0))?
        .collect()
    })
}

pub fn remove_source(db: &Database, id: i64) -> Result<()> {
    vectors::delete(db, CHUNKS, &chunk_ids(db, "f.source_id = ?1", id)?)?;
    db.with(|c| c.execute("DELETE FROM index_sources WHERE id = ?1", [id]))?;
    Ok(())
}

pub fn set_status(db: &Database, id: i64, status: &str, error: Option<&str>) -> Result<()> {
    db.with(|c| {
        c.execute(
            "UPDATE index_sources SET status = ?2, error = ?3,
                last_scan = CASE WHEN ?2 = 'idle' THEN ?4 ELSE last_scan END
             WHERE id = ?1",
            params![id, status, error, Utc::now()],
        )
    })?;
    Ok(())
}

/// Au démarrage : un parcours interrompu repasse « au repos ».
pub fn reset_statuses(db: &Database) -> Result<()> {
    db.with(|c| {
        c.execute(
            "UPDATE index_sources SET status = 'idle' WHERE status IN ('scanning', 'embedding')",
            [],
        )
    })?;
    Ok(())
}

pub fn sources(db: &Database) -> Result<Vec<SourceView>> {
    db.with(|c| {
        c.prepare(
            "SELECT s.id, s.path, s.enabled, s.status, s.error, s.last_scan,
                (SELECT COUNT(*) FROM index_files f WHERE f.source_id = s.id),
                (SELECT COUNT(*) FROM index_files f WHERE f.source_id = s.id AND f.error IS NOT NULL),
                (SELECT COUNT(*) FROM chunks c JOIN index_files f ON f.id = c.file_id WHERE f.source_id = s.id),
                (SELECT COUNT(*) FROM chunks c JOIN index_files f ON f.id = c.file_id
                   WHERE f.source_id = s.id AND c.embedded = 1)
             FROM index_sources s ORDER BY s.path",
        )?
        .query_map([], |r| {
            Ok(SourceView {
                id: r.get(0)?,
                path: r.get(1)?,
                enabled: r.get(2)?,
                status: r.get(3)?,
                error: r.get(4)?,
                last_scan: r.get(5)?,
                files: r.get(6)?,
                failed_files: r.get(7)?,
                chunks: r.get(8)?,
                embedded_chunks: r.get(9)?,
            })
        })?
        .collect()
    })
}

fn skip_entry(e: &walkdir::DirEntry) -> bool {
    let name = e.file_name().to_string_lossy().to_lowercase();
    e.depth() > 0
        && (name.starts_with('.')
            || name.starts_with('~')
            || (e.file_type().is_dir() && SKIP_DIRS.contains(&name.as_str())))
}

/// Parcourt un dossier et met l'index à jour (fichiers nouveaux, modifiés ou supprimés).
/// Bloquant : à appeler depuis `spawn_blocking`. `progress(fait, vus, chemin)`.
pub fn scan(
    db: &Database,
    source_id: i64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize, &str),
) -> Result<ScanStats> {
    let root = source_path(db, source_id)?;
    if !root.is_dir() {
        bail!("dossier introuvable : {}", root.display());
    }
    let files: Vec<(PathBuf, i64, i64)> = walkdir::WalkDir::new(&root)
        .max_depth(20)
        .into_iter()
        .filter_entry(|e| !skip_entry(e))
        .flatten()
        // Les images (photos surtout) ne sont pas indexées : la reconnaissance de
        // caractères y coûte cher pour peu de texte.
        .filter(|e| {
            e.file_type().is_file() && claudio_docs::kind_of(e.path()).is_some_and(|k| k != "image")
        })
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            (m.len() <= claudio_docs::MAX_FILE_BYTES).then_some(())?;
            let mtime = m
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_secs() as i64;
            Some((e.into_path(), mtime, m.len() as i64))
        })
        .collect();

    let mut stats = ScanStats {
        seen: files.len(),
        ..Default::default()
    };
    let mut seen_paths = std::collections::HashSet::new();
    for (i, (path, mtime, size)) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            bail!("indexation interrompue");
        }
        let p = path.display().to_string();
        seen_paths.insert(p.clone());
        progress(i, files.len(), &p);
        let known: Option<(i64, i64, i64)> = db.with(|c| {
            c.query_row(
                "SELECT id, mtime, size FROM index_files WHERE path = ?1",
                [&p],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })?;
        if let Some((_, m, s)) = known
            && m == *mtime
            && s == *size
        {
            stats.unchanged += 1;
            continue;
        }
        if let Some((id, ..)) = known {
            vectors::delete(db, CHUNKS, &chunk_ids(db, "f.id = ?1", id)?)?;
        }
        match claudio_docs::extract(path) {
            Ok(doc) => {
                let chunks = claudio_docs::chunk_document(&doc);
                db.with(|c| {
                    let tx = c.unchecked_transaction()?;
                    tx.execute(
                        "INSERT INTO index_files (source_id, path, mtime, size, kind, chunks, error, indexed_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)
                         ON CONFLICT(path) DO UPDATE SET mtime = ?3, size = ?4, kind = ?5, chunks = ?6,
                            error = NULL, indexed_at = ?7",
                        params![source_id, p, mtime, size, doc.kind, chunks.len() as i64, Utc::now()],
                    )?;
                    let file_id: i64 = tx.query_row("SELECT id FROM index_files WHERE path = ?1", [&p], |r| r.get(0))?;
                    tx.execute("DELETE FROM chunks WHERE file_id = ?1", [file_id])?;
                    for (n, ch) in chunks.iter().enumerate() {
                        tx.execute(
                            "INSERT INTO chunks (file_id, ord, location, text) VALUES (?1, ?2, ?3, ?4)",
                            params![file_id, n as i64, ch.location, ch.text],
                        )?;
                    }
                    tx.commit()
                })?;
                stats.indexed += 1;
            }
            Err(e) => {
                // Le fichier est noté en échec pour ne pas réessayer tant qu'il ne change pas.
                let msg = format!("{e:#}");
                db.with(|c| {
                    c.execute(
                        "INSERT INTO index_files (source_id, path, mtime, size, chunks, error, indexed_at)
                         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)
                         ON CONFLICT(path) DO UPDATE SET mtime = ?3, size = ?4, chunks = 0, error = ?5, indexed_at = ?6",
                        params![source_id, p, mtime, size, msg, Utc::now()],
                    )?;
                    c.execute(
                        "DELETE FROM chunks WHERE file_id = (SELECT id FROM index_files WHERE path = ?1)",
                        [&p],
                    )
                })?;
                stats.failed += 1;
            }
        }
    }
    // Fichiers disparus.
    let known: Vec<(i64, String)> = db.with(|c| {
        c.prepare("SELECT id, path FROM index_files WHERE source_id = ?1")?
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    })?;
    for (id, path) in known.into_iter().filter(|(_, p)| !seen_paths.contains(p)) {
        vectors::delete(db, CHUNKS, &chunk_ids(db, "f.id = ?1", id)?)?;
        db.with(|c| c.execute("DELETE FROM index_files WHERE id = ?1", [id]))?;
        tracing::debug!("retiré de l'index : {path}");
        stats.removed += 1;
    }
    Ok(stats)
}

/// Calcule les embeddings manquants ; renvoie le nombre de passages traités.
pub async fn embed_pending(
    db: &Database,
    embedder: &dyn Embedder,
    cancel: &AtomicBool,
) -> Result<usize> {
    vectors::ensure_vectors(db, &embedder.model(), embedder.dims())?;
    let mut done = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let batch: Vec<(i64, String)> = db.with(|c| {
            c.prepare("SELECT id, text FROM chunks WHERE embedded = 0 ORDER BY id LIMIT ?1")?
                .query_map([EMBED_BATCH as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })?;
        if batch.is_empty() {
            break;
        }
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let vecs = embedder.embed(&texts, false).await?;
        for ((id, _), v) in batch.iter().zip(vecs) {
            vectors::insert(db, CHUNKS, *id, &v)?;
            db.with(|c| c.execute("UPDATE chunks SET embedded = 1 WHERE id = ?1", [id]))?;
        }
        done += batch.len();
    }
    Ok(done)
}

pub fn pending_embeddings(db: &Database) -> Result<i64> {
    db.with(|c| {
        c.query_row("SELECT COUNT(*) FROM chunks WHERE embedded = 0", [], |r| {
            r.get(0)
        })
    })
}

/// Recherche hybride dans les documents indexés.
pub async fn search(
    db: &Database,
    embedder: Option<&dyn Embedder>,
    query: &str,
    k: usize,
) -> Result<Vec<Hit>> {
    let pool = (k * 4).max(20);
    let mut lists = Vec::new();
    if let Some(q) = fts_query(query) {
        lists.push(db.with(|c| {
            c.prepare("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts) LIMIT ?2")?
                .query_map(params![q, pool as i64], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()
        })?);
    }
    if let Some(e) = embedder
        && vectors::table_exists(db, CHUNKS)?
    {
        let qv = e.embed(&[query.to_owned()], true).await?;
        if let Some(v) = qv.first() {
            lists.push(
                vectors::knn(db, CHUNKS, v, pool)?
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect(),
            );
        }
    }
    let ranked = rrf(&lists);
    let mut hits = Vec::new();
    for (id, score) in ranked.into_iter().take(k) {
        let row: Option<(String, Option<String>, String)> = db.with(|c| {
            c.query_row(
                "SELECT f.path, c.location, c.text FROM chunks c JOIN index_files f ON f.id = c.file_id WHERE c.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })?;
        if let Some((path, location, text)) = row {
            hits.push(Hit {
                path,
                location,
                text,
                score,
            });
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Hashing;

    fn setup() -> (tempfile::TempDir, Database, i64) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("docs");
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(
            root.join("loyer.txt"),
            "Le loyer de l'appartement est de 900 euros par mois.",
        )
        .unwrap();
        std::fs::write(
            root.join("voiture.md"),
            "# Voiture\n\nLa révision de la Clio est prévue en mars.",
        )
        .unwrap();
        std::fs::write(root.join("photo.jpg"), [0u8; 10]).unwrap();
        std::fs::write(root.join("node_modules").join("x.txt"), "ignoré").unwrap();
        std::fs::write(root.join("casse.pdf"), b"%PDF pas un vrai pdf").unwrap();
        let db = Database::open_in_memory().unwrap();
        let id = add_source(&db, &root).unwrap();
        (dir, db, id)
    }

    #[tokio::test]
    async fn incremental_scan_and_hybrid_search() {
        let (dir, db, id) = setup();
        let stop = AtomicBool::new(false);
        let s = scan(&db, id, &stop, |_, _, _| {}).unwrap();
        assert_eq!(
            (s.seen, s.indexed, s.failed),
            (3, 2, 1),
            "jpg et node_modules ignorés, PDF cassé noté"
        );

        // Sans embeddings : plein texte seulement (accents ignorés).
        let hits = search(&db, None, "revision clio", 3).await.unwrap();
        assert!(hits[0].path.ends_with("voiture.md"));

        // Avec embeddings.
        assert_eq!(embed_pending(&db, &Hashing, &stop).await.unwrap(), 2);
        assert_eq!(pending_embeddings(&db).unwrap(), 0);
        let hits = search(&db, Some(&Hashing), "combien coûte le loyer ?", 2)
            .await
            .unwrap();
        assert!(hits[0].path.ends_with("loyer.txt"));
        assert!(hits[0].text.contains("900"));

        // Deuxième passage : rien n'a changé.
        let s = scan(&db, id, &stop, |_, _, _| {}).unwrap();
        assert_eq!(
            (s.indexed, s.unchanged),
            (0, 3),
            "le PDF en échec n'est pas retenté"
        );

        // Modification et suppression.
        let root = dir.path().join("docs");
        std::fs::remove_file(root.join("voiture.md")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(
            root.join("loyer.txt"),
            "Le loyer passe à 950 euros en janvier.",
        )
        .unwrap();
        let s = scan(&db, id, &stop, |_, _, _| {}).unwrap();
        assert_eq!((s.indexed, s.removed), (1, 1));
        embed_pending(&db, &Hashing, &stop).await.unwrap();
        let hits = search(&db, Some(&Hashing), "loyer", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("950"));

        let v = &sources(&db).unwrap()[0];
        assert_eq!(
            (v.files, v.failed_files, v.chunks, v.embedded_chunks),
            (2, 1, 1, 1)
        );
        remove_source(&db, id).unwrap();
        assert!(
            search(&db, Some(&Hashing), "loyer", 5)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
