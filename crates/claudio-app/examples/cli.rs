//! Outil de développement pour piloter Claudio sans interface graphique.
//!
//! ```text
//! cargo run -p claudio-app --example cli -- status
//! cargo run -p claudio-app --example cli -- recommend
//! cargo run -p claudio-app --example cli -- runtime
//! cargo run -p claudio-app --example cli -- install qwen3.5-0.8b Q4_K_M
//! cargo run -p claudio-app --example cli -- chat "Bonjour !"
//! ```

use std::io::Write;

use anyhow::{Result, bail};
use claudio_app::Claudio;
use claudio_app::claudio_core::AppPaths;
use claudio_app::claudio_llm::ChatEvent;
use claudio_app::claudio_llm::runtime::InstallEvent;

fn mb(b: u64) -> f64 {
    b as f64 / 1e6
}

#[tokio::main]
async fn main() -> Result<()> {
    let paths = AppPaths::resolve()?;
    paths.ensure()?;
    let _guard = claudio_app::claudio_core::logging::init(&paths.logs_dir());
    let app = Claudio::open(paths)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["status"] => println!("{}", serde_json::to_string_pretty(&app.status().await?)?),
        ["recommend"] => {
            for r in app.recommendations()? {
                let c = &r.candidate;
                println!(
                    "{:<26} {:<11} {:>8.0} Mo  besoin {:>7.0} Mo  {:?}  score {:.1}{}",
                    c.model_name,
                    c.quant,
                    mb(c.file_size),
                    mb(c.memory_needed),
                    c.placement,
                    c.score,
                    if r.installed { "  [installé]" } else { "" }
                );
            }
        }
        ["runtime"] => {
            let rt = app
                .install_runtime(|ev| match ev {
                    InstallEvent::Downloading { file, progress } => {
                        print!(
                            "\r{file} : {:.0} / {:.0} Mo   ",
                            mb(progress.downloaded),
                            mb(progress.total.unwrap_or(0))
                        );
                        std::io::stdout().flush().ok();
                    }
                    other => println!("{other:?}"),
                })
                .await?;
            println!("\nmoteur installé : {rt:?}");
        }
        ["install", id, quant] => {
            let m = app
                .install_model(id, quant, |p| {
                    print!(
                        "\r{:.0} / {:.0} Mo  ({:.1} Mo/s)   ",
                        mb(p.downloaded),
                        mb(p.total.unwrap_or(0)),
                        mb(p.bytes_per_sec)
                    );
                    std::io::stdout().flush().ok();
                })
                .await?;
            println!("\ninstallé : {}", m.key);
        }
        ["chat", text] => {
            let session = app.create_session()?;
            let reply = app
                .send_message(&session.id, text, |ev| {
                    match ev {
                        ChatEvent::Token { text } => print!("{text}"),
                        ChatEvent::Reasoning { text } => print!("\x1b[2m{text}\x1b[0m"),
                    }
                    std::io::stdout().flush().ok();
                })
                .await?;
            println!(
                "\n\n[{} jetons, {:.1} jetons/s, {} ms]",
                reply.message.tokens.unwrap_or(0),
                reply.tokens_per_sec.unwrap_or(0.0),
                reply.message.duration_ms.unwrap_or(0)
            );
        }
        _ => {
            bail!("usage : cli status | recommend | runtime | install <id> <quant> | chat <texte>")
        }
    }
    app.shutdown().await;
    Ok(())
}
