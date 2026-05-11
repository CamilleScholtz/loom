use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationArchetype {
    pub name: String,
    pub kind: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locations {
    #[serde(default, rename = "location")]
    pub locations: Vec<LocationArchetype>,
}
