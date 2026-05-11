use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(
            Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_newtype!(NpcId);
id_newtype!(LocationId);
id_newtype!(ItemId);
id_newtype!(FactId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_via_json() {
        let id = NpcId(42);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "42");
        let back: NpcId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn ids_are_distinct_types() {
        let n = NpcId(1);
        let l = LocationId(1);
        assert_eq!(n.to_string(), l.to_string());
        assert_eq!(n.0, l.0);
    }
}
