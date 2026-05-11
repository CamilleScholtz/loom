use serde::{Deserialize, Serialize};

use super::ids::LocationId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub id: LocationId,
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub adjacent: Vec<LocationId>,
}

impl Location {
    pub fn new(id: LocationId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            kind: String::new(),
            description: String::new(),
            adjacent: Vec::new(),
        }
    }
}
