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
    guard
}
