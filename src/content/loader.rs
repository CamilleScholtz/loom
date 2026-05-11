use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use thiserror::Error;

use super::backgrounds::Backgrounds;
use super::goals::Goals;
use super::items::Items;
use super::place_names::PlaceNames;
use super::locations::Locations;
use super::names::{Names, read_name_list};
use super::occupations::Occupations;
use super::pack_version::{Version, engine_version};
use super::prompts::PromptTemplates;
use super::secrets::Secrets;
use super::setting::{Setting, SettingPack};
use super::taboos::Taboos;
use super::traits::Traits;
use super::vocabulary::Vocabulary;
use super::vocation::{Contacts, OpeningHook, StartingSkills, Vocation, VocationPack};

#[derive(Error, Debug)]
pub enum ContentError {
    #[error(
        "pack {pack:?} declares version {found} but engine is {engine}; minor versions must match in 0.x"
    )]
    IncompatibleVersion {
        pack: String,
        found: Version,
        engine: Version,
    },
}

pub fn load_setting(root: &Path, name: &str) -> Result<SettingPack> {
    let pack_dir = root.join("settings").join(name);
    if !pack_dir.is_dir() {
        anyhow::bail!(
            "setting pack {:?} not found at {}",
            name,
            pack_dir.display()
        );
    }

    let setting: Setting = read_toml(&pack_dir.join("setting.toml"))?;
    let engine = engine_version();
    if !setting.version.is_compatible_with(engine) {
        return Err(ContentError::IncompatibleVersion {
            pack: format!("setting:{}", name),
            found: setting.version,
            engine,
        }
        .into());
    }

    let vocabulary: Vocabulary = read_toml(&pack_dir.join("vocabulary.toml"))?;
    let names = load_names(&pack_dir.join("names"))?;
    let items: Items = read_toml(&pack_dir.join("items.toml"))?;
    let locations: Locations = read_toml(&pack_dir.join("locations.toml"))?;
    let occupations: Occupations = read_toml(&pack_dir.join("occupations.toml"))?;
    let taboos: Taboos = read_toml(&pack_dir.join("taboos.toml"))?;
    let traits: Traits = read_toml(&pack_dir.join("traits.toml"))?;
    let prompts = load_prompts(&pack_dir.join("prompts"))?;
    let goals_path = pack_dir.join("goals.toml");
    let goals: Goals = if goals_path.exists() {
        read_toml(&goals_path)?
    } else {
        Goals::default()
    };
    let place_names_path = pack_dir.join("place_names.toml");
    let place_names: PlaceNames = if place_names_path.exists() {
        read_toml(&place_names_path)?
    } else {
        PlaceNames::default()
    };
    let secrets_path = pack_dir.join("secrets.toml");
    let secrets: Secrets = if secrets_path.exists() {
        read_toml(&secrets_path)?
    } else {
        Secrets::default()
    };

    Ok(SettingPack {
        setting,
        vocabulary,
        names,
        items,
        locations,
        occupations,
        taboos,
        traits,
        prompts,
        goals,
        place_names,
        secrets,
    })
}

/// Load a vocation pack from `<root>/settings/<setting>/vocations/<name>/`.
///
/// Vocations live under their parent setting because backgrounds, contacts,
/// and the opening hook are setting-flavored — a "former magistrate's clerk"
/// reads as a medieval background; the sci-fi version is a "former corporate
/// auditor". Sharing one vocation pack across settings would force one
/// register on the other.
pub fn load_vocation(root: &Path, setting: &str, name: &str) -> Result<VocationPack> {
    let pack_dir = root
        .join("settings")
        .join(setting)
        .join("vocations")
        .join(name);
    if !pack_dir.is_dir() {
        anyhow::bail!(
            "vocation pack {:?} for setting {:?} not found at {}",
            name,
            setting,
            pack_dir.display()
        );
    }

    let vocation: Vocation = read_toml(&pack_dir.join("vocation.toml"))?;
    let engine = engine_version();
    if !vocation.version.is_compatible_with(engine) {
        return Err(ContentError::IncompatibleVersion {
            pack: format!("vocation:{}/{}", setting, name),
            found: vocation.version,
            engine,
        }
        .into());
    }

    let starting_skills: StartingSkills = read_toml(&pack_dir.join("starting_skills.toml"))?;
    let contacts: Contacts = read_toml(&pack_dir.join("contacts.toml"))?;
    let backgrounds: Backgrounds = read_toml(&pack_dir.join("backgrounds.toml"))?;
    let hook_path = pack_dir.join("opening_hook.md");
    let opening_hook = OpeningHook {
        template: fs::read_to_string(&hook_path)
            .with_context(|| format!("reading {}", hook_path.display()))?,
    };

    Ok(VocationPack {
        vocation,
        starting_skills,
        contacts,
        backgrounds,
        opening_hook,
    })
}

