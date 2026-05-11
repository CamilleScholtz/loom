use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitOption {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traits {
    #[serde(default, rename = "virtue")]
    pub virtues: Vec<TraitOption>,
    #[serde(default, rename = "vice")]
    pub vices: Vec<TraitOption>,
    #[serde(default, rename = "inclination")]
    pub inclinations: Vec<TraitOption>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_sample() {
        let body = r#"
[[virtue]]
name = "Kind"
description = "soft answer"

[[vice]]
name = "Vain"
description = "pride a lever"

[[inclination]]
name = "Romantic"
description = "drawn to entanglements"
"#;
        let t: Traits = toml::from_str(body).unwrap();
        assert_eq!(t.virtues.len(), 1);
        assert_eq!(t.vices.len(), 1);
        assert_eq!(t.inclinations.len(), 1);
        assert_eq!(t.virtues[0].name, "Kind");
    }
}
