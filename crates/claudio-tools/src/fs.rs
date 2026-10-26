//! Outils fichiers : lister, lire, chercher, écrire, créer, déplacer, mettre à la corbeille.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use claudio_perms::Action;
use serde_json::{Value, json};

use crate::{Context, Done, ToolOutput, ToolSpec, arg_path, untrusted};

const MAX_READ_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_LINES: usize = 400;
const MAX_LIST: usize = 500;
const MAX_MATCHES: usize = 100;

pub fn specs() -> Vec<ToolSpec> {
    let spec = |name: &str, description: &str, parameters: Value| ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters,
    };
    vec![
        spec(
            "list_directory",
            "Liste le contenu d'un dossier (nom, type, taille, date).",
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Chemin complet du dossier"},
                "recursive": {"type": "boolean", "description": "Inclure les sous-dossiers (500 entrées max)"}
            }, "required": ["path"]}),
        ),
        spec(
            "read_file",
            "Lit un fichier : texte, PDF, Word, Excel, PowerPoint, OpenDocument. Pour les gros fichiers, lire par tranches avec offset et limit (en lignes).",
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Chemin complet du fichier"},
                "offset": {"type": "integer", "description": "Première ligne à lire (0 par défaut)"},
                "limit": {"type": "integer", "description": "Nombre de lignes (400 par défaut)"}
            }, "required": ["path"]}),
        ),
        spec(
            "search_files",
            "Cherche des fichiers dans un dossier et ses sous-dossiers, par nom (motif comme *.pdf) et/ou par texte contenu.",
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Dossier de départ"},
                "name_pattern": {"type": "string", "description": "Motif de nom, ex. *.txt ou facture*"},
                "text": {"type": "string", "description": "Texte à trouver dans les fichiers (insensible à la casse)"}
            }, "required": ["path"]}),
        ),
        spec(
            "write_file",
            "Écrit un fichier texte. mode : create (par défaut, échoue s'il existe), overwrite (remplace, une sauvegarde est gardée) ou append.",
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"},
                "mode": {"type": "string", "enum": ["create", "overwrite", "append"]}
            }, "required": ["path", "content"]}),
        ),
        spec(
            "create_directory",
            "Crée un dossier (et ses parents).",
            json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        ),
        spec(
            "move_path",
            "Déplace ou renomme un fichier ou un dossier. Échoue si la destination existe.",
            json!({"type": "object", "properties": {
                "source": {"type": "string"},
                "destination": {"type": "string"}
            }, "required": ["source", "destination"]}),
        ),
        spec(
            "delete_path",
            "Envoie un fichier ou un dossier à la corbeille de Windows (récupérable). L'utilisateur confirme toujours.",
            json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        ),
    ]
}

pub(crate) async fn call(ctx: &Context<'_>, name: &str, args: &Value) -> Option<Result<Done>> {
    Some(match name {
        "list_directory" => list(ctx, args).await,
        "read_file" => read(ctx, args).await,
        "search_files" => search(ctx, args).await,
        "write_file" => write(ctx, args).await,
        "create_directory" => mkdir(ctx, args).await,
        "move_path" => move_path(ctx, args).await,
        "delete_path" => delete(ctx, args).await,
        _ => return None,
    })
}

fn done(output: ToolOutput, detail: impl Into<String>, decision: String) -> Result<Done> {
    Ok(Done {
        output,
        detail: detail.into(),
        decision,
    })
}

fn describe(entry: &std::fs::DirEntry, base: &Path) -> String {
    let meta = entry.metadata().ok();
    let rel = entry
        .path()
        .strip_prefix(base)
        .unwrap_or(&entry.path())
        .display()
        .to_string();
    let date = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    match meta {
        Some(m) if m.is_dir() => format!("[dossier] {rel}\\  {date}"),
        Some(m) => format!("{rel}  {}  {date}", human_size(m.len())),
        None => rel,
    }
}

fn human_size(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} Go", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} Mo", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} Ko", b as f64 / 1024.0),
        b => format!("{b} o"),
    }
}

