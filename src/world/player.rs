use serde::{Deserialize, Serialize};

use super::ids::NpcId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerCharacter {
    pub id: NpcId,
    pub name: String,
    pub vocation: String,
    pub virtue: String,
    pub vice: String,
    pub inclination: String,
    pub background: String,
}
