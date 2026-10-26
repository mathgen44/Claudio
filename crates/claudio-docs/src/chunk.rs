//! Découpage en passages d'environ `TARGET` caractères, en respectant autant que
//! possible paragraphes et phrases, avec un léger recouvrement.

use serde::Serialize;

use crate::Document;

const TARGET: usize = 1200;
const OVERLAP: usize = 150;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Chunk {
    pub location: Option<String>,
    pub text: String,
}

/// Coupe `s` en morceaux d'au plus `max` caractères, de préférence sur un espace.
fn hard_split(s: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + max).min(chars.len());
        if end < chars.len()
            && let Some(sp) = (start + max / 2..end)
                .rev()
                .find(|&i| chars[i].is_whitespace())
        {
            end = sp;
        }
        out.push(
            chars[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_owned(),
        );
        start = end;
    }
    out.retain(|p| !p.is_empty());
    out
}

fn split_text(text: &str) -> Vec<String> {
    // Unités : paragraphes, eux-mêmes coupés s'ils sont trop longs.
    let units: Vec<String> = text
        .split("\n\n")
        .flat_map(|p| {
            let p = p.trim();
            if p.chars().count() > TARGET {
                hard_split(p, TARGET)
            } else {
                vec![p.to_owned()]
            }
        })
        .filter(|p| !p.is_empty())
        .collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for u in units {
        if !current.is_empty() && current.chars().count() + u.chars().count() + 2 > TARGET {
            // Le passage suivant reprend la fin du précédent pour garder le contexte.
            let tail: String = {
                let c: Vec<char> = current.chars().collect();
                c[c.len().saturating_sub(OVERLAP)..].iter().collect()
            };
            chunks.push(std::mem::take(&mut current));
            current = match tail.find(char::is_whitespace) {
                Some(i) => tail[i..].trim_start().to_owned(),
                None => String::new(),
            };
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(&u);
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

pub fn chunk_document(doc: &Document) -> Vec<Chunk> {
    doc.sections
        .iter()
        .flat_map(|s| {
            split_text(&s.text).into_iter().map(|text| Chunk {
                location: s.location.clone(),
                text,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Section;

    fn doc(text: &str) -> Document {
        Document {
            kind: "texte",
            sections: vec![Section {
                location: Some("page 1".into()),
                text: text.into(),
            }],
        }
    }

    #[test]
    fn short_text_is_one_chunk() {
        let c = chunk_document(&doc("Bonjour.\n\nAu revoir."));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].location.as_deref(), Some("page 1"));
    }

    #[test]
    fn long_text_is_split_with_overlap() {
        let para = "mot ".repeat(200); // 800 caractères
        let text = format!("{para}\n\n{para}\n\n{para}");
        let c = chunk_document(&doc(&text));
        assert!(c.len() >= 2);
        assert!(
            c.iter()
                .all(|x| x.text.chars().count() <= TARGET + OVERLAP + 4)
        );
        // Le début du 2e passage reprend la fin du 1er.
        assert!(c[1].text.starts_with("mot"));
        let huge = "a".repeat(5000);
        let c = chunk_document(&doc(&huge));
        assert!(c.len() >= 4);
    }
}
