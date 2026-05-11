use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Taboo {
    pub name: String,
    pub severity: u8,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Taboos {
    #[serde(default, rename = "taboo")]
    pub taboos: Vec<Taboo>,
}
