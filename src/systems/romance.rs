//! Courtship stage state machine.
//!
//! Each directed `Edge` carries a `RomanceStage` and the `Time` it entered
//! that stage. `tick` walks every edge once per day and applies any
//! transitions a pure function of the edge values + the reciprocal edge
//! produces. Transitions emit `Event::Remembered` for both subjects so the
//! event log and `npc.memorable_events` carry a narrative trace.
//!
//! The LLM never decides stages; the engine derives them from the axes the
//! LLM is allowed to nudge (attraction, affection, trust, grievance) and from
//! witnessed-betrayal facts in the knowledge graph.

use rand_chacha::ChaCha8Rng;

use crate::engine::{Event, Time};
use crate::systems::relationships::{Edge, RomanceStage};
use crate::world::{MemorableEvent, NpcId, World};

const INTERESTED_ATTRACTION: i32 = 20;
const COURTING_AFFECTION: i32 = 30;
const COURTING_ATTRACTION: i32 = 30;
const COMMITTED_DAYS: u32 = 60;
const ESTRANGED_GRIEVANCE: i32 = 60;

/// Compute the stage `subject` should be in toward `object` given the
/// directed edge `self_edge` and the reciprocal `other_edge`, evaluated at
/// `now`. Returns the new stage; the caller compares against the current
/// stage to detect a transition.
pub fn next_stage(self_edge: Edge, other_edge: Edge, now: Time) -> RomanceStage {
    // Estrangement dominates: a grievance spike collapses any prior stage.
    if self_edge.grievance >= ESTRANGED_GRIEVANCE
        && !matches!(self_edge.stage, RomanceStage::Strangers)
    {
        return RomanceStage::Estranged;
    }

    match self_edge.stage {
        // A fully grieved Strangers edge stays Strangers; estrangement only
        // applies after some relationship existed.
        RomanceStage::Strangers => {
            if self_edge.attraction >= INTERESTED_ATTRACTION {
                RomanceStage::Interested
            } else {
                RomanceStage::Strangers
            }
        }
        RomanceStage::Interested => {
            let mutual = matches!(
                other_edge.stage,
                RomanceStage::Interested
                    | RomanceStage::Courting
                    | RomanceStage::Committed
            );
            if mutual
                && self_edge.attraction >= COURTING_ATTRACTION
                && self_edge.affection >= COURTING_AFFECTION
            {
                RomanceStage::Courting
            } else if self_edge.attraction < INTERESTED_ATTRACTION / 2 {
                // Lost interest entirely.
                RomanceStage::Strangers
            } else {
                RomanceStage::Interested
            }
        }
        RomanceStage::Courting => {
            let sustained = now.day.saturating_sub(self_edge.stage_since.day) >= COMMITTED_DAYS;
            let both_high = self_edge.attraction >= COURTING_ATTRACTION
                && self_edge.affection >= COURTING_AFFECTION;
            if sustained && both_high {
                RomanceStage::Committed
            } else if !both_high {
                RomanceStage::Interested
            } else {
                RomanceStage::Courting
            }
        }
        RomanceStage::Committed => {
            // Stays committed unless grievance threshold (already handled
            // above) takes it to Estranged.
            RomanceStage::Committed
        }
        RomanceStage::Estranged => {
            // Repair is possible: very high affection + low grievance pulls
            // back to Interested. Otherwise the stage sticks.
            if self_edge.grievance < ESTRANGED_GRIEVANCE / 3
                && self_edge.affection >= COURTING_AFFECTION
            {
                RomanceStage::Interested
            } else {
                RomanceStage::Estranged
            }
        }
    }
}

/// Daily romance tick. Walks every directed edge once, computes the next
/// stage, and on a change writes back `stage`/`stage_since` and returns an
/// `Event::Remembered` for the subject (so dialogue can later read "we became
/// Courting on day 30" via `memorable_events`).
pub fn tick(world: &mut World, now: Time, _rng: &mut ChaCha8Rng) -> Vec<Event> {
    // Snapshot keys + edges first to avoid double-borrow with the reciprocal
    // edge lookup.
    let snapshot: Vec<((NpcId, NpcId), Edge, Edge)> = world
        .relationships_iter()
        .map(|((s, o), e)| ((*s, *o), *e, world.relationship(*o, *s)))
        .collect();

    let mut events: Vec<Event> = Vec::new();
    for ((subject, object), self_edge, other_edge) in snapshot {
        let new_stage = next_stage(self_edge, other_edge, now);
        if new_stage == self_edge.stage {
            continue;
        }
        let object_name = world
            .npc(object)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "another".into());
        let summary = transition_summary(self_edge.stage, new_stage, &object_name);
        let valence = transition_valence(new_stage);

        crate::systems::relationships::set_stage(world, subject, object, new_stage, now);

        events.push(Event::Remembered {
            holder: subject,
            event: MemorableEvent {
                when: now,
                summary,
                valence,
                related_to: Some(object),
                fact: None,
            },
            when: now,
        });
    }
    events
}

