use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vocabulary {
    #[serde(flatten)]
    pub categories: BTreeMap<String, Vec<String>>,
}

impl Vocabulary {
    pub fn category(&self, name: &str) -> Option<&[String]> {
        self.categories.get(name).map(|v| v.as_slice())
    }
}
