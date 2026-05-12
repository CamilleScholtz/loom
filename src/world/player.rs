use serde::{Deserialize, Serialize};

use super::ids::NpcId;
use super::sex::Sex;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerCharacter {
    pub id: NpcId,
    pub name: String,
    pub vocation: String,
    pub virtue: String,
    pub vice: String,
    pub inclination: String,
    pub background: String,
    #[serde(default)]
    pub sex: Sex,
}
