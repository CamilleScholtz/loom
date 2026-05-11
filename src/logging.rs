use std::path::Path;

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

pub fn init(save_dir: &Path, level: tracing::Level) -> Result<WorkerGuard> {
    std::fs::create_dir_all(save_dir)
        .with_context(|| format!("creating log dir {}", save_dir.display()))?;

    let appender = tracing_appender::rolling::never(save_dir, "loom.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level.to_string().to_ascii_lowercase()));

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_env_filter(filter)
        .with_target(true)
        .init();

    tracing::info!(
        log_dir = %save_dir.display(),
        level = %level,
        "logging initialized"
    );

    Ok(guard)
}
