//! Library surface. The binary entry point in `src/main.rs` is a thin wrapper
//! over `App::new(...).run(...)`. Everything else lives here so integration
//! tests can reach the public API.

use std::path::PathBuf;

use clap::Parser;

pub mod app;
pub mod config;
pub mod content;
pub mod engine;
pub mod llm;
pub mod logging;
pub mod storage;
pub mod systems;
pub mod ui;
pub mod world;

#[derive(Parser, Debug, Clone)]
#[command(name = "book", about = "a system-driven narrative")]
pub struct Cli {
    /// Deterministic seed for worldgen. Randomized if omitted.
    #[arg(long)]
    pub seed: Option<u64>,

    /// Directory for the current save (world.toml, events.log, book.log, ...).
    #[arg(long, default_value = "saves/current")]
    pub save_dir: PathBuf,

    /// Use the deterministic stub LLM client; never contacts OpenRouter.
    #[arg(long)]
    pub dry_run: bool,

    /// Tracing level for the file log: trace|debug|info|warn|error.
    #[arg(long, default_value = "info")]
    pub log_level: tracing::Level,

    /// Root directory holding content packs (settings/, vocations/).
    #[arg(long, default_value = "content")]
    pub content_root: PathBuf,

    /// Name of the setting pack to load from <content-root>/settings/.
    #[arg(long, default_value = "medieval")]
    pub setting: String,

    /// Name of the vocation pack to load from <content-root>/vocations/.
    #[arg(long, default_value = "investigator")]
    pub vocation: String,

    /// Path to a TOML config file (api_key, default_model, [models] overrides).
    /// Defaults to `$XDG_CONFIG_HOME/book/config.toml` or
    /// `$HOME/.config/book/config.toml`. Missing file is not an error.
    #[arg(long)]
    pub config: Option<PathBuf>,
}
