use serde::{Deserialize, Serialize};

use crate::systems::goals::GoalDef;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Goals {
    #[serde(default, rename = "goal")]
    pub defs: Vec<GoalDef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_sample() {
        let body = r#"
[[goal]]
kind = "eat"
summary_template = "to put something hot in the belly"
trait_affinity = []
value_affinity = [["Pleasure", 0.3]]

[goal.urgency]
op = "need"
need = "Hunger"

[goal.precondition]
op = "need_at_least"
need = "Hunger"
threshold = 0.4

[goal.resolve]
op = "need_below"
need = "Hunger"
threshold = 0.2
"#;
        let g: Goals = toml::from_str(body).unwrap();
        assert_eq!(g.defs.len(), 1);
        assert_eq!(g.defs[0].kind, "eat");
    }
}
