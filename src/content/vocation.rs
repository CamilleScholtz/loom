use serde::{Deserialize, Serialize};

use super::backgrounds::Backgrounds;
use super::pack_version::Version;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vocation {
    pub name: String,
    pub version: Version,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartingSkills {
    #[serde(default, rename = "skill")]
    pub skills: Vec<Skill>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub role: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contacts {
    #[serde(default, rename = "contact")]
    pub contacts: Vec<Contact>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpeningHook {
    pub template: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VocationPack {
    pub vocation: Vocation,
    pub starting_skills: StartingSkills,
    pub contacts: Contacts,
    pub backgrounds: Backgrounds,
    pub opening_hook: OpeningHook,
}
