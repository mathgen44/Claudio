//! Documents bureautiques (archives ZIP de XML) : Word, PowerPoint, OpenDocument.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::{Document, Section};

fn read_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<String> {
    let mut f = archive
        .by_name(name)
        .with_context(|| format!("{name} absent du document"))?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(s)
}

fn open(path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    zip::ZipArchive::new(std::fs::File::open(path)?).context("document bureautique illisible")
}

/// Texte d'un XML : `text` = balises contenant du texte, `para` = fin de paragraphe,
/// `tab`/`brk` = tabulation et saut de ligne, `space` = espace (OpenDocument).
fn xml_text(xml: &str, text: &[&str], para: &[&str], tab: &str, brk: &str, space: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut inside = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if text.contains(&e.name().into_inner()) {
                    inside += 1;
                }
            }
            Ok(Event::End(e)) => {
                let n = e.name().into_inner();
                if text.contains(&n) {
                    inside = inside.saturating_sub(1);
                }
                if para.contains(&n) {
                    out.push('\n');
                }
            }
            Ok(Event::Empty(e)) => match e.name().into_inner() {
                x if x == tab => out.push('\t'),
                x if x == brk => out.push('\n'),
                x if x == space => out.push(' '),
                _ => {}
            },
            Ok(Event::Text(t)) if inside > 0 => out.push_str(&t.into_inner()),
            Ok(Event::GeneralRef(r)) if inside > 0 => {
                // Entités (&amp;, &#233;…) livrées à part par quick-xml.
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else {
                    out.push_str(match r.into_inner().as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

pub fn docx(path: &Path) -> Result<String> {
    let xml = read_entry(&mut open(path)?, "word/document.xml")?;
    Ok(xml_text(&xml, &["w:t"], &["w:p"], "w:tab", "w:br", "-"))
}

pub fn pptx(path: &Path) -> Result<Document> {
    let mut archive = open(path)?;
    let mut slides: Vec<(u32, String)> = archive
        .file_names()
        .filter_map(|n| n.ok())
        .filter_map(|n| {
            let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?;
            Some((num.parse().ok()?, n.to_string()))
        })
        .collect();
    slides.sort();
    let mut sections = Vec::new();
    for (num, name) in slides {
        let xml = read_entry(&mut archive, &name)?;
        let text = xml_text(&xml, &["a:t"], &["a:p"], "a:tab", "a:br", "-");
        if !text.trim().is_empty() {
            sections.push(Section {
                location: Some(format!("diapositive {num}")),
                text,
            });
        }
    }
    Ok(Document {
        kind: "pptx",
        sections,
    })
}

pub fn opendocument(path: &Path) -> Result<String> {
    let xml = read_entry(&mut open(path)?, "content.xml")?;
    Ok(xml_text(
        &xml,
        &["text:p", "text:h", "text:span"],
        &["text:p", "text:h"],
        "text:tab",
        "text:line-break",
        "text:s",
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn zip_with(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn word_document() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("lettre.docx");
        zip_with(
            &p,
            &[(
                "word/document.xml",
                r#"<w:document><w:body><w:p><w:r><w:t>Bonjour</w:t></w:r><w:r><w:tab/><w:t xml:space="preserve">Hervé &amp; Cie</w:t></w:r></w:p><w:p><w:r><w:t>Ligne 2</w:t></w:r></w:p></w:body></w:document>"#,
            )],
        );
        assert_eq!(
            crate::extract(&p).unwrap().text(),
            "Bonjour\tHervé & Cie\nLigne 2\n"
        );
    }

    #[test]
    fn slides_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pres.pptx");
        let slide = |t: &str| format!(r#"<p:sld><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:sld>"#);
        zip_with(
            &p,
            &[
                ("ppt/slides/slide10.xml", &slide("Dix")),
                ("ppt/slides/slide2.xml", &slide("Deux")),
            ],
        );
        let d = crate::extract(&p).unwrap();
        assert_eq!(d.sections[0].location.as_deref(), Some("diapositive 2"));
        assert_eq!(d.sections[1].text.trim(), "Dix");
    }

    #[test]
    fn opendocument_text() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("note.odt");
        zip_with(
            &p,
            &[(
                "content.xml",
                r#"<office:document-content><office:body><text:h>Titre</text:h><text:p>Un<text:s/>texte <text:span>riche</text:span></text:p></office:body></office:document-content>"#,
            )],
        );
        assert_eq!(
            crate::extract(&p).unwrap().text(),
            "Titre\nUn texte riche\n"
        );
    }
}
