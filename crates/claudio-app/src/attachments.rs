//! Fichiers joints à un message (glisser-déposer) : leur texte est extrait sur
//! place et ajouté au message. Le dépôt vaut accord de lecture pour ce fichier.
//!
//! Les images sont rangées à part (voir `images`) et envoyées telles quelles aux
//! modèles qui les voient ; leur texte reconnu sert aux autres.

use std::path::Path;

use anyhow::{Context, Result, bail};
use claudio_core::audit::AuditRecord;
use serde::Serialize;

use crate::Claudio;

/// Texte maximal retenu par fichier joint (environ 10 000 jetons).
const MAX_ATTACHMENT_CHARS: usize = 40_000;

#[derive(Debug, Clone, Serialize)]
pub struct Attachment {
    pub name: String,
    pub path: String,
    /// Type reconnu : `pdf`, `docx`, `tableur`, `texte`…
    pub kind: &'static str,
    pub text: String,
    /// Texte coupé à `MAX_ATTACHMENT_CHARS`.
    pub truncated: bool,
    /// Image rangée (nom dans le dossier des images), jointe au message comme image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

fn read(path: &Path) -> Result<Attachment> {
    if path.is_dir() {
        bail!("c'est un dossier : ajoutez-le plutôt dans Mémoire › Documents");
    }
    let name = path
        .file_name()
        .context("nom de fichier invalide")?
        .to_string_lossy()
        .into_owned();
    let kind = claudio_docs::kind_of(path)
        .with_context(|| format!("{name} : type de fichier non pris en charge"))?;
    let text = claudio_docs::extract(path)
        .with_context(|| format!("lecture de {name}"))?
        .text();
    if text.trim().is_empty() {
        bail!("{name} : aucun texte lisible");
    }
    let truncated = text.chars().count() > MAX_ATTACHMENT_CHARS;
    let text = if truncated {
        text.chars().take(MAX_ATTACHMENT_CHARS).collect()
    } else {
        text
    };
    Ok(Attachment {
        name,
        path: path.display().to_string(),
        kind,
        text,
        truncated,
        image: None,
    })
}

/// Range une image et note son texte reconnu (OCR), s'il y en a.
fn store_image(dir: &Path, name: String, path: String, bytes: &[u8]) -> Result<Attachment> {
    let id = crate::images::store(dir, bytes).with_context(|| format!("lecture de {name}"))?;
    let stored = crate::images::path_of(dir, &id)?;
    let text = claudio_docs::extract(&stored)
        .map(|d| d.text())
        .unwrap_or_default();
    crate::images::set_text(dir, &id, text.trim())?;
    Ok(Attachment {
        name,
        path,
        kind: "image",
        text: String::new(),
        truncated: false,
        image: Some(id),
    })
}

/// Texte d'un message sans le contenu des fichiers joints (blocs `<fichier …>`
/// composés par l'interface), remplacés par leur seule balise d'ouverture.
pub(crate) fn strip(message: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in message.lines() {
        if !inside && line.starts_with("<fichier nom=\"") && line.ends_with('>') {
            inside = true;
            out.push_str(line);
            out.push('\n');
        } else if inside {
            inside = line != "</fichier>";
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

impl Claudio {
    /// Lit un fichier déposé par l'utilisateur ; la lecture est inscrite au journal.
    pub async fn read_attachment(&self, path: &str) -> Result<Attachment> {
        let p = Path::new(path).to_path_buf();
        let dir = self.paths.images_dir();
        let res = tokio::task::spawn_blocking(move || {
            if claudio_docs::kind_of(&p) == Some("image") {
                let name = p
                    .file_name()
                    .context("nom de fichier invalide")?
                    .to_string_lossy()
                    .into_owned();
                let bytes = std::fs::read(&p).with_context(|| format!("lecture de {name}"))?;
                store_image(&dir, name, p.display().to_string(), &bytes)
            } else {
                read(&p)
            }
        })
        .await?;
        let outcome = match &res {
            Ok(_) => "ok".to_owned(),
            Err(e) => format!("{e:#}"),
        };
        let _ = self.db.audit(&AuditRecord {
            session_id: None,
            tool: "fichier_joint",
            detail: path,
            decision: "déposé par l'utilisateur",
            outcome: &outcome,
        });
        res
    }

    /// Image collée depuis le presse-papiers (ou déposée hors de l'application).
    pub async fn attach_image(&self, name: &str, bytes: Vec<u8>) -> Result<Attachment> {
        let dir = self.paths.images_dir();
        let name = if name.trim().is_empty() {
            "image collée".to_owned()
        } else {
            name.trim().to_owned()
        };
        let path = format!("presse-papiers:{name}");
        tokio::task::spawn_blocking(move || store_image(&dir, name, path, &bytes)).await?
    }

    /// Image rangée, en adresse `data:` pour l'affichage.
    pub fn image_data_url(&self, id: &str) -> Result<String> {
        crate::images::data_url(&self.paths.images_dir(), id)
    }

    /// Supprime une conversation et les images qui n'étaient jointes qu'à elle.
    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.db.delete_session(id)?;
        let keep = self.db.referenced_images()?;
        if let Err(e) = crate::images::prune(&self.paths.images_dir(), &keep) {
            tracing::warn!("ménage des images : {e:#}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_file_contents() {
        let m = "Résume\n\n<fichier nom=\"a.txt\" type=\"texte\">\nsecret\n</fichier>\n\n\
                 <fichier nom=\"b.md\" type=\"texte\">\nx\n</fichier>";
        assert_eq!(
            strip(m),
            "Résume\n\n<fichier nom=\"a.txt\" type=\"texte\">\n\n<fichier nom=\"b.md\" type=\"texte\">"
        );
        assert_eq!(strip("Bonjour"), "Bonjour");
    }

    #[test]
    fn reads_and_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.md");
        std::fs::write(&f, "é".repeat(MAX_ATTACHMENT_CHARS + 10)).unwrap();
        let a = read(&f).unwrap();
        assert_eq!(a.name, "notes.md");
        assert_eq!(a.kind, "texte");
        assert!(a.truncated);
        assert_eq!(a.text.chars().count(), MAX_ATTACHMENT_CHARS);

        assert!(read(dir.path()).is_err());
        let zip = dir.path().join("archive.zip");
        std::fs::write(&zip, [0x50u8, 0x4b]).unwrap();
        assert!(
            read(&zip)
                .unwrap_err()
                .to_string()
                .contains("non pris en charge")
        );
    }
}
