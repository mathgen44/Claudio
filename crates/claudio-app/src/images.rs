//! Images jointes aux conversations : réduites, réencodées et rangées dans le
//! dossier des données de Claudio, sous un nom tiré de leur contenu.
//!
//! À côté de chaque image, un fichier `.txt` garde son texte reconnu (OCR),
//! transmis à la place de l'image aux modèles qui ne voient pas.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use claudio_llm::chat::ImagePart;
use image::{DynamicImage, ImageFormat, ImageReader};
use sha2::{Digest, Sha256};

/// Plus grand côté conservé : au-delà, les modèles réduisent eux-mêmes l'image
/// (1568 px est la limite conseillée par Anthropic, suffisante pour les autres).
pub const MAX_SIDE: u32 = 1568;
/// Taille maximale d'une image déposée (avant réduction).
const MAX_INPUT_BYTES: usize = 40 << 20;

/// Nom d'une image rangée : empreinte hexadécimale et extension.
fn valid_id(id: &str) -> Result<()> {
    let ok = id.split_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && stem.chars().all(|c| c.is_ascii_hexdigit())
            && matches!(ext, "jpg" | "png")
    });
    if !ok {
        bail!("nom d'image invalide : {id:?}");
    }
    Ok(())
}

fn media_type(id: &str) -> &'static str {
    if id.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    }
}

/// Réduit l'image et la réencode : JPEG, ou PNG si elle a de la transparence.
fn normalize(bytes: &[u8]) -> Result<(Vec<u8>, &'static str)> {
    if bytes.len() > MAX_INPUT_BYTES {
        bail!("image trop lourde (40 Mo au plus)");
    }
    let img = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()
        .context("image illisible ou format non pris en charge")?;
    let img = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let mut out = Vec::new();
    if img.color().has_alpha() {
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)?;
        Ok((out, "png"))
    } else {
        let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88);
        rgb.write_with_encoder(enc)?;
        Ok((out, "jpg"))
    }
}

/// Range une image et renvoie son nom. La même image donne toujours le même nom.
pub fn store(dir: &Path, bytes: &[u8]) -> Result<String> {
    let (data, ext) = normalize(bytes)?;
    let hash = hex::encode(Sha256::digest(&data));
    let id = format!("{}.{ext}", &hash[..24]);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(&id);
    if !path.is_file() {
        let tmp = dir.join(format!("{id}.tmp"));
        std::fs::write(&tmp, &data)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(id)
}

pub fn path_of(dir: &Path, id: &str) -> Result<PathBuf> {
    valid_id(id)?;
    Ok(dir.join(id))
}

fn text_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.txt"))
}

/// Texte reconnu dans l'image ; enregistré une fois pour toutes.
pub fn set_text(dir: &Path, id: &str, text: &str) -> Result<()> {
    valid_id(id)?;
    std::fs::write(text_path(dir, id), text)?;
    Ok(())
}

/// Image prête à être envoyée à un modèle.
pub fn load(dir: &Path, id: &str, name: &str) -> Result<ImagePart> {
    let bytes =
        std::fs::read(path_of(dir, id)?).with_context(|| format!("image {id} introuvable"))?;
    Ok(ImagePart {
        name: name.to_owned(),
        media_type: media_type(id).to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        text: std::fs::read_to_string(text_path(dir, id)).unwrap_or_default(),
    })
}

/// Adresse `data:` de l'image, pour l'afficher dans l'interface.
pub fn data_url(dir: &Path, id: &str) -> Result<String> {
    Ok(load(dir, id, "")?.data_url())
}

/// Supprime les images qui ne sont plus jointes à aucun message.
pub fn prune(dir: &Path, keep: &std::collections::HashSet<String>) -> Result<usize> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(0);
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let id = name.strip_suffix(".txt").unwrap_or(&name);
        if valid_id(id).is_ok() && !keep.contains(id) {
            std::fs::remove_file(e.path())?;
            if name == id {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let img = if alpha {
            DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                w,
                h,
                image::Rgba([10, 20, 30, 128]),
            ))
        } else {
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([200, 10, 10])))
        };
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn stores_reduces_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let id = store(dir.path(), &png(4000, 1000, false)).unwrap();
        assert!(id.ends_with(".jpg"));
        assert_eq!(
            store(dir.path(), &png(4000, 1000, false)).unwrap(),
            id,
            "même contenu, même nom"
        );
        let img = image::open(dir.path().join(&id)).unwrap();
        assert_eq!((img.width(), img.height()), (MAX_SIDE, 392));

        set_text(dir.path(), &id, "TOTAL 42 €").unwrap();
        let part = load(dir.path(), &id, "facture.png").unwrap();
        assert_eq!(part.media_type, "image/jpeg");
        assert_eq!(part.text, "TOTAL 42 €");
        assert_eq!(part.name, "facture.png");
        assert!(
            data_url(dir.path(), &id)
                .unwrap()
                .starts_with("data:image/jpeg;base64,/9j/")
        );

        let transparent = store(dir.path(), &png(10, 10, true)).unwrap();
        assert!(transparent.ends_with(".png"));
    }

    #[test]
    fn rejects_bad_names_and_data() {
        let dir = tempfile::tempdir().unwrap();
        assert!(path_of(dir.path(), "../config.toml").is_err());
        assert!(path_of(dir.path(), "abc.exe").is_err());
        assert!(path_of(dir.path(), "abc.jpg").is_ok());
        assert!(store(dir.path(), b"pas une image").is_err());
    }

    #[test]
    fn prunes_unreferenced_images() {
        let dir = tempfile::tempdir().unwrap();
        let a = store(dir.path(), &png(8, 8, false)).unwrap();
        let b = store(dir.path(), &png(9, 9, false)).unwrap();
        set_text(dir.path(), &b, "x").unwrap();
        let keep = [a.clone()].into_iter().collect();
        assert_eq!(prune(dir.path(), &keep).unwrap(), 1);
        assert!(dir.path().join(&a).exists());
        assert!(!dir.path().join(&b).exists());
        assert!(!dir.path().join(format!("{b}.txt")).exists());
    }
}