async fn list(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let path = arg_path(args, "path")?;
    let decision = ctx
        .authorize("list_directory", Action::Read(path.clone()))
        .await?;
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let base = path.clone();
    let lines = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        if !base.is_dir() {
            bail!("dossier introuvable : {}", base.display());
        }
        let mut out = Vec::new();
        if recursive {
            for e in walkdir::WalkDir::new(&base)
                .min_depth(1)
                .max_depth(8)
                .into_iter()
                .flatten()
            {
                if out.len() >= MAX_LIST {
                    out.push(format!("… (limité à {MAX_LIST} entrées)"));
                    break;
                }
                let rel = e
                    .path()
                    .strip_prefix(&base)
                    .unwrap_or(e.path())
                    .display()
                    .to_string();
                out.push(if e.file_type().is_dir() {
                    format!("[dossier] {rel}\\")
                } else {
                    rel
                });
            }
        } else {
            let mut entries: Vec<_> = std::fs::read_dir(&base)?
                .flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with(".claudio-"))
                .collect();
            entries.sort_by_key(|e| (!e.path().is_dir(), e.file_name()));
            for e in entries.iter().take(MAX_LIST) {
                out.push(describe(e, &base));
            }
            if entries.len() > MAX_LIST {
                out.push(format!("… et {} autres", entries.len() - MAX_LIST));
            }
        }
        Ok(out)
    })
    .await??;
    let body = if lines.is_empty() {
        "(dossier vide)".to_owned()
    } else {
        lines.join("\n")
    };
    done(ToolOutput::ok(body), path.display().to_string(), decision)
}

/// Lit un fichier texte ; refuse les fichiers binaires.
fn read_text(path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("fichier introuvable : {}", path.display()))?;
    if meta.is_dir() {
        bail!(
            "{} est un dossier : utilisez list_directory",
            path.display()
        );
    }
    if meta.len() > MAX_READ_BYTES {
        bail!(
            "fichier trop gros ({}) pour être lu en entier",
            human_size(meta.len())
        );
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    if bytes.iter().take(8000).any(|&b| b == 0) {
        bail!(
            "fichier binaire : lecture non prise en charge (PDF et documents Office arriveront plus tard)"
        );
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn read(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let path = arg_path(args, "path")?;
    let decision = ctx
        .authorize("read_file", Action::Read(path.clone()))
        .await?;
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|l| l.clamp(1, 2000) as usize)
        .unwrap_or(DEFAULT_LINES);
    let p = path.clone();
    // PDF et documents bureautiques : texte extrait ; sinon lecture directe.
    let text = tokio::task::spawn_blocking(move || match claudio_docs::kind_of(&p) {
        Some(kind) if kind != "texte" => claudio_docs::extract(&p).map(|d| d.text()),
        _ => read_text(&p),
    })
    .await??;
    let lines: Vec<&str> = text.lines().collect();
    let end = (offset + limit).min(lines.len());
    let mut body = lines.get(offset..end).unwrap_or(&[]).join("\n");
    if end < lines.len() {
        body.push_str(&format!(
            "\n\n[… lignes {offset} à {end} sur {} : continuer avec offset={end}]",
            lines.len()
        ));
    }
    done(
        ToolOutput::ok(untrusted(&path.display().to_string(), &body)),
        path.display().to_string(),
        decision,
    )
}

async fn search(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let path = arg_path(args, "path")?;
    let pattern = args
        .get("name_pattern")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase);
    if pattern.is_none() && text.is_none() {
        bail!("indiquez name_pattern et/ou text");
    }
    let decision = ctx
        .authorize("search_files", Action::Read(path.clone()))
        .await?;
    let base = path.clone();
    let found = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        let glob = match &pattern {
            Some(p) => Some(
                globset::GlobBuilder::new(p)
                    .case_insensitive(true)
                    .build()
                    .context("motif de nom invalide")?
                    .compile_matcher(),
            ),
            None => None,
        };
        let mut out = Vec::new();
        for e in walkdir::WalkDir::new(&base)
            .max_depth(12)
            .into_iter()
            .flatten()
        {
            if out.len() >= MAX_MATCHES {
                out.push(format!("… (limité à {MAX_MATCHES} résultats)"));
                break;
            }
            if !e.file_type().is_file() {
                continue;
            }
            if let Some(g) = &glob
                && !g.is_match(e.file_name())
            {
                continue;
            }
            let shown = e.path().display().to_string();
            match &text {
                None => out.push(shown),
                Some(t) => {
                    let Ok(content) = read_text(e.path()) else {
                        continue;
                    };
                    if let Some((n, line)) = content
                        .lines()
                        .enumerate()
                        .find(|(_, l)| l.to_lowercase().contains(t))
                    {
                        let excerpt: String = line.trim().chars().take(160).collect();
                        out.push(format!("{shown}:{} : {excerpt}", n + 1));
                    }
                }
            }
        }
        Ok(out)
    })
    .await??;
    let body = if found.is_empty() {
        "Aucun fichier trouvé.".to_owned()
    } else {
        found.join("\n")
    };
    done(
        ToolOutput::ok(untrusted("recherche de fichiers", &body)),
        path.display().to_string(),
        decision,
    )
}

