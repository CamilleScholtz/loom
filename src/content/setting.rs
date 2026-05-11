use serde::{Deserialize, Serialize};

use super::goals::Goals;
use super::items::Items;
use super::locations::Locations;
use super::names::Names;
use super::occupations::Occupations;
use super::pack_version::Version;
use super::place_names::PlaceNames;
use super::prompts::PromptTemplates;
use super::secrets::Secrets;
use super::taboos::Taboos;
use super::traits::Traits;
use super::vocabulary::Vocabulary;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub name: String,
    pub period: String,
    pub register: String,
    pub version: Version,
    /// Generic noun for the place the playthrough is set in — "village",
    /// "ship", "outpost". Used in UI labels and as the root for derived
    /// phrasings like "leave the {place_kind}".
    #[serde(default = "default_place_kind")]
    pub place_kind: String,
    /// One inhabitant of `place_kind` — "villager", "crewmate". Used as a
    /// fallback when an NPC's name isn't known.
    #[serde(default = "default_inhabitant_singular")]
    pub inhabitant_singular: String,
    /// All inhabitants — "villagers", "crew". Used in UI labels like "The
    /// {inhabitant_plural} will call you:".
    #[serde(default = "default_inhabitant_plural")]
    pub inhabitant_plural: String,
}

fn default_place_kind() -> String {
    "village".into()
}
fn default_inhabitant_singular() -> String {
    "villager".into()
}
fn default_inhabitant_plural() -> String {
    "villagers".into()
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingPack {
    pub setting: Setting,
    pub vocabulary: Vocabulary,
    pub names: Names,
    pub items: Items,
    pub locations: Locations,
    pub occupations: Occupations,
    pub taboos: Taboos,
    pub traits: Traits,
    pub prompts: PromptTemplates,
    pub goals: Goals,
    pub place_names: PlaceNames,
    pub secrets: Secrets,
}
