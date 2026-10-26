//! Outils web : lire une page, faire une recherche.
//!
//! Chaque adresse (et chaque redirection) passe par le moteur de permissions ; les
//! adresses du réseau local sont refusées par défaut pour qu'un contenu malveillant
//! ne puisse pas faire sonder la box, le NAS, etc.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use claudio_perms::Action;
use futures_util::StreamExt;
use reqwest::header;
use serde_json::{Value, json};
use url::Url;

use crate::{Context, Done, ToolOutput, ToolSpec, arg_str, untrusted};

const MAX_BYTES: usize = 5 * 1024 * 1024;
const PAGE_CHARS: usize = 20_000;
const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Claudio/0.1";

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "web_fetch".into(),
            description: "Télécharge une page web et renvoie son texte (pages longues : continuer avec offset)."
                .into(),
            parameters: json!({"type": "object", "properties": {
                "url": {"type": "string", "description": "Adresse complète (http ou https)"},
                "offset": {"type": "integer", "description": "Position (en caractères) où reprendre la lecture"}
            }, "required": ["url"]}),
        },
        ToolSpec {
            name: "web_search".into(),
            description: "Recherche sur le web (DuckDuckGo) ; renvoie titres, adresses et extraits.".into(),
            parameters: json!({"type": "object", "properties": {
                "query": {"type": "string"}
            }, "required": ["query"]}),
        },
    ]
}

pub(crate) async fn call(ctx: &Context<'_>, name: &str, args: &Value) -> Option<Result<Done>> {
    Some(match name {
        "web_fetch" => fetch_tool(ctx, args).await,
        "web_search" => search_tool(ctx, args).await,
        _ => return None,
    })
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || v.octets()[0] == 100 && (v.octets()[1] & 0xC0) == 64 // 100.64.0.0/10
        }
        IpAddr::V6(v) => {
            v.is_loopback()
                || v.is_unspecified()
                || (v.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
                || (v.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
                || v.to_ipv4_mapped().is_some_and(|m| is_private(IpAddr::V4(m)))
        }
    }
}

async fn check_destination(ctx: &Context<'_>, url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("seules les adresses http et https sont acceptées");
    }
    if ctx.engine.policy().allow_private_network {
        return Ok(());
    }
    let host = url.host_str().context("adresse sans hôte")?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
        .await
        .with_context(|| format!("hôte introuvable : {host}"))?;
    for a in addrs {
        if is_private(a.ip()) {
            bail!(
                "{host} est une adresse du réseau local : refusé (réglable dans Outils et accès)"
            );
        }
    }
    Ok(())
}

/// Télécharge une adresse en autorisant chaque redirection ; renvoie l'adresse
/// finale, le type de contenu, le corps et les décisions prises.
async fn get(
    ctx: &Context<'_>,
    tool: &str,
    start: Url,
) -> Result<(Url, String, Vec<u8>, Vec<String>)> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut url = start;
    let mut decisions = Vec::new();
    for _ in 0..=MAX_REDIRECTS {
        decisions.push(
            ctx.authorize(tool, Action::Fetch { url: url.clone() })
                .await?,
        );
        check_destination(ctx, &url).await?;
        let resp = client
            .get(url.clone())
            .send()
            .await
            .context("site injoignable")?;
        if resp.status().is_redirection() {
            let loc = resp
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .context("redirection sans destination")?;
            url = url.join(loc)?;
            continue;
        }
        if !resp.status().is_success() {
            bail!("le site a répondu {}", resp.status());
        }
        let ctype = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut body = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk?);
            if body.len() > MAX_BYTES {
                body.truncate(MAX_BYTES);
                break;
            }
        }
        return Ok((url, ctype, body, decisions));
    }
    bail!("trop de redirections")
}

