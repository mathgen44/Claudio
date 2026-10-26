//! Extraction du texte des documents (texte, PDF, Word, Excel, PowerPoint,
//! OpenDocument, HTML) et découpage en passages pour l'indexation.

mod chunk;
mod office;

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;

pub use chunk::{Chunk, chunk_document};

/// Taille maximale d'un fichier traité.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Texte maximal retenu par document.
const MAX_CHARS: usize = 2_000_000;

/// Partie d'un document (page, feuille, diapositive) avec son texte.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    /// Emplacement lisible : « page 3 », « feuille Budget », « diapositive 2 ».
    pub location: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Document {
    pub kind: &'static str,
    pub sections: Vec<Section>,
}

impl Document {
    pub fn text(&self) -> String {
        self.sections
            .iter()
            .map(|s| match &s.location {
                Some(l) => format!("[{l}]\n{}", s.text),
                None => s.text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn single(kind: &'static str, text: String) -> Self {
        Self {
            kind,
            sections: vec![Section {
                location: None,
                text,
            }],
        }
    }
}

const TEXT_EXT: &[&str] = &[
    "txt", "md", "markdown", "rst", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml", "ini",
    "cfg", "conf", "log", "xml", "rs", "py", "js", "ts", "tsx", "jsx", "java", "kt", "c", "h",
    "cpp", "hpp", "cs", "go", "rb", "php", "swift", "sql", "sh", "ps1", "bat", "css", "scss",
    "vue", "svelte", "tex", "srt", "vtt",
];

/// Type de document reconnu d'après l'extension, ou `None` si non pris en charge.
pub fn kind_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "pdf" => "pdf",
        "docx" | "docm" => "docx",
        "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" => "tableur",
        "pptx" | "pptm" => "pptx",
        "odt" | "odp" => "opendocument",
        "html" | "htm" => "html",
        e if TEXT_EXT.contains(&e) => "texte",
        _ => return None,
    })
}

fn limit(mut s: String) -> String {
    if s.len() > MAX_CHARS {
        let mut cut = MAX_CHARS;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

fn read_text(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    if bytes.iter().take(8000).any(|&b| b == 0) {
        bail!("fichier binaire");
    }
    // UTF-8 de préférence, sinon Windows-1252 (fichiers texte anciens en français).
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s.trim_start_matches('\u{feff}').to_owned(),
        Err(e) => e.into_bytes().iter().map(|&b| cp1252(b)).collect(),
    })
}

/// Caractère Windows-1252 (approximation suffisante : Latin-1 + guillemets et tirets).
fn cp1252(b: u8) -> char {
    match b {
        0x80 => '€',
        0x91 | 0x92 => '\'',
        0x93 | 0x94 => '"',
        0x96 | 0x97 => '-',
        0x85 => '…',
        0x9C => 'œ',
        0x8C => 'Œ',
        b => b as char,
    }
}

fn pdf(path: &Path) -> Result<Document> {
    let p = path.to_path_buf();
    // La bibliothèque PDF peut paniquer sur des fichiers malformés.
    let pages = std::panic::catch_unwind(move || pdf_extract::extract_text_by_pages(&p))
        .map_err(|_| anyhow::anyhow!("PDF illisible"))?
        .context("PDF illisible (protégé ou malformé ?)")?;
    let sections: Vec<Section> = pages
        .into_iter()
        .enumerate()
        .filter(|(_, t)| !t.trim().is_empty())
        .map(|(i, t)| Section {
            location: Some(format!("page {}", i + 1)),
            text: t,
        })
        .collect();
    if sections.is_empty() {
        bail!(
            "PDF sans texte (document scanné ? la reconnaissance de caractères n'est pas encore prise en charge)"
        );
    }
    Ok(Document {
        kind: "pdf",
        sections,
    })
}

fn spreadsheet(path: &Path) -> Result<Document> {
    use calamine::{Reader, open_workbook_auto};
    let mut wb = open_workbook_auto(path).context("classeur illisible")?;
    let mut sections = Vec::new();
    for name in wb.sheet_names().to_vec() {
        let Ok(range) = wb.worksheet_range(&name) else {
            continue;
        };
        let mut text = String::new();
        for row in range.rows().take(10_000) {
            let cells: Vec<String> = row.iter().map(|c| c.to_string()).collect();
            if cells.iter().any(|c| !c.is_empty()) {
                text.push_str(cells.join("\t").trim_end());
                text.push('\n');
            }
        }
        if !text.trim().is_empty() {
            sections.push(Section {
                location: Some(format!("feuille {name}")),
                text,
            });
        }
    }
    Ok(Document {
        kind: "tableur",
        sections,
    })
}

/// Extrait le texte d'un fichier (bloquant).
pub fn extract(path: &Path) -> Result<Document> {
    let meta =
        std::fs::metadata(path).with_context(|| format!("introuvable : {}", path.display()))?;
    if meta.len() > MAX_FILE_BYTES {
        bail!("fichier trop gros ({} Mo)", meta.len() / (1024 * 1024));
    }
    let kind = kind_of(path).context("type de fichier non pris en charge")?;
    let mut doc = match kind {
        "pdf" => pdf(path)?,
        "docx" => Document::single(kind, office::docx(path)?),
        "pptx" => office::pptx(path)?,
        "opendocument" => Document::single(kind, office::opendocument(path)?),
        "tableur" => spreadsheet(path)?,
        "html" => Document::single(
            kind,
            html2text::from_read(read_text(path)?.as_bytes(), 120).unwrap_or_default(),
        ),
        _ => Document::single(kind, read_text(path)?),
    };
    for s in &mut doc.sections {
        s.text = limit(std::mem::take(&mut s.text));
    }
    doc.sections.retain(|s| !s.text.trim().is_empty());
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Path::new("a/Rapport.PDF")), Some("pdf"));
        assert_eq!(kind_of(Path::new("budget.xlsx")), Some("tableur"));
        assert_eq!(kind_of(Path::new("notes.md")), Some("texte"));
        assert_eq!(kind_of(Path::new("photo.jpg")), None);
        assert_eq!(kind_of(Path::new("sans_extension")), None);
    }

    #[test]
    fn text_and_legacy_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let utf8 = dir.path().join("a.txt");
        std::fs::write(&utf8, "\u{feff}Été à Noël").unwrap();
        assert_eq!(extract(&utf8).unwrap().text(), "Été à Noël");
        let latin = dir.path().join("b.txt");
        std::fs::write(&latin, b"Caf\xe9 \x80").unwrap();
        assert_eq!(extract(&latin).unwrap().text(), "Café €");
        let bin = dir.path().join("c.txt");
        std::fs::write(&bin, [0u8, 1, 2]).unwrap();
        assert!(extract(&bin).is_err());
    }
}

#[cfg(test)]
mod pdf_tests {
    /// PDF minimal d'une page (offsets de la table xref calculés).
    fn tiny_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for off in offsets {
            out.extend(format!("{off:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .bytes(),
        );
        out
    }

    #[test]
    fn pdf_pages() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("facture.pdf");
        std::fs::write(&p, tiny_pdf("Montant total 104 euros")).unwrap();
        let d = super::extract(&p).unwrap();
        assert_eq!(d.sections[0].location.as_deref(), Some("page 1"));
        assert!(d.text().contains("Montant total 104 euros"), "{}", d.text());
        std::fs::write(&p, b"%PDF-1.4 n'importe quoi").unwrap();
        assert!(super::extract(&p).is_err());
    }
}
