//! Compétences : procédures réutilisables en Markdown. Proposées par l'assistant
//! après une tâche réussie, elles ne servent qu'une fois validées par l'utilisateur.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use claudio_core::Database;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    /// Identifiant (nom du fichier sans extension).
    pub name: String,
    pub description: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub body: String,
    /// D'où vient la proposition (titre de la tâche).
    pub origin: String,
    pub created_at: DateTime<Utc>,
}

/// Identifiant de fichier : minuscules, chiffres et tirets.
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.trim().to_lowercase().chars() {
        let c = match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_ascii_alphanumeric() => c,
            _ => '-',
        };
        if c != '-' || !s.ends_with('-') {
            s.push(c);
        }
    }
    s.trim_matches('-').chars().take(60).collect()
}

fn parse(name: &str, text: &str) -> Skill {
    let mut description = String::new();
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---")
        && let Some(end) = rest.find("\n---")
    {
        for line in rest[..end].lines() {
            if let Some(d) = line.trim().strip_prefix("description:") {
                description = d.trim().trim_matches('"').to_owned();
            }
        }
        body = rest[end + 4..].trim_start_matches(['\r', '\n']);
    }
    Skill {
        name: name.to_owned(),
        description,
        body: body.to_owned(),
    }
}

pub fn list(dir: &Path) -> Vec<Skill> {
    let mut out: Vec<Skill> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter_map(|p| {
            let name = p.file_stem()?.to_string_lossy().into_owned();
            Some(parse(&name, &std::fs::read_to_string(&p).ok()?))
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn path_of(dir: &Path, name: &str) -> Result<PathBuf> {
    let s = slug(name);
    if s.is_empty() || s != name {
        bail!("nom de compétence invalide : {name}");
    }
    Ok(dir.join(format!("{s}.md")))
}

pub fn get(dir: &Path, name: &str) -> Result<Skill> {
    let p = path_of(dir, name)?;
    let text =
        std::fs::read_to_string(&p).with_context(|| format!("compétence introuvable : {name}"))?;
    Ok(parse(name, &text))
}

/// Écrit (ou remplace) une compétence ; renvoie son identifiant.
pub fn save(dir: &Path, name: &str, description: &str, body: &str) -> Result<String> {
    let s = slug(name);
    if s.is_empty() || body.trim().is_empty() {
        bail!("nom et contenu obligatoires");
    }
    std::fs::create_dir_all(dir)?;
    let desc = description.replace(['\n', '\r'], " ");
    std::fs::write(
        dir.join(format!("{s}.md")),
        format!(
            "---\nname: {s}\ndescription: {}\n---\n\n{}\n",
            desc.trim(),
            body.trim()
        ),
    )?;
    Ok(s)
}

pub fn delete(dir: &Path, name: &str) -> Result<()> {
    std::fs::remove_file(path_of(dir, name)?)?;
    Ok(())
}

pub fn propose(
    db: &Database,
    name: &str,
    description: &str,
    body: &str,
    origin: &str,
) -> Result<i64> {
    db.with(|c| {
        c.execute(
            "INSERT INTO skill_proposals (name, description, body, origin, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![slug(name), description, body, origin, Utc::now()],
        )?;
        Ok(c.last_insert_rowid())
    })
}

pub fn proposals(db: &Database) -> Result<Vec<Proposal>> {
    db.with(|c| {
        c.prepare(
            "SELECT id, name, description, body, origin, created_at FROM skill_proposals
             WHERE status = 'pending' ORDER BY id DESC",
        )?
        .query_map([], |r| {
            Ok(Proposal {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                body: r.get(3)?,
                origin: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?
        .collect()
    })
}

/// Valide une proposition (éventuellement corrigée) : elle devient un fichier.
pub fn accept(
    db: &Database,
    dir: &Path,
    id: i64,
    name: &str,
    description: &str,
    body: &str,
) -> Result<String> {
    let s = save(dir, name, description, body)?;
    db.with(|c| {
        c.execute(
            "UPDATE skill_proposals SET status = 'accepted' WHERE id = ?1",
            [id],
        )
    })?;
    Ok(s)
}

pub fn reject(db: &Database, id: i64) -> Result<()> {
    db.with(|c| {
        c.execute(
            "UPDATE skill_proposals SET status = 'rejected' WHERE id = ?1",
            [id],
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(
            slug("Résumer une facture d'électricité"),
            "resumer-une-facture-d-electricite"
        );
        assert_eq!(slug("  --  "), "");
    }

    #[test]
    fn proposal_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let id = propose(
            &db,
            "Revue de presse",
            "Faire une revue de presse",
            "1. Chercher\n2. Résumer",
            "Tâche X",
        )
        .unwrap();
        assert_eq!(proposals(&db).unwrap().len(), 1);
        let name = accept(
            &db,
            dir.path(),
            id,
            "Revue de presse",
            "Faire une revue de presse",
            "1. Chercher\n2. Résumer",
        )
        .unwrap();
        assert_eq!(name, "revue-de-presse");
        assert!(proposals(&db).unwrap().is_empty());
        let s = get(dir.path(), "revue-de-presse").unwrap();
        assert_eq!(s.description, "Faire une revue de presse");
        assert!(s.body.starts_with("1. Chercher"));
        assert_eq!(list(dir.path()).len(), 1);
        assert!(
            get(dir.path(), "../secret").is_err(),
            "pas de chemin arbitraire"
        );
        delete(dir.path(), "revue-de-presse").unwrap();
        assert!(list(dir.path()).is_empty());
        let id2 = propose(&db, "x", "y", "z", "o").unwrap();
        reject(&db, id2).unwrap();
        assert!(proposals(&db).unwrap().is_empty());
    }
}
