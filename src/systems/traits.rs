use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Value {
    Duty,
    Freedom,
    Pleasure,
    Ambition,
    Faith,
    Family,
    Reputation,
    Curiosity,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ValueWeights(pub BTreeMap<Value, f32>);

impl ValueWeights {
    pub fn get(&self, v: Value) -> f32 {
        self.0.get(&v).copied().unwrap_or(0.0)
    }

    pub fn set(&mut self, v: Value, w: f32) {
        self.0.insert(v, w);
    }

    /// Return a copy with weights normalized to sum to 1.0. Negative entries
    /// are treated as zero. If all weights are zero, returns an even split.
    pub fn normalized(&self) -> Self {
        let total: f32 = self.0.values().map(|w| w.max(0.0)).sum();
        if total < f32::EPSILON {
            let n = self.0.len().max(1) as f32;
            let mut out = self.0.clone();
            for w in out.values_mut() {
                *w = 1.0 / n;
            }
            return Self(out);
        }
        let mut out = self.0.clone();
        for w in out.values_mut() {
            *w = w.max(0.0) / total;
        }
        Self(out)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitSet {
    #[serde(default)]
    pub virtues: Vec<String>,
    #[serde(default)]
    pub vices: Vec<String>,
    #[serde(default)]
    pub inclinations: Vec<String>,
}

impl TraitSet {
    pub fn has(&self, name: &str) -> bool {
        self.virtues.iter().any(|t| t == name)
            || self.vices.iter().any(|t| t == name)
            || self.inclinations.iter().any(|t| t == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_weights_normalize_sum_to_one() {
        let mut w = ValueWeights::default();
        w.set(Value::Duty, 2.0);
        w.set(Value::Faith, 1.0);
        w.set(Value::Pleasure, 1.0);
        let n = w.normalized();
        let total: f32 = n.0.values().sum();
        assert!((total - 1.0).abs() < 1e-5, "total = {}", total);
        assert!((n.get(Value::Duty) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn value_weights_negative_treated_as_zero() {
        let mut w = ValueWeights::default();
        w.set(Value::Duty, -3.0);
        w.set(Value::Faith, 1.0);
        let n = w.normalized();
        assert_eq!(n.get(Value::Duty), 0.0);
        assert!((n.get(Value::Faith) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn value_weights_all_zero_splits_evenly() {
        let mut w = ValueWeights::default();
        w.set(Value::Duty, 0.0);
        w.set(Value::Faith, 0.0);
        let n = w.normalized();
        assert!((n.get(Value::Duty) - 0.5).abs() < 1e-5);
        assert!((n.get(Value::Faith) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn traitset_has_matches_across_categories() {
        let t = TraitSet {
            virtues: vec!["Kind".into()],
            vices: vec!["Vain".into()],
            inclinations: vec!["Romantic".into()],
        };
        assert!(t.has("Kind"));
        assert!(t.has("Vain"));
        assert!(t.has("Romantic"));
        assert!(!t.has("Wrathful"));
    }

    #[test]
    fn value_serde_roundtrips_as_named_variants() {
        let s = serde_json::to_string(&Value::Faith).unwrap();
        assert_eq!(s, "\"Faith\"");
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, Value::Faith);
    }
}

#[cfg(test)]
mod toml_check {
    use super::*;

    #[test]
    fn values_serialize_as_toml_table() {
        let mut w = ValueWeights::default();
        w.set(Value::Faith, 0.3);
        w.set(Value::Duty, 0.7);
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Wrap { values: ValueWeights }
        let body = toml::to_string_pretty(&Wrap { values: w.clone() }).expect("ser");
        let back: Wrap = toml::from_str(&body).expect("de");
        assert_eq!(back.values.0, w.0);
    }
}
