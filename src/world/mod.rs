pub mod ids;
pub mod location;
pub mod npc;
pub mod player;
pub mod sex;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::Fact;
use crate::systems::Edge;

#[allow(unused_imports)]
pub use ids::{FactId, ItemId, LocationId, NpcId};
pub use location::Location;
pub use npc::{MemorableEvent, Npc, Schedule, ScheduleBlock, Secret};
pub use player::PlayerCharacter;
pub use sex::Sex;

fn default_place_kind() -> String {
    "village".into()
}
fn default_inhabitant_singular() -> String {
    "villager".into()
}
fn default_inhabitant_plural() -> String {
    "villagers".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct World {
    pub seed: u64,
    pub day: u32,
    pub event_index: u64,
    pub clock_minutes: u32,
    pub next_fact_id: u32,
    #[serde(default)]
    pub village_name: String,
    #[serde(default = "default_place_kind")]
    pub place_kind: String,
    #[serde(default = "default_inhabitant_singular")]
    pub inhabitant_singular: String,
    #[serde(default = "default_inhabitant_plural")]
    pub inhabitant_plural: String,
    #[serde(default)]
    pub start_day: u32,
    #[serde(default)]
    pub deadline_day: u32,
    pub npcs: BTreeMap<NpcId, Npc>,
    pub locations: BTreeMap<LocationId, Location>,
    pub facts: BTreeMap<FactId, Fact>,
    /// Directed relationship edges. Mutated **only** by
    /// [`crate::systems::relationships::apply_relationship_shift`] via the
    /// [`Event::RelationshipShift`] event. Read access via
    /// [`World::relationship`] and [`World::relationships_from`].
    #[serde(default)]
    pub(crate) relationships: BTreeMap<(NpcId, NpcId), Edge>,
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            day: 0,
            event_index: 0,
            clock_minutes: 0,
            next_fact_id: 1,
            village_name: String::new(),
            place_kind: default_place_kind(),
            inhabitant_singular: default_inhabitant_singular(),
            inhabitant_plural: default_inhabitant_plural(),
            start_day: 0,
            deadline_day: 0,
            npcs: BTreeMap::new(),
            locations: BTreeMap::new(),
            facts: BTreeMap::new(),
            relationships: BTreeMap::new(),
        }
    }

    pub fn insert_npc(&mut self, npc: Npc) {
        self.npcs.insert(npc.id, npc);
    }

    pub fn insert_location(&mut self, location: Location) {
        self.locations.insert(location.id, location);
    }

    pub fn npc(&self, id: NpcId) -> Option<&Npc> {
        self.npcs.get(&id)
    }

    pub fn location(&self, id: LocationId) -> Option<&Location> {
        self.locations.get(&id)
    }

    pub fn fact(&self, id: FactId) -> Option<&Fact> {
        self.facts.get(&id)
    }

    /// Returns the directed edge from `subject` to `object`, or a default
    /// (all-zero) edge if none is recorded.
    pub fn relationship(&self, subject: NpcId, object: NpcId) -> Edge {
        self.relationships
            .get(&(subject, object))
            .copied()
            .unwrap_or_default()
    }

    /// Iterate outgoing edges from `subject` (object, edge) pairs.
    pub fn relationships_from(
        &self,
        subject: NpcId,
    ) -> impl Iterator<Item = (NpcId, &Edge)> + '_ {
        self.relationships
            .range((subject, NpcId(u32::MIN))..=(subject, NpcId(u32::MAX)))
            .map(|((_, obj), edge)| (*obj, edge))
    }

    /// Iterate every edge in the world.
    pub fn relationships_iter(&self) -> impl Iterator<Item = (&(NpcId, NpcId), &Edge)> + '_ {
        self.relationships.iter()
    }

    /// Intern a fact, deduplicating by value. Returns the existing FactId if an
    /// equal Fact is already registered, otherwise allocates a fresh one.
    pub fn intern_fact(&mut self, fact: Fact) -> FactId {
        for (id, existing) in &self.facts {
            if existing == &fact {
                return *id;
            }
        }
        let id = FactId(self.next_fact_id);
        self.next_fact_id = self.next_fact_id.checked_add(1).expect("FactId overflow");
        self.facts.insert(id, fact);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(kind: &str, summary: &str) -> Fact {
        Fact {
            kind: kind.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn intern_fact_dedupes_by_value() {
        let mut world = World::new(0);
        let a = world.intern_fact(fact("rumor", "the well runs dry"));
        let b = world.intern_fact(fact("rumor", "the well runs dry"));
        assert_eq!(a, b);
        assert_eq!(world.facts.len(), 1);
    }

    #[test]
    fn intern_fact_allocates_distinct_ids_for_distinct_facts() {
        let mut world = World::new(0);
        let a = world.intern_fact(fact("rumor", "x"));
        let b = world.intern_fact(fact("rumor", "y"));
        assert_ne!(a, b);
        assert_eq!(world.facts.len(), 2);
    }

    #[test]
    fn intern_fact_advances_next_fact_id() {
        let mut world = World::new(0);
        assert_eq!(world.next_fact_id, 1);
        let _ = world.intern_fact(fact("k", "a"));
        assert_eq!(world.next_fact_id, 2);
        let _ = world.intern_fact(fact("k", "b"));
        assert_eq!(world.next_fact_id, 3);
    }
}
