use anyhow::{Context, Result};
use clap::Parser;

use loom::{Cli, app, config::UserConfig, content, llm, logging};

fn main() -> Result<()> {
    let cli = Cli::parse();

    std::fs::create_dir_all(&cli.save_dir)
        .with_context(|| format!("creating save dir {}", cli.save_dir.display()))?;

    let _log_guard = logging::init(&cli.save_dir, cli.log_level)?;

    let user_config = UserConfig::load(cli.config.as_deref()).context("loading user config")?;

    tracing::info!(
        seed = ?cli.seed,
        save_dir = %cli.save_dir.display(),
        dry_run = cli.dry_run,
        config_path = ?cli.config.as_deref().map(|p| p.display().to_string()),
        "loom starting"
    );

    let llm_cfg = llm::LlmConfig::resolve(cli.dry_run, user_config.api_key.as_deref());
    let client = llm::make_client(&llm_cfg);

    let registry = content::ContentRegistry::load(&cli.content_root, &cli.setting, &cli.vocation)
        .with_context(|| {
            format!(
                "loading content (setting={}, vocation={}) from {}",
                cli.setting,
                cli.vocation,
                cli.content_root.display()
            )
        })?;

    let mut terminal = ratatui::init();
    let result = app::App::new(cli, client, registry, user_config).run(&mut terminal);
    ratatui::restore();
    result
}
