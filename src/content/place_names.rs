use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceNames {
    #[serde(default)]
    pub villages: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_sample() {
        let body = r#"
villages = ["Ashwell", "Brookmere"]
"#;
        let p: PlaceNames = toml::from_str(body).unwrap();
        assert_eq!(p.villages.len(), 2);
        assert_eq!(p.villages[0], "Ashwell");
    }

    #[test]
    fn missing_villages_default_empty() {
        let p: PlaceNames = toml::from_str("").unwrap();
        assert!(p.villages.is_empty());
    }
}
