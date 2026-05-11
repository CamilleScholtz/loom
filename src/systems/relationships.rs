use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::engine::{EdgeDelta, Time};
use crate::world::{NpcId, World};

/// Coarse courtship stage held on each directed `Edge`. Transitions are
/// computed by `crate::systems::romance::tick` from the underlying axes
/// (attraction, affection, grievance) plus the reciprocal edge's stage. A
/// stage is a projection of state, never an LLM-supplied label.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RomanceStage {
    #[default]
    Strangers,
    Interested,
    Courting,
    Committed,
    Estranged,
}

impl RomanceStage {
    pub fn label(&self) -> &'static str {
        match self {
            RomanceStage::Strangers => "strangers",
            RomanceStage::Interested => "interested",
            RomanceStage::Courting => "courting",
            RomanceStage::Committed => "committed",
            RomanceStage::Estranged => "estranged",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub affection: i32,
    pub trust: i32,
    pub debt: i32,
    pub grievance: i32,
    /// Romantic attraction. Independent of affection per the GAME.md romance
    /// rule ("a character can love someone they don't desire, desire someone
    /// they don't trust"). Bounded `[ATTRACTION_MIN, ATTRACTION_MAX]`.
    #[serde(default)]
    pub attraction: i32,
    /// Coarse courtship stage. Default `Strangers`. Driven by `romance::tick`,
    /// not by `apply_relationship_shift`.
    #[serde(default)]
    pub stage: RomanceStage,
    /// Day the current stage was entered. Used by the time-gated
    /// `Courting → Committed` transition.
    #[serde(default)]
    pub stage_since: Time,
}

impl Edge {
    pub const AFFECTION_MIN: i32 = -100;
    pub const AFFECTION_MAX: i32 = 100;
    pub const TRUST_MIN: i32 = -100;
    pub const TRUST_MAX: i32 = 100;
    pub const GRIEVANCE_MIN: i32 = 0;
    pub const GRIEVANCE_MAX: i32 = 100;
    pub const ATTRACTION_MIN: i32 = -100;
    pub const ATTRACTION_MAX: i32 = 100;

