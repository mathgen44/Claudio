//! Traitements du texte : mot d'éveil dans une transcription, texte lisible à voix haute.

fn fold(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' => 'i',
        'ô' | 'ö' | 'ó' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ç' => 'c',
        c => c,
    }
}

fn normalize(word: &str) -> String {
    word.to_lowercase()
        .chars()
        .map(fold)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != *cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Cherche le mot d'éveil au début de la transcription (« Claudio », « Dis Claudio »,
/// ou une graphie approchante comme « Clodio »). Renvoie la demande qui suit, vide
/// si l'utilisateur n'a dit que le mot d'éveil.
pub fn wake(transcript: &str, phrase: &str) -> Option<String> {
    let target: Vec<String> = phrase.split_whitespace().map(normalize).collect();
    if target.is_empty() {
        return None;
    }
    let words: Vec<&str> = transcript
        .split(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == '!' || c == '?')
        .filter(|w| !normalize(w).is_empty())
        .collect();
    let close = |w: &str, t: &str| {
        let w = normalize(w);
        let tolerance = if t.chars().count() >= 6 { 2 } else { 1 };
        distance(&w, t) <= tolerance
    };
    // Le mot d'éveil peut être précédé de « dis », « hé », « OK »…
    for start in 0..words.len().min(3) {
        if start + target.len() > words.len() {
            break;
        }
        if target
            .iter()
            .enumerate()
            .all(|(k, t)| close(words[start + k], t))
        {
            let rest = words[start + target.len()..].join(" ");
            return Some(rest.trim().to_owned());
        }
    }
    None
}

/// Texte d'une réponse débarrassé de sa mise en forme Markdown, pour la lecture.
pub fn speakable(markdown: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in markdown.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            if !in_code {
                out.push_str("(Voir le bloc de code à l'écran.)\n");
            }
            in_code = !in_code;
            continue;
        }
        if in_code || t.starts_with('|') {
            continue;
        }
        let t = t.trim_start_matches(['#', '>', '-', '*', '+']).trim_start();
        out.push_str(&links(t));
        out.push('\n');
    }
    let cleaned: String = out
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '~'))
        .collect();
    cleaned
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `[texte](adresse)` → `texte` ; adresses nues → « lien ».
fn links(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](") else {
            break;
        };
        let Some(end) = rest[open + close..].find(')') else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..open + close]);
        rest = &rest[open + close + end + 1..];
    }
    out.push_str(rest);
    out.split(' ')
        .map(|w| {
            if w.starts_with("http://") || w.starts_with("https://") {
                "lien"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wake_word() {
        assert_eq!(
            wake("Claudio, quelle est la météo ?", "Claudio").as_deref(),
            Some("quelle est la météo")
        );
        assert_eq!(wake("Dis Clodio.", "Claudio").as_deref(), Some(""));
        assert_eq!(
            wake("OK, cloudio mets de la musique", "Claudio").as_deref(),
            Some("mets de la musique")
        );
        assert_eq!(wake("Je parlais à Claudia hier soir", "Claudio"), None);
        assert_eq!(wake("Bonjour tout le monde", "Claudio"), None);
        assert_eq!(
            wake("Hé Jarvis, allume", "Hé Jarvis").as_deref(),
            Some("allume")
        );
    }

    #[test]
    fn strips_markdown_for_speech() {
        let s = speakable(
            "## Titre\n\nVoici **trois** points :\n- un [site](https://x.fr)\n- voir https://y.fr\n\n```rust\nfn main() {}\n```\n| a | b |\nFin.",
        );
        assert_eq!(
            s,
            "Titre\nVoici trois points :\nun site\nvoir lien\n(Voir le bloc de code à l'écran.)\nFin."
        );
    }
}
