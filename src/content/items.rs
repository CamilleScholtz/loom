use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemArchetype {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
    #[serde(default)]
    pub value: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Items {
    #[serde(default, rename = "item")]
    pub items: Vec<ItemArchetype>,
}
