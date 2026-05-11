use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sex {
    Male,
    Female,
}

impl Default for Sex {
    fn default() -> Self {
        Self::Female
    }
}