fn load_names(dir: &Path) -> Result<Names> {
    Ok(Names {
        given_male: read_name_list(&dir.join("given.male.txt"))?,
        given_female: read_name_list(&dir.join("given.female.txt"))?,
        surnames: read_name_list(&dir.join("surnames.txt"))?,
    })
}

fn load_prompts(dir: &Path) -> Result<PromptTemplates> {
    if !dir.is_dir() {
        anyhow::bail!("prompts directory {} missing", dir.display());
    }
    let mut templates = BTreeMap::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading prompts dir {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
        .collect();
    entries.sort();
    for path in entries {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("bad prompt filename {}", path.display()))?
            .to_string();
        let body = fs::read_to_string(&path)
            .with_context(|| format!("reading prompt {}", path.display()))?;
        templates.insert(stem, body);
    }
    Ok(PromptTemplates { templates })
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&body).with_context(|| format!("parsing toml at {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn scaffold_setting(root: &Path, name: &str, version: &str) {
        let pack = root.join("settings").join(name);
        write(
            &pack.join("setting.toml"),
            &format!(
                "name = \"{}\"\nperiod = \"x\"\nregister = \"y\"\nversion = \"{}\"\n",
                name, version
            ),
        );
        write(&pack.join("vocabulary.toml"), "greetings = [\"hail\"]\n");
        write(&pack.join("names/given.male.txt"), "Aldwin\nBryn\n");
        write(&pack.join("names/given.female.txt"), "Adela\n");
        write(&pack.join("names/surnames.txt"), "Miller\n");
        write(
            &pack.join("items.toml"),
            "[[item]]\nname = \"bread\"\nkind = \"food\"\nvalue = 1\n",
        );
        write(
            &pack.join("locations.toml"),
            "[[location]]\nname = \"tavern\"\nkind = \"social\"\ndescription = \"a place\"\n",
        );
        write(
            &pack.join("occupations.toml"),
            "[[occupation]]\nname = \"miller\"\ndescription = \"grinds grain\"\n",
        );
        write(
            &pack.join("taboos.toml"),
            "[[taboo]]\nname = \"theft\"\nseverity = 4\ndescription = \"do not\"\n",
        );
        write(
            &pack.join("traits.toml"),
            "[[virtue]]\nname = \"Kind\"\ndescription = \"soft\"\n\
             [[vice]]\nname = \"Vain\"\ndescription = \"proud\"\n\
             [[inclination]]\nname = \"Romantic\"\ndescription = \"smitten\"\n",
        );
        write(&pack.join("prompts/npc_voice.md"), "speak as {{npc.name}}\n");
        write(&pack.join("prompts/scene_open.md"), "you arrive at {{scene}}\n");
    }

    #[test]
    fn loads_a_minimal_setting() {
        let dir = TempDir::new().unwrap();
        let engine = engine_version();
        scaffold_setting(dir.path(), "fake", &engine.to_string());
        let pack = load_setting(dir.path(), "fake").unwrap();
        assert_eq!(pack.setting.name, "fake");
        assert_eq!(pack.names.given_male.len(), 2);
        assert_eq!(pack.items.items.len(), 1);
        assert_eq!(pack.prompts.templates.len(), 2);
        assert!(pack.prompts.get("npc_voice").is_some());
    }

    #[test]
    fn rejects_incompatible_version() {
        let dir = TempDir::new().unwrap();
        scaffold_setting(dir.path(), "fake", "9.9.9");
        let err = load_setting(dir.path(), "fake").unwrap_err();
        let downcast = err.downcast_ref::<ContentError>().expect("ContentError");
        match downcast {
            ContentError::IncompatibleVersion { pack, .. } => {
                assert_eq!(pack, "setting:fake");
            }
        }
    }

    #[test]
    fn missing_pack_dir_errors() {
        let dir = TempDir::new().unwrap();
        let err = load_setting(dir.path(), "nope").unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("nope"));
    }
}
