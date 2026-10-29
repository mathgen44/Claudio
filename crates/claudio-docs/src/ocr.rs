//! Reconnaissance de caractères par le moteur intégré à Windows
//! (`Windows.Media.Ocr`) : images, et pages de PDF rendues par `Windows.Data.Pdf`.
//! Tout se passe sur la machine, sans téléchargement.

use std::path::Path;

use anyhow::{Context, Result};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform,
    ColorManagementMode, ExifOrientationMode,
};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::InMemoryRandomAccessStream;
use windows::Storage::{FileAccessMode, StorageFile};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::HSTRING;

/// Pages de PDF reconnues au plus par document (environ 0,5 s par page).
pub const MAX_OCR_PAGES: u32 = 300;
/// Résolution de rendu des pages de PDF.
const PDF_DPI: f32 = 300.0;

fn engine() -> Result<OcrEngine> {
    // Les fils de tokio ne sont pas initialisés pour WinRT ; un second appel est sans effet.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    if let Ok(e) = OcrEngine::TryCreateFromUserProfileLanguages() {
        return Ok(e);
    }
    for tag in ["fr-FR", "en-US"] {
        let lang = Language::CreateLanguage(&HSTRING::from(tag))?;
        if OcrEngine::IsLanguageSupported(&lang).unwrap_or(false)
            && let Ok(e) = OcrEngine::TryCreateFromLanguage(&lang)
        {
            return Ok(e);
        }
    }
    anyhow::bail!(
        "reconnaissance de caractères indisponible : ajoutez une langue avec la reconnaissance optique dans les paramètres de Windows"
    )
}

fn storage_file(path: &Path) -> Result<StorageFile> {
    let abs = std::path::absolute(path)?;
    Ok(StorageFile::GetFileFromPathAsync(&HSTRING::from(abs.as_os_str()))?.join()?)
}

/// Texte de l'image décodée, réduite si elle dépasse la taille admise par le moteur.
fn recognize(engine: &OcrEngine, decoder: &BitmapDecoder) -> Result<String> {
    let max = OcrEngine::MaxImageDimension()? as f64;
    let (w, h) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    let transform = BitmapTransform::new()?;
    let scale = max / w.max(h).max(1) as f64;
    if scale < 1.0 {
        transform.SetScaledWidth(((w as f64 * scale) as u32).max(1))?;
        transform.SetScaledHeight(((h as f64 * scale) as u32).max(1))?;
        transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    }
    let bitmap = decoder
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let lines: Vec<String> = result
        .Lines()?
        .into_iter()
        .filter_map(|l| l.Text().ok())
        .map(|t| t.to_string_lossy())
        .collect();
    Ok(lines.join("\n"))
}

/// Texte d'une image (PNG, JPEG, TIFF…).
pub fn image(path: &Path) -> Result<String> {
    let engine = engine()?;
    let stream = storage_file(path)?
        .OpenAsync(FileAccessMode::Read)?
        .join()?;
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .context("image illisible")?
        .join()
        .context("image illisible")?;
    recognize(&engine, &decoder)
}

/// Texte des pages de PDF retenues par `wanted` (numéros à partir de 0),
/// dans la limite de `MAX_OCR_PAGES`.
pub fn pdf_pages(path: &Path, wanted: impl Fn(u32) -> bool) -> Result<Vec<(u32, String)>> {
    let engine = engine()?;
    let doc = PdfDocument::LoadFromFileAsync(&storage_file(path)?)?
        .join()
        .context("PDF illisible (protégé par mot de passe ?)")?;
    let max = OcrEngine::MaxImageDimension()? as f32;
    let mut out = Vec::new();
    for i in (0..doc.PageCount()?)
        .filter(|&i| wanted(i))
        .take(MAX_OCR_PAGES as usize)
    {
        let page = doc.GetPage(i)?;
        // Taille en pixels logiques (1/96 de pouce), rendue à 300 points par pouce.
        let size = page.Size()?;
        let scale = (PDF_DPI / 96.0).min(max / size.Width.max(size.Height).max(1.0));
        let options = PdfPageRenderOptions::new()?;
        options.SetDestinationWidth(((size.Width * scale) as u32).max(1))?;
        options.SetDestinationHeight(((size.Height * scale) as u32).max(1))?;
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)?
            .join()?;
        stream.Seek(0)?;
        let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
        let text = recognize(&engine, &decoder)?;
        tracing::debug!(page = i + 1, chars = text.len(), "page reconnue");
        out.push((i, text));
    }
    Ok(out)
}