fn to_text(ctype: &str, body: &[u8]) -> Result<String> {
    if ctype.contains("html") || ctype.is_empty() && body.starts_with(b"<") {
        Ok(html2text::from_read(body, 100).unwrap_or_default())
    } else if ctype.starts_with("text/") || ctype.contains("json") || ctype.contains("xml") {
        Ok(String::from_utf8_lossy(body).into_owned())
    } else {
        bail!("contenu de type « {ctype} » non pris en charge")
    }
}

async fn fetch_tool(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let url: Url = arg_str(args, "url")?
        .trim()
        .parse()
        .context("adresse invalide")?;
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let (final_url, ctype, body, decisions) = get(ctx, "web_fetch", url.clone()).await?;
    let text = to_text(&ctype, &body)?;
    let chars: Vec<char> = text.chars().collect();
    let end = (offset + PAGE_CHARS).min(chars.len());
    let mut page: String = chars.get(offset..end).unwrap_or(&[]).iter().collect();
    if end < chars.len() {
        page.push_str(&format!(
            "\n\n[… caractères {offset} à {end} sur {} : continuer avec offset={end}]",
            chars.len()
        ));
    }
    Ok(Done {
        output: ToolOutput::ok(untrusted(final_url.as_str(), &page)),
        detail: url.to_string(),
        decision: decisions.join(" / "),
    })
}

/// Extrait les résultats de la page HTML de DuckDuckGo.
fn parse_ddg(html: &str) -> Vec<(String, String, String)> {
    let doc = scraper::Html::parse_document(html);
    let result = scraper::Selector::parse(".result").unwrap();
    let link = scraper::Selector::parse("a.result__a").unwrap();
    let snippet = scraper::Selector::parse(".result__snippet").unwrap();
    doc.select(&result)
        .filter_map(|r| {
            let a = r.select(&link).next()?;
            let title = a.text().collect::<String>().trim().to_owned();
            let href = a.value().attr("href")?;
            // Les liens passent par une redirection : l'adresse réelle est dans `uddg`.
            let real = Url::parse(&format!("https:{}", href.trim_start_matches("https:")))
                .ok()
                .and_then(|u| {
                    u.query_pairs()
                        .find(|(k, _)| k == "uddg")
                        .map(|(_, v)| v.into_owned())
                })
                .unwrap_or_else(|| href.to_owned());
            let snip = r
                .select(&snippet)
                .next()
                .map(|s| s.text().collect::<String>().trim().to_owned())
                .unwrap_or_default();
            Some((title, real, snip))
        })
        .take(10)
        .collect()
}

async fn search_tool(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let query = arg_str(args, "query")?;
    let url = Url::parse_with_params("https://html.duckduckgo.com/html/", &[("q", query)])?;
    let (_, _, body, decisions) = get(ctx, "web_search", url).await?;
    let results = parse_ddg(&String::from_utf8_lossy(&body));
    let text = if results.is_empty() {
        "Aucun résultat (ou la recherche a été bloquée).".to_owned()
    } else {
        results
            .iter()
            .enumerate()
            .map(|(i, (t, u, s))| format!("{}. {t}\n   {u}\n   {s}", i + 1))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Done {
        output: ToolOutput::ok(untrusted("recherche web", &text)),
        detail: query.to_owned(),
        decision: decisions.join(" / "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "169.254.0.1",
            "100.64.1.1",
            "::1",
            "fe80::1",
            "fd00::1",
        ] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn parses_duckduckgo() {
        let html = r#"<div class="result"><h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&rut=x">Rust</a></h2>
            <a class="result__snippet">Un langage <b>fiable</b>.</a></div>"#;
        let r = parse_ddg(html);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "Rust");
        assert_eq!(r[0].1, "https://www.rust-lang.org/");
        assert_eq!(r[0].2, "Un langage fiable.");
    }

    #[test]
    fn html_to_text() {
        let t = to_text(
            "text/html",
            b"<html><body><h1>Titre</h1><p>Bonjour <a href='x'>lien</a></p></body></html>",
        )
        .unwrap();
        assert!(t.contains("Titre") && t.contains("Bonjour"));
        assert!(to_text("application/pdf", b"%PDF").is_err());
    }
}
