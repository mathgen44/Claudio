use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Initialise les journaux : un fichier par jour dans `logs_dir` et la console.
///
/// Le niveau se règle avec `CLAUDIO_LOG` (ex. `debug`, `claudio_llm=trace`).
/// Garder le `WorkerGuard` vivant jusqu'à la fin du programme.
pub fn init(logs_dir: &Path) -> WorkerGuard {
    let appender = tracing_appender::rolling::daily(logs_dir, "claudio.log");
    let (file_writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("CLAUDIO_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,claudio=debug,claudio_core=debug,claudio_llm=debug,claudio_models=debug,claudio_hw=debug"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file_writer).with_ansi(false))
        .with(fmt::layer().with_writer(std::io::stderr))
        .try_init();
    record_panics(logs_dir);
    guard
}

/// Inscrit les paniques dans `plantages.log`. L'application est compilée avec
/// `panic = "abort"` : l'écriture est directe, le journal habituel (tamponné)
/// n'aurait pas le temps d'être vidé.
fn record_panics(logs_dir: &Path) {
    use std::io::Write;
    let file = logs_dir.join("plantages.log");
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let report = format!(
            "{} version {} fil « {} » : {info}\n",
            chrono::Utc::now().to_rfc3339(),
            env!("CARGO_PKG_VERSION"),
            thread.name().unwrap_or("?"),
        );
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
        {
            let _ = f.write_all(report.as_bytes());
        }
        default(info);
    }));
}
