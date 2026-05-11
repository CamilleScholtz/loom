use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptTemplates {
    pub templates: BTreeMap<String, String>,
}

impl PromptTemplates {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.templates.get(key).map(String::as_str)
    }
}
