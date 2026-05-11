use serde::{Deserialize, Serialize};

/// One secret category authored in the setting pack. The category drives the
/// engine's reaction weighting; the templates seed actual `Fact` summaries at
/// worldgen time. `affinity_traits` are vice/virtue names that bias an NPC
/// toward holding this kind of secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretCategory {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub summary_templates: Vec<String>,
    #[serde(default)]
    pub affinity_traits: Vec<String>,
    #[serde(default = "default_min_age")]
    pub min_age: u32,
}

fn default_min_age() -> u32 {
    18
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secrets {
    #[serde(default, rename = "category")]
    pub categories: Vec<SecretCategory>,
}

impl Secrets {
    pub fn by_name(&self, name: &str) -> Option<&SecretCategory> {
        self.categories.iter().find(|c| c.name == name)
    }
}
