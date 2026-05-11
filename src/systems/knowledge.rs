use std::collections::BTreeMap;

use rand::Rng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::engine::{Distortion, Event, Time};
use crate::world::{FactId, LocationId, NpcId, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnowledgeSource {
    Witnessed,
    ToldBy { teller: NpcId },
    Inferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeEdge {
    pub source: KnowledgeSource,
    pub confidence: f32,
    pub distortion: Distortion,
    pub acquired_at: Time,
}

pub const WITNESSED_CONFIDENCE: f32 = 0.9;
const GOSSIPY_TRAIT: &str = "Gossipy";
const DEFAULT_GOSSIP_P: f32 = 0.08;
const GOSSIPY_GOSSIP_P: f32 = 0.25;
const SECRETIVE_TRAIT: &str = "Secretive";
const SECRETIVE_GOSSIP_P: f32 = 0.01;
const UNTRUSTWORTHY_TRAIT: &str = "Untrustworthy";
const BOASTFUL_TRAIT: &str = "Boastful";
const DISTORTION_CAP: u8 = 4;

/// Record that `observer` directly witnessed `fact`. Idempotent in the sense
/// that re-witnessing does not lower an existing confidence.
pub(crate) fn apply_witnessed(world: &mut World, observer: NpcId, fact: FactId, when: Time) {
    let Some(npc) = world.npcs.get_mut(&observer) else {
        return;
    };
    let edge = KnowledgeEdge {
        source: KnowledgeSource::Witnessed,
        confidence: WITNESSED_CONFIDENCE,
        distortion: Distortion { level: 0 },
        acquired_at: when,
    };
    merge_in(&mut npc.knowledge, fact, edge);
}

/// Record that `listener` was told `fact` by `teller`. Confidence degrades
/// with distortion.
pub(crate) fn apply_told(
    world: &mut World,
    teller: NpcId,
    listener: NpcId,
    fact: FactId,
    distortion: Distortion,
    when: Time,
) {
    let Some(npc) = world.npcs.get_mut(&listener) else {
        return;
    };
    let base = WITNESSED_CONFIDENCE * (1.0 - distortion.level as f32 / 8.0).max(0.0);
    let edge = KnowledgeEdge {
        source: KnowledgeSource::ToldBy { teller },
        confidence: base.clamp(0.0, 1.0),
        distortion,
        acquired_at: when,
    };
    merge_in(&mut npc.knowledge, fact, edge);
}

fn merge_in(map: &mut BTreeMap<FactId, KnowledgeEdge>, fact: FactId, incoming: KnowledgeEdge) {
    use std::collections::btree_map::Entry;
    match map.entry(fact) {
        Entry::Vacant(v) => {
            v.insert(KnowledgeEdge {
                confidence: incoming.confidence.clamp(0.0, 1.0),
                ..incoming
            });
        }
        Entry::Occupied(mut o) => {
            let existing = o.get_mut();
            // Take the stronger confidence with a small pull toward the new value.
            let new_conf = if incoming.confidence > existing.confidence {
                (0.7 * incoming.confidence + 0.3 * existing.confidence).clamp(0.0, 1.0)
            } else {
                (0.7 * existing.confidence + 0.3 * incoming.confidence).clamp(0.0, 1.0)
            };
            existing.confidence = new_conf;
            // Distortion only goes up (knowledge does not self-clarify).
            if incoming.distortion.level > existing.distortion.level {
                existing.distortion = incoming.distortion;
            }
        }
    }
}

/// Rumor-propagation tick. Two-phase: (1) collect proposed Told events from
/// NPCs co-located with someone who knows a fact they don't; (2) apply them.
/// Returns the produced events for the caller to append to the event log.
pub fn tick_propagation(
    world: &mut World,
    now: Time,
    rng: &mut ChaCha8Rng,
) -> Vec<Event> {
    let mut by_loc: BTreeMap<LocationId, Vec<NpcId>> = BTreeMap::new();
    for npc in world.npcs.values() {
        if let Some(loc) = npc.location {
            by_loc.entry(loc).or_default().push(npc.id);
        }
    }

    let mut proposed: Vec<Event> = Vec::new();
    for (_loc, ids) in &by_loc {
        if ids.len() < 2 {
            continue;
        }
        for i in 0..ids.len() {
            for j in 0..ids.len() {
                if i == j {
                    continue;
                }
                let a = ids[i];
                let b = ids[j];
                let p = gossip_probability(world, a);
                if rng.r#gen::<f32>() >= p {
                    continue;
                }
                // Pick a fact `a` knows that `b` does not (or knows weaker).
                let Some((fact, teller_edge)) = pick_transmissible_fact(world, a, b, rng) else {
                    continue;
                };
                let new_level =
                    bump_distortion(teller_edge.distortion.level, world, a);
                let distortion = Distortion {
                    level: new_level.min(DISTORTION_CAP),
                };
                proposed.push(Event::Told {
                    teller: a,
                    listener: b,
                    fact,
                    distortion,
                    when: now,
                });
            }
        }
    }

    for ev in &proposed {
        if let Event::Told {
            teller,
            listener,
            fact,
            distortion,
            when,
        } = ev
        {
            apply_told(world, *teller, *listener, *fact, *distortion, *when);
        }
    }

    proposed
}

fn gossip_probability(world: &World, who: NpcId) -> f32 {
    let Some(npc) = world.npc(who) else {
        return 0.0;
    };
    if npc.traits.has(SECRETIVE_TRAIT) {
        SECRETIVE_GOSSIP_P
    } else if npc.traits.has(GOSSIPY_TRAIT) {
        GOSSIPY_GOSSIP_P
    } else {
        DEFAULT_GOSSIP_P
    }
}

fn bump_distortion(prev: u8, world: &World, teller: NpcId) -> u8 {
    let mut bump = 1u8;
    if let Some(t) = world.npc(teller) {
        if t.traits.has(UNTRUSTWORTHY_TRAIT) {
            bump = bump.saturating_add(1);
        }
        if t.traits.has(BOASTFUL_TRAIT) {
            bump = bump.saturating_add(1);
        }
    }
    prev.saturating_add(bump).min(DISTORTION_CAP)
}

fn pick_transmissible_fact(
    world: &World,
    teller: NpcId,
    listener: NpcId,
    rng: &mut ChaCha8Rng,
) -> Option<(FactId, KnowledgeEdge)> {
    let t = world.npc(teller)?;
    let l = world.npc(listener)?;
    let mut candidates: Vec<(FactId, KnowledgeEdge)> = Vec::new();
    for (fact, edge) in &t.knowledge {
        let stronger = match l.knowledge.get(fact) {
            None => true,
            Some(existing) => edge.confidence > existing.confidence + 0.05,
        };
        if stronger {
            candidates.push((*fact, *edge));
        }
    }
    candidates.shuffle(rng);
    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Fact;
    use crate::world::{LocationId, Npc};
    use rand::SeedableRng;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(7)
    }

    fn t() -> Time {
        Time { day: 0, minute: 0 }
    }

    fn fact_id(world: &mut World, kind: &str, summary: &str) -> FactId {
        world.intern_fact(Fact {
            kind: kind.into(),
            summary: summary.into(),
        })
    }

    #[test]
    fn apply_witnessed_records_edge() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        let f = fact_id(&mut w, "theft", "x");
        apply_witnessed(&mut w, NpcId(1), f, t());
        let edge = w.npc(NpcId(1)).unwrap().knowledge.get(&f).unwrap();
        assert!(matches!(edge.source, KnowledgeSource::Witnessed));
        assert_eq!(edge.confidence, WITNESSED_CONFIDENCE);
        assert_eq!(edge.distortion.level, 0);
        assert_eq!(edge.acquired_at, t());
    }

    #[test]
    fn apply_told_records_source_and_distortion() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "teller"));
        w.insert_npc(Npc::new(NpcId(2), "listener"));
        let f = fact_id(&mut w, "rumor", "x");
        apply_told(&mut w, NpcId(1), NpcId(2), f, Distortion { level: 2 }, t());
        let edge = w.npc(NpcId(2)).unwrap().knowledge.get(&f).unwrap();
        assert_eq!(edge.source, KnowledgeSource::ToldBy { teller: NpcId(1) });
        assert!(edge.confidence > 0.0 && edge.confidence < WITNESSED_CONFIDENCE);
        assert_eq!(edge.distortion.level, 2);
    }

    #[test]
    fn confidence_stays_in_unit_range_under_extreme_distortion() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        w.insert_npc(Npc::new(NpcId(2), "b"));
        let f = fact_id(&mut w, "rumor", "x");
        for level in 0..=10 {
            apply_told(
                &mut w,
                NpcId(1),
                NpcId(2),
                f,
                Distortion { level },
                t(),
            );
            let c = w.npc(NpcId(2)).unwrap().knowledge.get(&f).unwrap().confidence;
            assert!((0.0..=1.0).contains(&c), "confidence out of range: {}", c);
        }
    }

    #[test]
    fn propagation_requires_proximity() {
        let mut w = World::new(0);
        let mut a = Npc::new(NpcId(1), "a");
        a.location = Some(LocationId(1));
        a.traits.virtues.push("Gossipy".into());
        let mut b = Npc::new(NpcId(2), "b");
        b.location = Some(LocationId(2)); // different location
        w.insert_npc(a);
        w.insert_npc(b);

        let f = fact_id(&mut w, "rumor", "x");
        apply_witnessed(&mut w, NpcId(1), f, t());

        let mut r = rng();
        for _ in 0..200 {
            let _ = tick_propagation(&mut w, t(), &mut r);
        }
        assert!(
            w.npc(NpcId(2)).unwrap().knowledge.get(&f).is_none(),
            "fact should not propagate across locations"
        );
    }

    #[test]
    fn propagation_succeeds_in_same_location_eventually() {
        let mut w = World::new(0);
        let mut a = Npc::new(NpcId(1), "a");
        a.location = Some(LocationId(1));
        a.traits.virtues.push("Gossipy".into());
        let mut b = Npc::new(NpcId(2), "b");
        b.location = Some(LocationId(1));
        w.insert_npc(a);
        w.insert_npc(b);

        let f = fact_id(&mut w, "rumor", "x");
        apply_witnessed(&mut w, NpcId(1), f, t());

        let mut r = rng();
        let mut transmitted = false;
        for _ in 0..200 {
            let _ = tick_propagation(&mut w, t(), &mut r);
            if w.npc(NpcId(2)).unwrap().knowledge.get(&f).is_some() {
                transmitted = true;
                break;
            }
        }
        assert!(transmitted, "rumor should propagate within 200 ticks");
    }

    #[test]
    fn distortion_monotone_increases_per_hop() {
        let mut w = World::new(0);
        for (id, name) in [(1u32, "a"), (2, "b"), (3, "c")] {
            let mut npc = Npc::new(NpcId(id), name);
            npc.location = Some(LocationId(1));
            npc.traits.virtues.push("Gossipy".into());
            w.insert_npc(npc);
        }
        let f = fact_id(&mut w, "rumor", "x");
        apply_witnessed(&mut w, NpcId(1), f, t());

        let mut r = rng();
        // Run propagation enough ticks for the fact to chain.
        for _ in 0..500 {
            let _ = tick_propagation(&mut w, t(), &mut r);
        }
        // Whoever has acquired the fact via Told must have distortion ≥ 1.
        for id in [NpcId(2), NpcId(3)] {
            if let Some(edge) = w.npc(id).unwrap().knowledge.get(&f)
                && matches!(edge.source, KnowledgeSource::ToldBy { .. })
            {
                assert!(edge.distortion.level >= 1);
            }
        }
    }

    #[test]
    fn merge_does_not_lower_existing_confidence() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        let f = fact_id(&mut w, "rumor", "x");
        // First: direct witness (high confidence).
        apply_witnessed(&mut w, NpcId(1), f, t());
        let high = w.npc(NpcId(1)).unwrap().knowledge.get(&f).unwrap().confidence;
        // Then: someone tells them, with distortion (lower confidence).
        apply_told(
            &mut w,
            NpcId(2),
            NpcId(1),
            f,
            Distortion { level: 3 },
            t(),
        );
        let after = w.npc(NpcId(1)).unwrap().knowledge.get(&f).unwrap().confidence;
        assert!(after >= 0.7 * high - 0.01, "confidence regressed: {} -> {}", high, after);
    }
}
