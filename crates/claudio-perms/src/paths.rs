//! Normalisation des chemins pour les contrôles d'accès.
//!
//! Tout chemin est rendu absolu et canonique (liens et jonctions résolus) avant
//! d'être comparé aux dossiers autorisés, pour qu'un `..` ou un lien ne permette
//! pas de sortir d'un dossier.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Forme canonique d'un chemin, existant ou non (pour un fichier à créer, le
/// dernier dossier existant est résolu puis le reste est ajouté tel quel).
pub fn normalize(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!(
            "chemin relatif refusé : {} (indiquez un chemin complet)",
            path.display()
        );
    }
    if let Ok(p) = dunce::canonicalize(path) {
        return Ok(p);
    }
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .with_context(|| format!("chemin invalide : {}", path.display()))?
            .to_owned();
        rest.push(name);
        if !existing.pop() {
            bail!("chemin invalide : {}", path.display());
        }
    }
    let mut out = dunce::canonicalize(&existing)?;
    for part in rest.into_iter().rev() {
        let p = Path::new(&part);
        if !matches!(p.components().next(), Some(Component::Normal(_)))
            || p.components().count() != 1
        {
            bail!("chemin invalide : {}", path.display());
        }
        out.push(part);
    }
    Ok(out)
}

fn key(c: Component) -> String {
    c.as_os_str().to_string_lossy().to_lowercase()
}

/// `child` est-il `parent` ou un de ses descendants ? (insensible à la casse, comme Windows)
pub fn is_within(child: &Path, parent: &Path) -> bool {
    let mut c = child.components();
    for p in parent.components() {
        match c.next() {
            Some(x) if key(x) == key(p) => {}
            _ => return false,
        }
    }
    true
}

/// Dossier concerné par une demande : le chemin lui-même s'il s'agit d'un dossier,
/// sinon son dossier parent.
pub fn folder_of(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within() {
        assert!(is_within(
            Path::new(r"C:\Docs\a\b.txt"),
            Path::new(r"c:\docs")
        ));
        assert!(is_within(Path::new(r"C:\Docs"), Path::new(r"C:\Docs")));
        assert!(!is_within(Path::new(r"C:\Docs2\x"), Path::new(r"C:\Docs")));
        assert!(!is_within(Path::new(r"C:\"), Path::new(r"C:\Docs")));
    }

    #[test]
    fn normalizes_dotdot_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("a")).unwrap();
        let sneaky = root.join("a").join("..").join("..").join("x.txt");
        let n = normalize(&sneaky).unwrap();
        assert!(!is_within(&n, &root.join("a")));
        let new_file = normalize(&root.join("a").join("nouveau").join("f.txt")).unwrap();
        assert!(is_within(&new_file, &root.join("a")));
        assert!(normalize(Path::new("relatif.txt")).is_err());
    }
}
