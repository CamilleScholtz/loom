use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Background {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub starting_skills: Vec<String>,
    #[serde(default)]
    pub initial_reputation: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backgrounds {
    #[serde(default, rename = "background")]
    pub backgrounds: Vec<Background>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_sample() {
        let body = r#"
[[background]]
name = "Former magistrate's clerk"
description = "Reads handwriting and weighs testimony."
starting_skills = ["records"]
initial_reputation = 1
"#;
        let b: Backgrounds = toml::from_str(body).unwrap();
        assert_eq!(b.backgrounds.len(), 1);
        assert_eq!(b.backgrounds[0].starting_skills, vec!["records"]);
        assert_eq!(b.backgrounds[0].initial_reputation, 1);
    }

    #[test]
    fn defaults_apply_for_optional_fields() {
        let body = r#"
[[background]]
name = "Country physician"
description = "Country doctor."
"#;
        let b: Backgrounds = toml::from_str(body).unwrap();
        assert!(b.backgrounds[0].starting_skills.is_empty());
        assert_eq!(b.backgrounds[0].initial_reputation, 0);
    }
}
