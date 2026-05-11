use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::engine::Time;
use crate::world::{Npc, NpcId, World};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reputation {
    pub honor: f32,
    pub menace: f32,
    pub competence: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RepAxis {
    Honor,
    Menace,
    Competence,
}

const DECAY: f32 = 0.999;

/// Apply slow decay of every NPC's reputation toward zero. Each axis is bounded:
/// honor ∈ [-1, 1], menace ∈ [0, 1], competence ∈ [0, 1].
pub fn tick(world: &mut World, _now: Time, _rng: &mut ChaCha8Rng) {
    for npc in world.npcs.values_mut() {
        npc.reputation.honor = clamp_signed(npc.reputation.honor * DECAY);
        npc.reputation.menace = clamp_unsigned(npc.reputation.menace * DECAY);
        npc.reputation.competence = clamp_unsigned(npc.reputation.competence * DECAY);
    }
}

/// Push a reputation axis by `amount`. Use this when an action — typically
/// witnessed by ≥2 NPCs — should update a public summary. Bypasses no
/// invariants because reputation is fully derived state with simple bounds.
pub(crate) fn bump(npc: &mut Npc, axis: RepAxis, amount: f32) {
    match axis {
        RepAxis::Honor => npc.reputation.honor = clamp_signed(npc.reputation.honor + amount),
        RepAxis::Menace => npc.reputation.menace = clamp_unsigned(npc.reputation.menace + amount),
        RepAxis::Competence => {
            npc.reputation.competence = clamp_unsigned(npc.reputation.competence + amount)
        }
    }
}

/// Convenience wrapper that bumps a reputation axis by NpcId. No-op if absent.
pub(crate) fn bump_by_id(world: &mut World, who: NpcId, axis: RepAxis, amount: f32) {
    if let Some(npc) = world.npcs.get_mut(&who) {
        bump(npc, axis, amount);
    }
}

fn clamp_signed(v: f32) -> f32 {
    v.clamp(-1.0, 1.0)
}

fn clamp_unsigned(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0)
    }

    fn t() -> Time {
        Time { day: 0, minute: 0 }
    }

    #[test]
    fn decay_pulls_honor_toward_zero() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.reputation.honor = 0.8;
        npc.reputation.menace = 0.5;
        npc.reputation.competence = 0.5;
        w.insert_npc(npc);
        let mut r = rng();
        for _ in 0..5000 {
            tick(&mut w, t(), &mut r);
        }
        let rep = w.npc(NpcId(1)).unwrap().reputation;
        assert!(rep.honor.abs() < 0.05);
        assert!(rep.menace < 0.05);
        assert!(rep.competence < 0.05);
    }

    #[test]
    fn decay_preserves_sign_of_honor() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.reputation.honor = -0.6;
        w.insert_npc(npc);
        let mut r = rng();
        for _ in 0..100 {
            tick(&mut w, t(), &mut r);
            assert!(w.npc(NpcId(1)).unwrap().reputation.honor <= 0.0);
        }
    }

    #[test]
    fn bump_respects_bounds() {
        let mut npc = Npc::new(NpcId(1), "a");
        bump(&mut npc, RepAxis::Honor, 5.0);
        assert_eq!(npc.reputation.honor, 1.0);
        bump(&mut npc, RepAxis::Honor, -5.0);
        assert_eq!(npc.reputation.honor, -1.0);

        let mut npc = Npc::new(NpcId(1), "a");
        bump(&mut npc, RepAxis::Menace, -2.0);
        assert_eq!(npc.reputation.menace, 0.0);
        bump(&mut npc, RepAxis::Menace, 2.0);
        assert_eq!(npc.reputation.menace, 1.0);
    }

    #[test]
    fn bump_by_id_is_noop_for_missing_npc() {
        let mut w = World::new(0);
        // Should not panic.
        bump_by_id(&mut w, NpcId(99), RepAxis::Honor, 0.2);
    }
}