/// Copie de sauvegarde avant remplacement.
fn backup(ctx: &Context<'_>, path: &Path) -> Result<PathBuf> {
    let dir = ctx
        .backups
        .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(path.file_name().unwrap_or_default());
    std::fs::copy(path, &dest)?;
    Ok(dest)
}

async fn write(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let path = arg_path(args, "path")?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .context("paramètre « content » manquant")?;
    let mode = args.get("mode").and_then(Value::as_str).unwrap_or("create");
    let exists = path.exists();
    if path.is_dir() {
        bail!("{} est un dossier", path.display());
    }
    let overwrite = match mode {
        "create" if exists => bail!(
            "{} existe déjà : utilisez mode=overwrite ou append",
            path.display()
        ),
        "create" | "append" => false,
        "overwrite" => exists,
        other => bail!("mode inconnu : {other}"),
    };
    let decision = ctx
        .authorize(
            "write_file",
            Action::Write {
                path: path.clone(),
                overwrite,
            },
        )
        .await?;
    let mut note = String::new();
    if overwrite {
        let b = backup(ctx, &path)?;
        note = format!(" (ancienne version sauvegardée dans {})", b.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if mode == "append" {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(content.as_bytes())?;
    } else {
        std::fs::write(&path, content)?;
    }
    done(
        ToolOutput::ok(format!(
            "{} octets écrits dans {}{note}",
            content.len(),
            path.display()
        )),
        format!("{} ({mode})", path.display()),
        decision,
    )
}

async fn mkdir(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let path = arg_path(args, "path")?;
    let decision = ctx
        .authorize(
            "create_directory",
            Action::Write {
                path: path.join("_"),
                overwrite: false,
            },
        )
        .await?;
    std::fs::create_dir_all(&path)?;
    done(
        ToolOutput::ok(format!("dossier créé : {}", path.display())),
        path.display().to_string(),
        decision,
    )
}

async fn move_path(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let src = arg_path(args, "source")?;
    let dst = arg_path(args, "destination")?;
    if !src.exists() {
        bail!("introuvable : {}", src.display());
    }
    if dst.exists() {
        bail!("la destination existe déjà : {}", dst.display());
    }
    // Déplacer, c'est retirer de la source (comme une suppression) et écrire à la destination.
    let d1 = ctx
        .authorize(
            "move_path",
            Action::Write {
                path: src.clone(),
                overwrite: true,
            },
        )
        .await?;
    let d2 = ctx
        .authorize(
            "move_path",
            Action::Write {
                path: dst.clone(),
                overwrite: false,
            },
        )
        .await?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&src, &dst).context("déplacement impossible (autre disque ?)")?;
    done(
        ToolOutput::ok(format!("{} → {}", src.display(), dst.display())),
        format!("{} → {}", src.display(), dst.display()),
        format!("{d1} / {d2}"),
    )
}

async fn delete(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let path = arg_path(args, "path")?;
    if !path.exists() {
        bail!("introuvable : {}", path.display());
    }
    let decision = ctx
        .authorize("delete_path", Action::Delete(path.clone()))
        .await?;
    let p = path.clone();
    tokio::task::spawn_blocking(move || trash::delete(&p))
        .await?
        .context("envoi à la corbeille impossible")?;
    done(
        ToolOutput::ok(format!("{} envoyé à la corbeille", path.display())),
        path.display().to_string(),
        decision,
    )
}
