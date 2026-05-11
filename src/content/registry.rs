use std::fs;
use std::path::Path;

use anyhow::Result;

use super::loader::{load_setting, load_vocation};
use super::setting::SettingPack;
use super::vocation::VocationPack;

#[derive(Clone, Debug, PartialEq)]
pub struct ContentRegistry {
    pub setting: SettingPack,
    pub vocation: VocationPack,
}

/// Enumerate setting pack names available under `<root>/settings/`. Returns
/// an empty Vec if the directory is missing. Used by the title menu's setting
/// cycler so the user can browse what's installed without editing CLI flags.
pub fn list_setting_packs(root: &Path) -> Vec<String> {
    list_pack_names(&root.join("settings"))
}

/// Enumerate vocation pack names available under
/// `<root>/settings/<setting>/vocations/`. Vocations are setting-scoped — a
/// setting must be chosen before the pool is known.
pub fn list_vocation_packs(root: &Path, setting: &str) -> Vec<String> {
    list_pack_names(&root.join("settings").join(setting).join("vocations"))
}

fn list_pack_names(dir: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    names.sort();
    names
}

impl ContentRegistry {
    pub fn load(root: &Path, setting: &str, vocation: &str) -> Result<Self> {
        tracing::info!(
            content_root = %root.display(),
            setting,
            vocation,
            "loading content packs"
        );
        let setting_pack = load_setting(root, setting)?;
        let vocation_pack = load_vocation(root, setting, vocation)?;
        tracing::info!(
            setting = %setting_pack.setting.name,
            vocation = %vocation_pack.vocation.name,
            setting_version = %setting_pack.setting.version,
            vocation_version = %vocation_pack.vocation.version,
            "content loaded"
        );
        Ok(Self {
            setting: setting_pack,
            vocation: vocation_pack,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn loads_bundled_medieval_and_investigator() {
        let root = project_root().join("content");
        let registry = ContentRegistry::load(&root, "medieval", "investigator").unwrap();

        assert_eq!(registry.setting.setting.name, "medieval");
        assert_eq!(registry.vocation.vocation.name, "investigator");

        assert!(!registry.setting.names.given_male.is_empty());
        assert!(!registry.setting.names.given_female.is_empty());
        assert!(!registry.setting.names.surnames.is_empty());
        assert!(!registry.setting.items.items.is_empty());
        assert!(!registry.setting.locations.locations.is_empty());
        assert!(!registry.setting.occupations.occupations.is_empty());
        assert!(!registry.setting.taboos.taboos.is_empty());
        for key in ["npc_voice", "scene_open", "options", "epilogue"] {
            assert!(
                registry.setting.prompts.get(key).is_some(),
                "missing prompt template {}",
                key
            );
        }
        assert!(!registry.vocation.starting_skills.skills.is_empty());
        assert!(!registry.vocation.contacts.contacts.is_empty());
        assert!(!registry.vocation.opening_hook.template.is_empty());
        assert!(
            registry.setting.traits.virtues.len() >= 5,
            "expected ≥ 5 virtues, got {}",
            registry.setting.traits.virtues.len()
        );
        assert!(
            registry.setting.traits.vices.len() >= 5,
            "expected ≥ 5 vices, got {}",
            registry.setting.traits.vices.len()
        );
        assert!(
            registry.setting.traits.inclinations.len() >= 5,
            "expected ≥ 5 inclinations, got {}",
            registry.setting.traits.inclinations.len()
        );
        assert!(
            registry.vocation.backgrounds.backgrounds.len() >= 3,
            "expected ≥ 3 backgrounds, got {}",
            registry.vocation.backgrounds.backgrounds.len()
        );
        assert!(
            registry.setting.goals.defs.len() >= 6,
            "expected ≥ 6 goal archetypes, got {}",
            registry.setting.goals.defs.len()
        );
        assert!(
            registry.setting.place_names.villages.len() >= 5,
            "expected ≥ 5 village names, got {}",
            registry.setting.place_names.villages.len()
        );
        // Sanity: every goal kind is non-empty and unique.
        let mut kinds: Vec<&str> =
            registry.setting.goals.defs.iter().map(|g| g.kind.as_str()).collect();
        kinds.sort();
        let n = kinds.len();
        kinds.dedup();
        assert_eq!(kinds.len(), n, "goal kinds must be unique");
    }
}
