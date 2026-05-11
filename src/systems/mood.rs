use std::collections::VecDeque;

use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::engine::{Distortion, EdgeDelta, Outcome, Time};
use crate::world::{Npc, World};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mood {
    pub valence: f32,
    pub arousal: f32,
}

impl Default for Mood {
    fn default() -> Self {
        Self {
            valence: 0.0,
            arousal: 0.2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoodImprint {
    pub valence: f32,
    pub weight: f32,
    pub at: Time,
}

pub const MAX_IMPRINTS: usize = 16;
pub const IMPRINT_WEIGHT_DECAY: f32 = 0.92;
pub const IMPRINT_DROP_THRESHOLD: f32 = 0.01;
const AROUSAL_RELAX: f32 = 0.02;
const AROUSAL_BASELINE: f32 = 0.2;

/// Per-tick mood decay for every NPC. Decays each imprint's weight, drops
/// imprints under threshold, and recomputes `mood.valence` as the weighted
/// mean of remaining imprints. Arousal relaxes toward its baseline.
pub fn tick(world: &mut World, _now: Time, _rng: &mut ChaCha8Rng) {
    for npc in world.npcs.values_mut() {
        for imp in npc.mood_imprints.iter_mut() {
            imp.weight *= IMPRINT_WEIGHT_DECAY;
        }
        npc.mood_imprints
            .retain(|i| i.weight >= IMPRINT_DROP_THRESHOLD);
        npc.mood.valence = weighted_valence(&npc.mood_imprints);
        if npc.mood.arousal > AROUSAL_BASELINE {
            npc.mood.arousal = (npc.mood.arousal - AROUSAL_RELAX).max(AROUSAL_BASELINE);
        } else {
            npc.mood.arousal = (npc.mood.arousal + AROUSAL_RELAX).min(AROUSAL_BASELINE);
        }
    }
}

pub(crate) fn push_imprint(npc: &mut Npc, valence: f32, weight: f32, now: Time) {
    let valence = valence.clamp(-1.0, 1.0);
    let weight = weight.clamp(0.0, 1.0);
    npc.mood_imprints.push_back(MoodImprint {
        valence,
        weight,
        at: now,
    });
    while npc.mood_imprints.len() > MAX_IMPRINTS {
        npc.mood_imprints.pop_front();
    }
    // Refresh live mood so callers see the imprint reflected immediately.
    npc.mood.valence = weighted_valence(&npc.mood_imprints);
    // Arousal nudges toward 1.0 with significant imprints.
    npc.mood.arousal = (npc.mood.arousal + valence.abs() * 0.3).min(1.0);
}

fn weighted_valence(buf: &VecDeque<MoodImprint>) -> f32 {
    if buf.is_empty() {
        return 0.0;
    }
    let mut num = 0.0;
    let mut den = 0.0;
    for i in buf {
        num += i.valence * i.weight;
        den += i.weight;
    }
    if den < f32::EPSILON {
        0.0
    } else {
        (num / den).clamp(-1.0, 1.0)
    }
}

/// Heuristic mapping from a fact's `kind` to an emotional valence.
/// Used by [`crate::systems::apply_event`] when an NPC witnesses or is told a fact.
pub fn valence_for_fact_kind(kind: &str) -> f32 {
    match kind.to_ascii_lowercase().as_str() {
        "theft" => -0.4,
        "murder" => -0.9,
        "death" => -0.7,
        "lie" | "betrayal" => -0.6,
        "scandal" | "affair" => -0.3,
        "rumor" => -0.1,
        "gift" => 0.4,
        "favor" => 0.3,
        "marriage" | "birth" => 0.6,
        "victory" => 0.5,
        _ => 0.0,
    }
}

pub(crate) fn imprint_witnessed(npc: &mut Npc, kind: &str, now: Time) {
    let v = valence_for_fact_kind(kind);
    if v.abs() < f32::EPSILON {
        return;
    }
    push_imprint(npc, v, 1.0, now);
}

pub(crate) fn imprint_told(npc: &mut Npc, kind: &str, distortion: Distortion, now: Time) {
    let v = valence_for_fact_kind(kind);
    if v.abs() < f32::EPSILON {
        return;
    }
    let scale = 0.5 * (1.0 - distortion.level as f32 / 4.0).max(0.0);
    push_imprint(npc, v, scale.clamp(0.0, 1.0), now);
}

pub(crate) fn imprint_relationship_shift(npc: &mut Npc, delta: &EdgeDelta, now: Time) {
    let signal = (delta.affection + delta.trust) as f32 / 200.0;
    if signal.abs() < f32::EPSILON {
        return;
    }
    let valence = signal.clamp(-1.0, 1.0);
    push_imprint(npc, valence, valence.abs().min(1.0).max(0.1), now);
}

pub(crate) fn imprint_goal_resolved(npc: &mut Npc, outcome: &Outcome, now: Time) {
    let v = match outcome {
        Outcome::Achieved => 0.4,
        Outcome::Frustrated => -0.4,
        Outcome::Abandoned => -0.1,
    };
    push_imprint(npc, v, 0.6, now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::NpcId;
    use rand::SeedableRng;

    fn t(min: u32) -> Time {
        Time { day: 0, minute: min }
    }

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0)
    }

    #[test]
    fn imprint_decays_under_threshold_in_finite_ticks() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        push_imprint(w.npcs.get_mut(&NpcId(1)).unwrap(), 1.0, 1.0, t(0));
        let mut r = rng();
        for _ in 0..200 {
            tick(&mut w, t(0), &mut r);
        }
        assert!(w.npc(NpcId(1)).unwrap().mood_imprints.is_empty());
        assert_eq!(w.npc(NpcId(1)).unwrap().mood.valence, 0.0);
    }

    #[test]
    fn valence_stays_in_range_under_mixed_events() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        let mut r = rng();
        for i in 0..50 {
            let v = if i % 2 == 0 { 0.8 } else { -0.7 };
            push_imprint(w.npcs.get_mut(&NpcId(1)).unwrap(), v, 1.0, t(i));
            tick(&mut w, t(i), &mut r);
            let val = w.npc(NpcId(1)).unwrap().mood.valence;
            assert!((-1.0..=1.0).contains(&val));
        }
    }

