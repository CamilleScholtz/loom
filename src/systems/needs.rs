use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::engine::Time;
use crate::world::World;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Needs {
    pub hunger: f32,
    pub sleep: f32,
    pub safety: f32,
    pub belonging: f32,
    pub purpose: f32,
}

impl Default for Needs {
    fn default() -> Self {
        Self {
            hunger: 0.2,
            sleep: 0.2,
            safety: 0.1,
            belonging: 0.2,
            purpose: 0.2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NeedKind {
    Hunger,
    Sleep,
    Safety,
    Belonging,
    Purpose,
}

impl Needs {
    pub fn get(&self, k: NeedKind) -> f32 {
        match k {
            NeedKind::Hunger => self.hunger,
            NeedKind::Sleep => self.sleep,
            NeedKind::Safety => self.safety,
            NeedKind::Belonging => self.belonging,
            NeedKind::Purpose => self.purpose,
        }
    }

    pub fn set(&mut self, k: NeedKind, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match k {
            NeedKind::Hunger => self.hunger = v,
            NeedKind::Sleep => self.sleep = v,
            NeedKind::Safety => self.safety = v,
            NeedKind::Belonging => self.belonging = v,
            NeedKind::Purpose => self.purpose = v,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NeedDecayRates {
    pub hunger: f32,
    pub sleep: f32,
    pub safety: f32,
    pub belonging: f32,
    pub purpose: f32,
}

impl Default for NeedDecayRates {
    fn default() -> Self {
        Self {
            hunger: 0.02,
            sleep: 0.01,
            safety: 0.001,
            belonging: 0.005,
            purpose: 0.003,
        }
    }
}

/// Advance the needs of every NPC by one tick. Called by the simulation
/// orchestrator (Phase 5+); not meant to be invoked manually mid-step.
pub fn tick(world: &mut World, _now: Time, _rng: &mut ChaCha8Rng) {
    let rates = NeedDecayRates::default();
    for npc in world.npcs.values_mut() {
        npc.needs.hunger = (npc.needs.hunger + rates.hunger).clamp(0.0, 1.0);
        npc.needs.sleep = (npc.needs.sleep + rates.sleep).clamp(0.0, 1.0);
        npc.needs.safety = (npc.needs.safety + rates.safety).clamp(0.0, 1.0);
        npc.needs.belonging = (npc.needs.belonging + rates.belonging).clamp(0.0, 1.0);
        npc.needs.purpose = (npc.needs.purpose + rates.purpose).clamp(0.0, 1.0);
    }
}

/// Satisfy a need by `amount` (positive). Clamps to `[0, 1]`. Routine event;
/// not logged per TECH.md "Routine ticks are imperative, unlogged."
pub fn satisfy(npc: &mut crate::world::Npc, need: NeedKind, amount: f32) {
    let current = npc.needs.get(need);
    npc.needs.set(need, current - amount.max(0.0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Npc, NpcId};
    use rand::SeedableRng;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0)
    }

    fn t() -> Time {
        Time { day: 0, minute: 0 }
    }

    #[test]
    fn decay_monotonic_and_clamped() {
        let mut world = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs = Needs {
            hunger: 0.0,
            sleep: 0.0,
            safety: 0.0,
            belonging: 0.0,
            purpose: 0.0,
        };
        world.insert_npc(npc);
        let mut r = rng();

        let mut prev = world.npc(NpcId(1)).unwrap().needs;
        for _ in 0..200 {
            tick(&mut world, t(), &mut r);
            let cur = world.npc(NpcId(1)).unwrap().needs;
            assert!(cur.hunger >= prev.hunger - f32::EPSILON);
            assert!(cur.sleep >= prev.sleep - f32::EPSILON);
            assert!(cur.safety >= prev.safety - f32::EPSILON);
            assert!(cur.belonging >= prev.belonging - f32::EPSILON);
            assert!(cur.purpose >= prev.purpose - f32::EPSILON);
            assert!(cur.hunger <= 1.0);
            assert!(cur.sleep <= 1.0);
            assert!(cur.safety <= 1.0);
            assert!(cur.belonging <= 1.0);
            assert!(cur.purpose <= 1.0);
            prev = cur;
        }
    }

    #[test]
    fn satisfy_clamps_to_zero() {
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs.hunger = 0.3;
        satisfy(&mut npc, NeedKind::Hunger, 1.0);
        assert_eq!(npc.needs.hunger, 0.0);
    }

    #[test]
    fn satisfy_negative_amount_is_noop() {
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs.hunger = 0.5;
        satisfy(&mut npc, NeedKind::Hunger, -10.0);
        assert_eq!(npc.needs.hunger, 0.5);
    }

    #[test]
    fn needs_get_set_roundtrip() {
        let mut needs = Needs::default();
        needs.set(NeedKind::Hunger, 0.7);
        assert_eq!(needs.get(NeedKind::Hunger), 0.7);
        needs.set(NeedKind::Safety, 1.5);
        assert_eq!(needs.get(NeedKind::Safety), 1.0);
        needs.set(NeedKind::Sleep, -0.2);
        assert_eq!(needs.get(NeedKind::Sleep), 0.0);
    }
}
