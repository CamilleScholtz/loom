//! User-level config file, loaded from `~/.config/book/config.toml` by default
//! (override with `--config <path>`). Carries the OpenRouter API key and
//! per-call-site model selection. Missing file is not an error — the defaults
//! kick in.
//!
//! Precedence for the API key, highest first:
//!   1. `OPENROUTER_API_KEY` environment variable
//!   2. `api_key` in the config file
//!   3. No key → `FakeClient`
//!
//! Precedence for the model used at a given call site:
//!   1. `[models].<call_site>` in the config file
//!   2. `default_model` in the config file
//!   3. Built-in default (`DEFAULT_MODEL`)

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Fallback when nothing in the user config selects a model.
pub const DEFAULT_MODEL: &str = "anthropic/claude-haiku-4-5";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model used for every call site that doesn't have its own override.
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub models: ModelOverrides,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelOverrides {
    #[serde(default)]
    pub scene_narration: Option<String>,
    #[serde(default)]
    pub options: Option<String>,
    #[serde(default)]
    pub npc_voice: Option<String>,
    #[serde(default)]
    pub epilogue: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallSite {
    SceneNarration,
    Options,
    NpcVoice,
    Epilogue,
}

impl UserConfig {
    /// Returns the conventional config path: `$XDG_CONFIG_HOME/book/config.toml`
    /// when set, else `$HOME/.config/book/config.toml`. Returns `None` if
    /// neither env var is available (unusual; typically only in stripped CI
    /// containers).
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Some(PathBuf::from(xdg).join("book").join("config.toml"));
            }
        }
        std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .map(|h| {
                PathBuf::from(h)
                    .join(".config")
                    .join("book")
                    .join("config.toml")
            })
    }

    /// Load the config from `explicit` if provided, else from `default_path()`.
    /// A missing file is not an error — `UserConfig::default()` is returned.
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let path = match explicit {
            Some(p) => Some(p.to_path_buf()),
            None => Self::default_path(),
        };
        let Some(path) = path else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Self =
            toml::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    /// Write this config to `path`, creating parent directories if needed.
    /// Used by the in-TUI config editor to persist edits.
    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = toml::to_string_pretty(self)
            .with_context(|| format!("serializing config for {}", path.display()))?;
        std::fs::write(path, body)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Resolve the model to use for one call site, applying the precedence
    /// rules documented at the module level.
    pub fn model_for(&self, call_site: CallSite) -> &str {
        let override_str = match call_site {
            CallSite::SceneNarration => self.models.scene_narration.as_deref(),
            CallSite::Options => self.models.options.as_deref(),
            CallSite::NpcVoice => self.models.npc_voice.as_deref(),
            CallSite::Epilogue => self.models.epilogue.as_deref(),
        };
        override_str
            .or(self.default_model.as_deref())
            .unwrap_or(DEFAULT_MODEL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn defaults_are_empty_and_fall_back_to_builtin() {
        let cfg = UserConfig::default();
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.model_for(CallSite::SceneNarration), DEFAULT_MODEL);
        assert_eq!(cfg.model_for(CallSite::Options), DEFAULT_MODEL);
        assert_eq!(cfg.model_for(CallSite::NpcVoice), DEFAULT_MODEL);
    }

    #[test]
    fn default_model_applies_when_no_override() {
        let cfg = UserConfig {
            default_model: Some("acme/foo".into()),
            ..Default::default()
        };
        assert_eq!(cfg.model_for(CallSite::SceneNarration), "acme/foo");
        assert_eq!(cfg.model_for(CallSite::NpcVoice), "acme/foo");
    }

    #[test]
    fn per_call_site_override_wins() {
        let cfg = UserConfig {
            default_model: Some("acme/cheap".into()),
            models: ModelOverrides {
                npc_voice: Some("acme/strong".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.model_for(CallSite::SceneNarration), "acme/cheap");
        assert_eq!(cfg.model_for(CallSite::NpcVoice), "acme/strong");
    }

    #[test]
    fn missing_file_yields_default_not_error() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope.toml");
        let cfg = UserConfig::load(Some(&missing)).unwrap();
        assert!(cfg.api_key.is_none());
    }

    #[test]
    fn loads_from_explicit_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
api_key = "sk-test-xyz"
default_model = "acme/base"

[models]
npc_voice = "acme/strong"
"#,
        )
        .unwrap();
        let cfg = UserConfig::load(Some(&path)).unwrap();
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test-xyz"));
        assert_eq!(cfg.model_for(CallSite::Options), "acme/base");
        assert_eq!(cfg.model_for(CallSite::NpcVoice), "acme/strong");
    }

    #[test]
    fn round_trips_through_toml() {
        let cfg = UserConfig {
            api_key: Some("key".into()),
            default_model: Some("d".into()),
            models: ModelOverrides {
                scene_narration: Some("a".into()),
                options: Some("b".into()),
                npc_voice: Some("c".into()),
                epilogue: Some("e".into()),
            },
        };
        let body = toml::to_string_pretty(&cfg).unwrap();
        let back: UserConfig = toml::from_str(&body).unwrap();
        assert_eq!(back.api_key.as_deref(), Some("key"));
        assert_eq!(back.model_for(CallSite::SceneNarration), "a");
        assert_eq!(back.model_for(CallSite::Options), "b");
        assert_eq!(back.model_for(CallSite::NpcVoice), "c");
        assert_eq!(back.model_for(CallSite::Epilogue), "e");
    }
}