fn transition_summary(from: RomanceStage, to: RomanceStage, other: &str) -> String {
    match (from, to) {
        (_, RomanceStage::Interested) => format!("found myself drawn to {}", other),
        (_, RomanceStage::Courting) => format!("began courting {}", other),
        (_, RomanceStage::Committed) => format!("our bond with {} settled into something lasting", other),
        (_, RomanceStage::Estranged) => format!("turned cold against {}", other),
        (_, RomanceStage::Strangers) => format!("my feelings for {} cooled away", other),
    }
}

fn transition_valence(to: RomanceStage) -> f32 {
    match to {
        RomanceStage::Interested => 0.3,
        RomanceStage::Courting => 0.6,
        RomanceStage::Committed => 0.8,
        RomanceStage::Estranged => -0.7,
        RomanceStage::Strangers => -0.1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EdgeDelta;
    use crate::systems::relationships::apply_relationship_shift;
    use crate::world::Npc;
    use rand::SeedableRng;

    fn t(day: u32) -> Time {
        Time { day, minute: 480 }
    }

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0)
    }

    fn world_with_pair() -> World {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "Aldwin"));
        w.insert_npc(Npc::new(NpcId(2), "Edith"));
        w
    }

    fn shift_pair(world: &mut World, subject: NpcId, object: NpcId, delta: EdgeDelta) {
        apply_relationship_shift(world, subject, object, &delta);
    }

    #[test]
    fn strangers_advance_to_interested_on_attraction_alone() {
        let mut w = world_with_pair();
        shift_pair(
            &mut w,
            NpcId(1),
            NpcId(2),
            EdgeDelta {
                attraction: 25,
                ..EdgeDelta::default()
            },
        );
        let events = tick(&mut w, t(1), &mut rng());
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Remembered { holder, .. } if *holder == NpcId(1)
        )));
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).stage, RomanceStage::Interested);
    }

    #[test]
    fn interested_advances_to_courting_only_with_reciprocal() {
        let mut w = world_with_pair();
        // A interested in B (one-sided high attraction + affection).
        shift_pair(
            &mut w,
            NpcId(1),
            NpcId(2),
            EdgeDelta {
                attraction: 40,
                affection: 40,
                ..EdgeDelta::default()
            },
        );
        // First tick: A goes Strangers → Interested.
        let _ = tick(&mut w, t(1), &mut rng());
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).stage, RomanceStage::Interested);
        // Second tick: still Interested because B has nothing back.
        let _ = tick(&mut w, t(2), &mut rng());
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).stage, RomanceStage::Interested);

        // Now B reciprocates enough to be Interested.
        shift_pair(
            &mut w,
            NpcId(2),
            NpcId(1),
            EdgeDelta {
                attraction: 25,
                ..EdgeDelta::default()
            },
        );
        let _ = tick(&mut w, t(3), &mut rng());
        // After B turns Interested, A's next tick should promote to Courting.
        let _ = tick(&mut w, t(4), &mut rng());
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).stage, RomanceStage::Courting);
    }

    #[test]
    fn courting_advances_to_committed_after_sustained_window() {
        let mut w = world_with_pair();
        shift_pair(
            &mut w,
            NpcId(1),
            NpcId(2),
            EdgeDelta {
                attraction: 50,
                affection: 50,
                ..EdgeDelta::default()
            },
        );
        shift_pair(
            &mut w,
            NpcId(2),
            NpcId(1),
            EdgeDelta {
                attraction: 50,
                affection: 50,
                ..EdgeDelta::default()
            },
        );
        // Two ticks at day 1 to advance through Interested → Courting on both sides.
        let _ = tick(&mut w, t(1), &mut rng());
        let _ = tick(&mut w, t(1), &mut rng());
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).stage, RomanceStage::Courting);
        // A long while later — should be Committed.
        let _ = tick(&mut w, t(80), &mut rng());
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).stage, RomanceStage::Committed);
    }

    #[test]
    fn any_stage_collapses_to_estranged_on_grievance_spike() {
        let mut w = world_with_pair();
        shift_pair(
            &mut w,
            NpcId(1),
            NpcId(2),
            EdgeDelta {
                attraction: 50,
                affection: 50,
                ..EdgeDelta::default()
            },
        );
        let _ = tick(&mut w, t(1), &mut rng());
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).stage, RomanceStage::Interested);

        shift_pair(
            &mut w,
            NpcId(1),
            NpcId(2),
            EdgeDelta {
                grievance: 70,
                ..EdgeDelta::default()
            },
        );
        let _ = tick(&mut w, t(2), &mut rng());
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).stage, RomanceStage::Estranged);
    }
}
