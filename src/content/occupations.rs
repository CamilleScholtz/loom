use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occupation {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub typical_traits: Vec<String>,
    /// Location `kind` where this occupation does its work — e.g. "tavern",
    /// "mill", "smithy". Empty string means the work has no fixed location
    /// and the holder wanders.
    #[serde(default)]
    pub work_location_kind: String,
    /// Location `kind` the holder retires to overnight. Empty string means
    /// no fixed home; default sleep behavior is wherever they happen to be.
    #[serde(default)]
    pub home_location_kind: String,
    /// Half-open `[start, end)` business hours. `end > start`: a daytime
    /// shift. `end <= start`: an overnight shift wrapping past midnight.
    /// Default `(0, 0)` means no fixed hours.
    #[serde(default)]
    pub work_hours: (u8, u8),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occupations {
    #[serde(default, rename = "occupation")]
    pub occupations: Vec<Occupation>,
}