    pub fn clamp_to_bounds(&mut self) {
        self.affection = self.affection.clamp(Self::AFFECTION_MIN, Self::AFFECTION_MAX);
        self.trust = self.trust.clamp(Self::TRUST_MIN, Self::TRUST_MAX);
        self.grievance = self.grievance.clamp(Self::GRIEVANCE_MIN, Self::GRIEVANCE_MAX);
        self.attraction = self
            .attraction
            .clamp(Self::ATTRACTION_MIN, Self::ATTRACTION_MAX);
    }
}

/// Apply a [`crate::engine::Event::RelationshipShift`] to the world. This is
/// the **only** function in the crate that writes to `World.relationships`.
/// Callers outside `systems` must go through [`crate::systems::apply_event`].
pub(crate) fn apply_relationship_shift(
    world: &mut World,
    subject: NpcId,
    object: NpcId,
    delta: &EdgeDelta,
) {
    let edge = world
        .relationships
        .entry((subject, object))
        .or_insert_with(Edge::default);
    edge.affection = edge.affection.saturating_add(delta.affection);
    edge.trust = edge.trust.saturating_add(delta.trust);
    edge.debt = edge.debt.saturating_add(delta.debt);
    edge.grievance = edge.grievance.saturating_add(delta.grievance);
    edge.attraction = edge.attraction.saturating_add(delta.attraction);
    edge.clamp_to_bounds();
}

/// Set the romance `stage` on the directed (subject, object) edge and record
/// the time the stage was entered. Crate-private; the only legitimate caller
/// is `crate::systems::romance::tick`, which derives stages from the existing
/// axes. Other axes (affection, trust, debt, grievance, attraction) still go
/// only through `apply_relationship_shift`.
pub(crate) fn set_stage(
    world: &mut World,
    subject: NpcId,
    object: NpcId,
    stage: RomanceStage,
    when: Time,
) {
    let edge = world
        .relationships
        .entry((subject, object))
        .or_insert_with(Edge::default);
    edge.stage = stage;
    edge.stage_since = when;
}

/// Per-tick grievance decay. Other axes (affection, trust, debt) only change
/// via [`apply_relationship_shift`]. Called by the simulation orchestrator.
pub fn tick(world: &mut World, _now: Time, _rng: &mut ChaCha8Rng) {
    for edge in world.relationships.values_mut() {
        // Multiplicative decay rounded toward 0, never negative.
        let new = (edge.grievance as f32 * 0.99).floor() as i32;
        edge.grievance = new.max(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EdgeDelta;
    use crate::world::Npc;
    use rand::SeedableRng;

    fn t() -> Time {
        Time { day: 0, minute: 0 }
    }

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0)
    }

    fn delta(a: i32, tr: i32, d: i32, g: i32) -> EdgeDelta {
        EdgeDelta {
            affection: a,
            trust: tr,
            debt: d,
            grievance: g,
            attraction: 0,
        }
    }

    fn delta_attr(at: i32) -> EdgeDelta {
        EdgeDelta {
            attraction: at,
            ..EdgeDelta::default()
        }
    }

    fn world_with_two() -> World {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        w.insert_npc(Npc::new(NpcId(2), "b"));
        w
    }

    #[test]
    fn read_default_when_absent() {
        let w = world_with_two();
        let e = w.relationship(NpcId(1), NpcId(2));
        assert_eq!(e, Edge::default());
    }

    #[test]
    fn apply_shift_accumulates() {
        let mut w = world_with_two();
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta(5, 3, 0, 10));
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta(2, -1, 4, 5));
        let e = w.relationship(NpcId(1), NpcId(2));
        assert_eq!(e.affection, 7);
        assert_eq!(e.trust, 2);
        assert_eq!(e.debt, 4);
        assert_eq!(e.grievance, 15);
    }

    #[test]
    fn apply_shift_clamps_bounds() {
        let mut w = world_with_two();
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta(200, -200, 0, 250));
        let e = w.relationship(NpcId(1), NpcId(2));
        assert_eq!(e.affection, Edge::AFFECTION_MAX);
        assert_eq!(e.trust, Edge::TRUST_MIN);
        assert_eq!(e.grievance, Edge::GRIEVANCE_MAX);
    }

    #[test]
    fn directed_edges_are_independent() {
        let mut w = world_with_two();
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta(10, 0, 0, 0));
        let one_two = w.relationship(NpcId(1), NpcId(2));
        let two_one = w.relationship(NpcId(2), NpcId(1));
        assert_eq!(one_two.affection, 10);
        assert_eq!(two_one.affection, 0);
    }

    #[test]
    fn grievance_decay_floor_zero() {
        let mut w = world_with_two();
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta(0, 0, 0, 5));
        let mut r = rng();
        for _ in 0..1000 {
            tick(&mut w, t(), &mut r);
        }
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).grievance, 0);
    }

    #[test]
    fn grievance_decay_monotonic_downward() {
        let mut w = world_with_two();
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta(0, 0, 0, 100));
        let mut r = rng();
        let mut prev = 100;
        for _ in 0..200 {
            tick(&mut w, t(), &mut r);
            let g = w.relationship(NpcId(1), NpcId(2)).grievance;
            assert!(g <= prev);
            prev = g;
        }
    }

    #[test]
    fn other_axes_do_not_decay() {
        let mut w = world_with_two();
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta(40, 30, 20, 100));
        let mut r = rng();
        for _ in 0..50 {
            tick(&mut w, t(), &mut r);
        }
        let e = w.relationship(NpcId(1), NpcId(2));
        assert_eq!(e.affection, 40);
        assert_eq!(e.trust, 30);
        assert_eq!(e.debt, 20);
        // grievance moved
        assert!(e.grievance < 100);
    }

    #[test]
    fn attraction_accumulates_and_clamps() {
        let mut w = world_with_two();
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta_attr(40));
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta_attr(80));
        assert_eq!(
            w.relationship(NpcId(1), NpcId(2)).attraction,
            Edge::ATTRACTION_MAX
        );
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta_attr(-300));
        assert_eq!(
            w.relationship(NpcId(1), NpcId(2)).attraction,
            Edge::ATTRACTION_MIN
        );
    }

    #[test]
    fn attraction_is_independent_of_affection() {
        let mut w = world_with_two();
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta_attr(20));
        let e = w.relationship(NpcId(1), NpcId(2));
        assert_eq!(e.attraction, 20);
        assert_eq!(e.affection, 0);
    }

    #[test]
    fn relationships_from_returns_only_subject_rows() {
        let mut w = world_with_two();
        w.insert_npc(Npc::new(NpcId(3), "c"));
        apply_relationship_shift(&mut w, NpcId(1), NpcId(2), &delta(10, 0, 0, 0));
        apply_relationship_shift(&mut w, NpcId(1), NpcId(3), &delta(20, 0, 0, 0));
        apply_relationship_shift(&mut w, NpcId(2), NpcId(3), &delta(5, 0, 0, 0));
        let from_one: Vec<(NpcId, i32)> = w
            .relationships_from(NpcId(1))
            .map(|(o, e)| (o, e.affection))
            .collect();
        assert_eq!(from_one, vec![(NpcId(2), 10), (NpcId(3), 20)]);
    }
}
