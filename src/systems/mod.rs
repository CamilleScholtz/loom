// Phase 4 ships the simulation systems as a library; Phase 5 will wire them
// into the orchestrator. Until then, the public API surface here is exercised
// only by tests, so we silence dead-code lints at the module boundary.
#![allow(dead_code)]

pub mod goals;
pub mod knowledge;
pub mod mood;
pub mod needs;
pub mod relationships;
pub mod reputation;
pub mod romance;
pub mod traits;

#[allow(unused_imports)]
pub use goals::ActiveGoal;
#[allow(unused_imports)]
pub use knowledge::{KnowledgeEdge, KnowledgeSource};
#[allow(unused_imports)]
pub use mood::{Mood, MoodImprint};
#[allow(unused_imports)]
pub use needs::{NeedKind, Needs};
#[allow(unused_imports)]
pub use relationships::{Edge, RomanceStage};
#[allow(unused_imports)]
pub use reputation::Reputation;
#[allow(unused_imports)]
pub use traits::{TraitSet, Value, ValueWeights};

use crate::engine::Event;
use crate::world::World;

/// Apply an event to the world. This is the **only** public mutation funnel
/// for `Event` values — every system's `apply_*` is `pub(crate)` and called
/// only from here. Routine ticks (decay, propagation) are separate and not
/// driven through this function.
pub fn apply_event(world: &mut World, ev: &Event) {
    match ev {
        Event::Witnessed { observer, fact, when } => {
            knowledge::apply_witnessed(world, *observer, *fact, *when);
            let kind = world.fact(*fact).map(|f| f.kind.clone());
            if let (Some(kind), Some(npc)) = (kind, world.npcs.get_mut(observer)) {
                mood::imprint_witnessed(npc, &kind, *when);
            }
        }
        Event::Told {
            teller,
            listener,
            fact,
            distortion,
            when,
        } => {
            knowledge::apply_told(world, *teller, *listener, *fact, *distortion, *when);
            let kind = world.fact(*fact).map(|f| f.kind.clone());
            if let (Some(kind), Some(npc)) = (kind, world.npcs.get_mut(listener)) {
                mood::imprint_told(npc, &kind, *distortion, *when);
            }
        }
        Event::RelationshipShift {
            subject,
            object,
            delta,
            cause: _,
            when,
        } => {
            relationships::apply_relationship_shift(world, *subject, *object, delta);
            if let Some(npc) = world.npcs.get_mut(subject) {
                mood::imprint_relationship_shift(npc, delta, *when);
            }
            let _ = object; // object is recorded in the edge; no NPC state change on the object side.
        }
        Event::GoalFormed { holder, goal, when } => {
            goals::apply_goal_formed(world, *holder, goal, *when);
        }
        Event::GoalResolved {
            holder,
            goal,
            outcome,
            when,
        } => {
            goals::apply_goal_resolved(world, *holder, goal, outcome);
            if let Some(npc) = world.npcs.get_mut(holder) {
                mood::imprint_goal_resolved(npc, outcome, *when);
            }
        }
        Event::Spoken { .. } => {
            // Speech is rendered; no engine state mutation. Future: track
            // "heard" knowledge if the line carries a fact reference.
        }
        Event::Moved { who, to, when: _ } => {
            if let Some(npc) = world.npcs.get_mut(who) {
                npc.location = Some(*to);
            }
        }
        Event::Born { who: _, when: _ } => {
            // Worldgen uses `Born` as a record-keeping event; the NPC is
            // inserted into `world.npcs` by the worldgen pipeline, not by the
            // dispatcher. No state change here.
        }
        Event::Died { who, cause: _, when: _ } => {
            if let Some(npc) = world.npcs.get_mut(who) {
                npc.dead = true;
                npc.location = None;
            }
        }
        Event::Remembered { holder, event, when: _ } => {
            if let Some(npc) = world.npcs.get_mut(holder) {
                npc.memorable_events.push(event.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{
        Cause, Distortion, EdgeDelta, Fact, Goal as EvGoal, Outcome, Time,
    };
    use crate::world::{LocationId, Npc, NpcId};

    fn t() -> Time {
        Time { day: 0, minute: 0 }
    }

    fn fresh_world() -> World {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        w.insert_npc(Npc::new(NpcId(2), "b"));
        w
    }

    #[test]
    fn witnessed_writes_knowledge_and_imprint() {
        let mut w = fresh_world();
        let f = w.intern_fact(Fact {
            kind: "theft".into(),
            summary: "x".into(),
        });
        apply_event(
            &mut w,
            &Event::Witnessed {
                observer: NpcId(1),
                fact: f,
                when: t(),
            },
        );
        assert!(w.npc(NpcId(1)).unwrap().knowledge.contains_key(&f));
        assert!(w.npc(NpcId(1)).unwrap().mood.valence < 0.0);
    }

    #[test]
    fn told_writes_listener_knowledge() {
        let mut w = fresh_world();
        let f = w.intern_fact(Fact {
            kind: "rumor".into(),
            summary: "x".into(),
        });
        apply_event(
            &mut w,
            &Event::Told {
                teller: NpcId(1),
                listener: NpcId(2),
                fact: f,
                distortion: Distortion { level: 1 },
                when: t(),
            },
        );
        assert!(w.npc(NpcId(2)).unwrap().knowledge.contains_key(&f));
        // Teller's knowledge unaffected (engine does not infer prerequisites).
        assert!(!w.npc(NpcId(1)).unwrap().knowledge.contains_key(&f));
    }

    #[test]
    fn relationship_shift_routes_through_dispatcher() {
        let mut w = fresh_world();
        apply_event(
            &mut w,
            &Event::RelationshipShift {
                subject: NpcId(1),
                object: NpcId(2),
                delta: EdgeDelta {
                    affection: 30,
                    trust: 10,
                    debt: 0,
                    grievance: 0,
                    attraction: 0,
                },
                cause: Cause {
                    summary: "a kindness".into(),
                },
                when: t(),
            },
        );
        assert_eq!(w.relationship(NpcId(1), NpcId(2)).affection, 30);
        assert!(w.npc(NpcId(1)).unwrap().mood.valence > 0.0);
    }

    #[test]
    fn goal_formed_records_active_goal() {
        let mut w = fresh_world();
        apply_event(
            &mut w,
            &Event::GoalFormed {
                holder: NpcId(1),
                goal: EvGoal {
                    kind: "eat".into(),
                    summary: "eat".into(),
                },
                when: t(),
            },
        );
        assert_eq!(w.npc(NpcId(1)).unwrap().goals[0].kind, "eat");
    }

    #[test]
    fn goal_resolved_clears_active_and_imprints_mood() {
        let mut w = fresh_world();
        // Prime: form a goal first.
        apply_event(
            &mut w,
            &Event::GoalFormed {
                holder: NpcId(1),
                goal: EvGoal {
                    kind: "eat".into(),
                    summary: "eat".into(),
                },
                when: t(),
            },
        );
        apply_event(
            &mut w,
            &Event::GoalResolved {
                holder: NpcId(1),
                goal: EvGoal {
                    kind: "eat".into(),
                    summary: "eat".into(),
                },
                outcome: Outcome::Achieved,
                when: t(),
            },
        );
        assert!(w.npc(NpcId(1)).unwrap().goals.is_empty());
        assert!(w.npc(NpcId(1)).unwrap().mood.valence > 0.0);
    }

    #[test]
    fn moved_updates_location() {
        let mut w = fresh_world();
        apply_event(
            &mut w,
            &Event::Moved {
                who: NpcId(1),
                to: LocationId(7),
                when: t(),
            },
        );
        assert_eq!(w.npc(NpcId(1)).unwrap().location, Some(LocationId(7)));
    }

    #[test]
    fn spoken_is_a_noop_on_state() {
        let mut w = fresh_world();
        let before = w.npc(NpcId(1)).unwrap().clone();
        apply_event(
            &mut w,
            &Event::Spoken {
                speaker: NpcId(1),
                listeners: vec![NpcId(2)],
                line: "hi".into(),
                when: t(),
            },
        );
        let after = w.npc(NpcId(1)).unwrap();
        assert_eq!(before.location, after.location);
        assert_eq!(before.knowledge, after.knowledge);
        assert_eq!(before.goals, after.goals);
    }

    #[test]
    fn relationships_field_is_pub_crate_compile_check() {
        // This test compiles only because the test module is inside the crate.
        // External code would have no access to `world.relationships`. If this
        // file ever moves to an integration test, the line below will refuse
        // to compile, alerting us to a visibility regression.
        let w = fresh_world();
        let _ = &w.relationships;
    }

    #[test]
    fn replaying_sample_events_produces_expected_state() {
        let mut w = fresh_world();
        let f1 = w.intern_fact(Fact {
            kind: "theft".into(),
            summary: "x".into(),
        });
        let f2 = w.intern_fact(Fact {
            kind: "rumor".into(),
            summary: "y".into(),
        });
        let events = vec![
            Event::Witnessed {
                observer: NpcId(1),
                fact: f1,
                when: t(),
            },
            Event::Told {
                teller: NpcId(1),
                listener: NpcId(2),
                fact: f2,
                distortion: Distortion { level: 1 },
                when: t(),
            },
            Event::RelationshipShift {
                subject: NpcId(1),
                object: NpcId(2),
                delta: EdgeDelta {
                    affection: -10,
                    trust: -5,
                    debt: 0,
                    grievance: 5,
                    attraction: 0,
                },
                cause: Cause {
                    summary: "a quarrel".into(),
                },
                when: t(),
            },
        ];
        for ev in &events {
            apply_event(&mut w, ev);
        }
        assert!(w.npc(NpcId(1)).unwrap().knowledge.contains_key(&f1));
        assert!(w.npc(NpcId(2)).unwrap().knowledge.contains_key(&f2));
        let edge = w.relationship(NpcId(1), NpcId(2));
        assert_eq!(edge.affection, -10);
        assert_eq!(edge.grievance, 5);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::engine::{Cause, Distortion, EdgeDelta, Fact, Time};
    use crate::storage::SaveDir;
    use crate::world::{FactId, Npc, NpcId};
    use proptest::collection::vec as pvec;
    use proptest::prelude::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    const N_NPCS: u32 = 3;
    const N_FACTS: u32 = 4;

    fn npc_id() -> impl Strategy<Value = NpcId> {
        (1u32..=N_NPCS).prop_map(NpcId)
    }

    fn fact_id() -> impl Strategy<Value = FactId> {
        (1u32..=N_FACTS).prop_map(FactId)
    }

    fn time() -> impl Strategy<Value = Time> {
        (0u32..3, 0u32..1440).prop_map(|(day, minute)| Time { day, minute })
    }

    fn distortion() -> impl Strategy<Value = Distortion> {
        (0u8..=4).prop_map(|level| Distortion { level })
    }

    fn edge_delta() -> impl Strategy<Value = EdgeDelta> {
        (
            -100i32..=100,
            -100i32..=100,
            -50i32..=50,
            -50i32..=50,
        )
            .prop_map(|(a, t, d, g)| EdgeDelta {
                affection: a,
                trust: t,
                debt: d,
                grievance: g,
                attraction: 0,
            })
    }

    fn knowledge_event() -> impl Strategy<Value = Event> {
        prop_oneof![
            (npc_id(), fact_id(), time()).prop_map(|(observer, fact, when)| Event::Witnessed {
                observer,
                fact,
                when,
            }),
            (npc_id(), npc_id(), fact_id(), distortion(), time()).prop_map(
                |(teller, listener, fact, distortion, when)| Event::Told {
                    teller,
                    listener,
                    fact,
                    distortion,
                    when,
                },
            ),
        ]
    }

    fn any_event_no_relationship_shift() -> impl Strategy<Value = Event> {
        prop_oneof![
            knowledge_event(),
            (npc_id(), time()).prop_map(|(holder, when)| Event::GoalFormed {
                holder,
                goal: crate::engine::Goal {
                    kind: "k".into(),
                    summary: "s".into(),
                },
                when,
            }),
        ]
    }

    fn any_event() -> impl Strategy<Value = Event> {
        prop_oneof![
            knowledge_event(),
            (npc_id(), npc_id(), edge_delta(), time()).prop_map(
                |(subject, object, delta, when)| Event::RelationshipShift {
                    subject,
                    object,
                    delta,
                    cause: Cause {
                        summary: "x".into(),
                    },
                    when,
                },
            ),
        ]
    }

    fn populated_world() -> World {
        let mut w = World::new(0);
        for id in 1..=N_NPCS {
            w.insert_npc(Npc::new(NpcId(id), format!("npc-{}", id)));
        }
        for id in 1..=N_FACTS {
            let _ = w.intern_fact(Fact {
                kind: "rumor".into(),
                summary: format!("f-{}", id),
            });
        }
        w
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// 1. confidence_invariant — every KnowledgeEdge.confidence ∈ [0, 1]
        ///    after any sequence of Witnessed/Told events.
        #[test]
        fn confidence_invariant(events in pvec(knowledge_event(), 0..40)) {
            let mut w = populated_world();
            for ev in &events {
                apply_event(&mut w, ev);
                for npc in w.npcs.values() {
                    for edge in npc.knowledge.values() {
                        prop_assert!(
                            (0.0..=1.0).contains(&edge.confidence),
                            "confidence out of range: {}",
                            edge.confidence
                        );
                    }
                }
            }
        }

        /// 5. fact_id_stable_under_intern — equal Facts intern to the same FactId.
        #[test]
        fn fact_id_stable_under_intern(
            seeds in pvec((any::<u8>(), any::<u8>()), 0..20),
        ) {
            let mut w = World::new(0);
            for (a, b) in &seeds {
                let f = Fact {
                    kind: format!("k{}", a),
                    summary: format!("s{}", b),
                };
                let id1 = w.intern_fact(f.clone());
                let id2 = w.intern_fact(f);
                prop_assert_eq!(id1, id2);
            }
        }

        /// 3. no_fact_without_acquisition — every (npc, fact) in npc.knowledge
        ///    has a prior Witnessed/Told event for that pair.
        #[test]
        fn no_fact_without_acquisition(events in pvec(knowledge_event(), 0..40)) {
            let mut w = populated_world();
            let mut acquired: HashSet<(NpcId, FactId)> = HashSet::new();
            for ev in &events {
                match ev {
                    Event::Witnessed { observer, fact, .. } => {
                        acquired.insert((*observer, *fact));
                    }
                    Event::Told { listener, fact, .. } => {
                        acquired.insert((*listener, *fact));
                    }
                    _ => {}
                }
                apply_event(&mut w, ev);
            }
            for (id, npc) in &w.npcs {
                for fact in npc.knowledge.keys() {
                    prop_assert!(
                        acquired.contains(&(*id, *fact)),
                        "npc {:?} holds fact {:?} without acquisition event",
                        id,
                        fact
                    );
                }
            }
        }

        /// 4. relationships_only_change_via_event — events that are not
        ///    RelationshipShift never mutate world.relationships.
        #[test]
        fn relationships_only_change_via_event(
            events in pvec(any_event_no_relationship_shift(), 0..40),
        ) {
            let mut w = populated_world();
            let before = w.relationships.clone();
            for ev in &events {
                apply_event(&mut w, ev);
            }
            prop_assert_eq!(before, w.relationships.clone());
        }

        /// 2. relationships_round_trip — arbitrary World saves, loads, and
        ///    re-saves byte-identically.
        #[test]
        fn relationships_round_trip(events in pvec(any_event(), 0..30)) {
            let mut w = populated_world();
            for ev in &events {
                apply_event(&mut w, ev);
            }
            let dir_a = TempDir::new().unwrap();
            let dir_b = TempDir::new().unwrap();
            let save_a = SaveDir::open(dir_a.path()).unwrap();
            save_a.save_world(&w).unwrap();
            let loaded = save_a.load_world().unwrap();
            prop_assert_eq!(&w, &loaded);
            let save_b = SaveDir::open(dir_b.path()).unwrap();
            save_b.save_world(&loaded).unwrap();
            // Byte-identical check on the relationships file (smallest scope
            // we need for this invariant).
            for id in [NpcId(1), NpcId(2), NpcId(3)] {
                let a = std::fs::read(
                    dir_a
                        .path()
                        .join(format!("characters/{}-npc-{}/relationships.toml", id.0, id.0)),
                )
                .ok();
                let b = std::fs::read(
                    dir_b
                        .path()
                        .join(format!("characters/{}-npc-{}/relationships.toml", id.0, id.0)),
                )
                .ok();
                prop_assert_eq!(a, b);
            }
        }
    }

}