    #[test]
    fn told_imprint_weaker_than_witnessed_for_same_fact_kind() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "watcher"));
        w.insert_npc(Npc::new(NpcId(2), "listener"));
        imprint_witnessed(w.npcs.get_mut(&NpcId(1)).unwrap(), "theft", t(0));
        imprint_told(
            w.npcs.get_mut(&NpcId(2)).unwrap(),
            "theft",
            Distortion { level: 0 },
            t(0),
        );
        let w_val = w.npc(NpcId(1)).unwrap().mood.valence.abs();
        let t_val = w.npc(NpcId(2)).unwrap().mood.valence.abs();
        // Same valence (told absent distortion mirrors witnessed direction), but
        // the weight on the told imprint is halved, so the imprint magnitude is
        // smaller — confirm by checking the imprint buffers, not the normalized
        // weighted-mean (which would be equal since there's only one imprint).
        assert_eq!(w_val, t_val);
        let witnessed_weight = w.npc(NpcId(1)).unwrap().mood_imprints[0].weight;
        let told_weight = w.npc(NpcId(2)).unwrap().mood_imprints[0].weight;
        assert!(told_weight < witnessed_weight);
    }

    #[test]
    fn imprint_buffer_caps_at_max() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        for i in 0..(MAX_IMPRINTS + 5) {
            push_imprint(
                w.npcs.get_mut(&NpcId(1)).unwrap(),
                0.1,
                1.0,
                t(i as u32),
            );
        }
        assert_eq!(w.npc(NpcId(1)).unwrap().mood_imprints.len(), MAX_IMPRINTS);
    }

    #[test]
    fn goal_resolved_imprint_valence_signed_correctly() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        imprint_goal_resolved(
            w.npcs.get_mut(&NpcId(1)).unwrap(),
            &Outcome::Achieved,
            t(0),
        );
        assert!(w.npc(NpcId(1)).unwrap().mood.valence > 0.0);

        let mut w2 = World::new(0);
        w2.insert_npc(Npc::new(NpcId(1), "a"));
        imprint_goal_resolved(
            w2.npcs.get_mut(&NpcId(1)).unwrap(),
            &Outcome::Frustrated,
            t(0),
        );
        assert!(w2.npc(NpcId(1)).unwrap().mood.valence < 0.0);
    }

    #[test]
    fn relationship_shift_zero_delta_is_noop() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        imprint_relationship_shift(
            w.npcs.get_mut(&NpcId(1)).unwrap(),
            &EdgeDelta {
                affection: 0,
                trust: 0,
                debt: 0,
                grievance: 0,
                attraction: 0,
            },
            t(0),
        );
        assert!(w.npc(NpcId(1)).unwrap().mood_imprints.is_empty());
    }
}
