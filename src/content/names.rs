use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Names {
    pub given_male: Vec<String>,
    pub given_female: Vec<String>,
    pub surnames: Vec<String>,
}

pub fn read_name_list(path: &Path) -> Result<Vec<String>> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("reading name list at {}", path.display()))?;
    Ok(parse_name_list(&body))
}

pub(crate) fn parse_name_list(body: &str) -> Vec<String> {
    body.lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skipping_blanks_and_comments() {
        let body = "Aldwin\n  Bryn  \n\n# a comment\nCedric\n";
        let names = parse_name_list(body);
        assert_eq!(names, vec!["Aldwin", "Bryn", "Cedric"]);
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(parse_name_list("").is_empty());
        assert!(parse_name_list("\n\n# only comment\n").is_empty());
    }
}
