//! Reconnaissance de caractères par le moteur de Windows (images et PDF scannés).

use std::path::Path;

fn data(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

#[test]
fn reads_text_in_image() {
    let d = claudio_docs::extract(&data("scan.png")).unwrap();
    assert_eq!(d.kind, "image");
    let t = d.text();
    assert!(t.contains("4217") && t.contains("104"), "{t}");
}

#[test]
fn reads_scanned_pdf() {
    let d = claudio_docs::extract(&data("scan.pdf")).unwrap();
    assert_eq!(
        d.sections[0].location.as_deref(),
        Some("page 1 (texte reconnu)")
    );
    let t = d.text();
    assert!(t.contains("Montant total"), "{t}");
}
