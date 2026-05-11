use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::engine::{Cause, EdgeDelta, Event, Fact, Time};
use crate::systems::{KnowledgeSource, apply_event};
use crate::world::{FactId, LocationId, NpcId, World};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Incident {
    pub kind: IncidentKind,
    pub when: Time,
    pub location: LocationId,
    /// Perpetrator(s) first, victim(s) after.
    pub principals: Vec<NpcId>,
    pub witnesses: Vec<NpcId>,
    /// Physical/social residue facts.
    pub evidence: Vec<FactId>,
    /// What an arriving outsider could plausibly know.
    pub public_facts: Vec<FactId>,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IncidentKind {
    Death {
        victim: NpcId,
        perpetrator: NpcId,
        method: String,
    },
    Disappearance {
        who: NpcId,
        reason: DisappearanceReason,
    },
    Scandal {
        participants: Vec<NpcId>,
        taboo: String,
    },
}

impl Incident {
    /// True if `target` is the simulation's recorded culprit/principal for
    /// this incident kind. Used to grade a player accusation.
    pub fn is_correct_accusation(&self, target: NpcId) -> bool {
        match &self.kind {
            IncidentKind::Death { perpetrator, .. } => target == *perpetrator,
            IncidentKind::Disappearance { who, .. } => target == *who,
            IncidentKind::Scandal { participants, .. } => participants.contains(&target),
        }
    }

    /// The simulation's "answer" — used in the epilogue narration when the
    /// player accuses no one or names the wrong person. For a scandal where
    /// multiple participants exist, returns the first.
    pub fn primary_culprit(&self) -> NpcId {
        match &self.kind {
            IncidentKind::Death { perpetrator, .. } => *perpetrator,
            IncidentKind::Disappearance { who, .. } => *who,
            IncidentKind::Scandal { participants, .. } => {
                participants.first().copied().unwrap_or(NpcId(0))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisappearanceReason {
    Fled,
    Taken,
    Hiding,
    Accident,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RipeMoment {
    pub kind: IncidentKind,
    pub score: f32,
    pub when: Time,
    pub location: LocationId,
}

/// How a playthrough ended. Decides the framing of the epilogue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaseOutcome {
    /// Player named the correct culprit.
    Solved { target: NpcId },
    /// Player named someone who is not the simulation's recorded culprit.
    MisAccused { target: NpcId, actual: NpcId },
    /// Deadline arrived without an accusation.
    Unsolved,
    /// Player walked away from the village.
    LeftTown,
    /// Player character died.
    Died,
}

impl CaseOutcome {
    pub fn short_label(&self) -> &'static str {
        match self {
            CaseOutcome::Solved { .. } => "solved",
            CaseOutcome::MisAccused { .. } => "wrongly accused",
            CaseOutcome::Unsolved => "deadline passed, unsolved",
            CaseOutcome::LeftTown => "player left town",
            CaseOutcome::Died => "player died",
        }
    }
}

const DEATH_GRIEVANCE_THRESHOLD: i32 = 70;
const DISAPPEARANCE_SAFETY_THRESHOLD: f32 = 0.85;
const DISAPPEARANCE_GRIEVANCE_AGAINST_THRESHOLD: i32 = 60;

/// Scan the post-forward-sim world for the highest-scoring ripe condition.
/// Order of preference (highest first): Death > Scandal > Disappearance.
pub fn detect_ripe(world: &World) -> Option<RipeMoment> {
    let now = Time {
        day: world.day,
        minute: 720,
    };

    // ---- Death pressure ----
    // High-grievance edge → perpetrator (subject) toward victim (object).
    let mut best_death: Option<((NpcId, NpcId), i32)> = None;
    for ((subj, obj), edge) in world.relationships_iter() {
        if edge.grievance < DEATH_GRIEVANCE_THRESHOLD {
            continue;
        }
        let Some(s) = world.npc(*subj) else { continue };
        let Some(o) = world.npc(*obj) else { continue };
        if s.dead || o.dead {
            continue;
        }
        let score = edge.grievance;
        if best_death.map(|(_, sc)| score > sc).unwrap_or(true) {
            best_death = Some(((*subj, *obj), score));
        }
    }
    if let Some(((perpetrator, victim), grievance)) = best_death {
        let location = world
            .npc(victim)
            .and_then(|n| n.location)
            .unwrap_or(LocationId(0));
        return Some(RipeMoment {
            kind: IncidentKind::Death {
                victim,
                perpetrator,
                method: pick_method(grievance),
            },
            score: grievance as f32,
            when: now,
            location,
        });
    }

    // ---- Scandal pressure ----
    // An affair-kind fact known to multiple NPCs, with ≥ 1 indirect witness.
    let mut best_scandal: Option<(FactId, Vec<NpcId>, f32)> = None;
    for (fact_id, fact) in &world.facts {
        if fact.kind != "affair" && fact.kind != "scandal" {
            continue;
        }
        let mut indirect_witnesses: Vec<NpcId> = Vec::new();
        let mut direct_witnesses: Vec<NpcId> = Vec::new();
        for npc in world.npcs.values() {
            if let Some(edge) = npc.knowledge.get(fact_id) {
                match edge.source {
                    KnowledgeSource::Witnessed => direct_witnesses.push(npc.id),
                    KnowledgeSource::ToldBy { .. } | KnowledgeSource::Inferred => {
                        indirect_witnesses.push(npc.id)
                    }
                }
            }
        }
        if direct_witnesses.len() < 3 {
            continue; // need ≥ 1 outsider beyond the two participants
        }
        let participants: Vec<NpcId> = direct_witnesses
            .iter()
            .take(2)
            .copied()
            .collect();
        let score = (direct_witnesses.len() + indirect_witnesses.len()) as f32 * 5.0;
        if best_scandal.as_ref().map(|(_, _, sc)| score > *sc).unwrap_or(true) {
            best_scandal = Some((*fact_id, participants, score));
        }
    }
    if let Some((_fact, participants, score)) = best_scandal {
        let location = world
            .npc(*participants.first().unwrap_or(&NpcId(0)))
            .and_then(|n| n.location)
            .unwrap_or(LocationId(0));
        return Some(RipeMoment {
            kind: IncidentKind::Scandal {
                participants,
                taboo: "adultery".into(),
            },
            score,
            when: now,
            location,
        });
    }

    // ---- Disappearance pressure ----
    let mut best_disappearance: Option<(NpcId, f32, DisappearanceReason)> = None;
    for npc in world.npcs.values() {
        if npc.dead {
            continue;
        }
        let safety = npc.needs.safety;
        let mut grievance_against: i32 = 0;
        for ((_subj, obj), edge) in world.relationships_iter() {
            if *obj == npc.id {
                grievance_against = grievance_against.max(edge.grievance);
            }
        }
        let mut score: f32 = 0.0;
        let mut reason = DisappearanceReason::Fled;
        if safety >= DISAPPEARANCE_SAFETY_THRESHOLD {
            score = score.max(safety * 100.0);
            reason = DisappearanceReason::Fled;
        }
        if grievance_against >= DISAPPEARANCE_GRIEVANCE_AGAINST_THRESHOLD {
            let s = grievance_against as f32;
            if s > score {
                score = s;
                reason = DisappearanceReason::Hiding;
            }
        }
        if score <= 0.0 {
            continue;
        }
        if best_disappearance
            .as_ref()
            .map(|(_, sc, _)| score > *sc)
            .unwrap_or(true)
        {
            best_disappearance = Some((npc.id, score, reason));
        }
    }
    if let Some((who, score, reason)) = best_disappearance {
        let location = world
            .npc(who)
            .and_then(|n| n.location)
            .unwrap_or(LocationId(0));
        return Some(RipeMoment {
            kind: IncidentKind::Disappearance { who, reason },
            score,
            when: now,
            location,
        });
    }

    None
}

fn pick_method(grievance: i32) -> String {
    if grievance >= 90 {
        "struck down in anger".into()
    } else if grievance >= 80 {
        "poisoned".into()
    } else {
        "found dead under suspicious circumstances".into()
    }
}

/// Materialize a `RipeMoment` into a full `Incident`, emitting all consequent
/// events (Witnessed, Told, Died, Spoken, Moved) through the dispatcher.
pub fn resolve(
    world: &mut World,
    ripe: RipeMoment,
    rng: &mut ChaCha8Rng,
) -> (Incident, Vec<Event>) {
    let mut events: Vec<Event> = Vec::new();
    let now = ripe.when;
    let location = ripe.location;

    match &ripe.kind {
        IncidentKind::Death {
            victim,
            perpetrator,
            method,
        } => {
            let victim_name = world
                .npc(*victim)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| "unknown".into());
            let perp_name = world
                .npc(*perpetrator)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| "unknown".into());
            let here_name = world
                .location(location)
                .map(|l| l.name.clone())
                .unwrap_or_else(|| "unknown".into());

            // Public fact: the death is known by everyone.
            let public_fact = world.intern_fact(Fact {
                kind: "death".into(),
                summary: format!("{} was found dead at the {}", victim_name, here_name),
            });
            // Hidden fact: the perpetrator's identity (only the perpetrator
            // and any direct witness knows).
            let hidden_fact = world.intern_fact(Fact {
                kind: "murder".into(),
                summary: format!("{} killed {} ({})", perp_name, victim_name, method),
            });
            // Evidence: a residue fact at the scene.
            let evidence_fact = world.intern_fact(Fact {
                kind: "evidence".into(),
                summary: format!("a token of {}'s left at the {}", perp_name, here_name),
            });

            // Perpetrator witnesses the murder fact.
            let ev = Event::Witnessed {
                observer: *perpetrator,
                fact: hidden_fact,
                when: now,
            };
            apply_event(world, &ev);
            events.push(ev);

            // Co-located NPCs (other than the principals) also witness the death.
            let here_npcs: Vec<NpcId> = world
                .npcs
                .values()
                .filter(|n| {
                    !n.dead
                        && n.location == Some(location)
                        && n.id != *victim
                        && n.id != *perpetrator
                })
                .map(|n| n.id)
                .collect();
            // Witnesses: the perpetrator + any co-located at the time.
            let mut witnesses: Vec<NpcId> = Vec::new();
            witnesses.push(*perpetrator);
            for w in &here_npcs {
                let ev = Event::Witnessed {
                    observer: *w,
                    fact: public_fact,
                    when: now,
                };
                apply_event(world, &ev);
                events.push(ev);
                witnesses.push(*w);
            }
            // Victim dies.
            let ev = Event::Died {
                who: *victim,
                cause: method.clone(),
                when: now,
            };
            apply_event(world, &ev);
            events.push(ev);

            let summary = format!(
                "{} was found dead at the {}; foul play suspected ({})",
                victim_name, here_name, method
            );
            let incident = Incident {
                kind: ripe.kind.clone(),
                when: now,
                location,
                principals: vec![*perpetrator, *victim],
                witnesses,
                evidence: vec![evidence_fact],
                public_facts: vec![public_fact],
                summary,
            };
            (incident, events)
        }

        IncidentKind::Disappearance { who, reason } => {
            let who_name = world
                .npc(*who)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| "unknown".into());
            let public_fact = world.intern_fact(Fact {
                kind: "disappearance".into(),
                summary: format!("{} has not been seen for days", who_name),
            });
            // Move the missing NPC to the "elsewhere" sentinel.
            let ev = Event::Moved {
                who: *who,
                to: LocationId(0),
                when: now,
            };
            apply_event(world, &ev);
            events.push(ev);

            // Any NPC currently co-located with `who` *was* the last to see them.
            let last_seen_by: Vec<NpcId> = world
                .npcs
                .values()
                .filter(|n| !n.dead && n.location == Some(location) && n.id != *who)
                .map(|n| n.id)
                .collect();
            // Random "elder" witness retains the strongest memory.
            let mut last_seen_by = last_seen_by;
            last_seen_by.shuffle(rng);
            for w in last_seen_by.iter().take(3) {
                let ev = Event::Witnessed {
                    observer: *w,
                    fact: public_fact,
                    when: now,
                };
                apply_event(world, &ev);
                events.push(ev);
            }

            let summary = match reason {
                DisappearanceReason::Fled => {
                    format!("{} appears to have fled the village", who_name)
                }
                DisappearanceReason::Taken => format!("{} was taken; rumors abound", who_name),
                DisappearanceReason::Hiding => {
                    format!("{} has gone to ground after recent trouble", who_name)
                }
                DisappearanceReason::Accident => {
                    format!("{} is missing; accident feared", who_name)
                }
            };
            let incident = Incident {
                kind: ripe.kind.clone(),
                when: now,
                location,
                principals: vec![*who],
                witnesses: last_seen_by,
                evidence: Vec::new(),
                public_facts: vec![public_fact],
                summary,
            };
            (incident, events)
        }

        IncidentKind::Scandal {
            participants,
            taboo,
        } => {
            let names: Vec<String> = participants
                .iter()
                .map(|id| {
                    world
                        .npc(*id)
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| "unknown".into())
                })
                .collect();
            let public_fact = world.intern_fact(Fact {
                kind: "scandal".into(),
                summary: format!("{} are suspected of {}", names.join(" and "), taboo),
            });
            // Speech events: a few NPCs in the village spread the word.
            let speakers: Vec<NpcId> = world
                .npcs
                .values()
                .filter(|n| !n.dead && n.id != participants[0])
                .take(3)
                .map(|n| n.id)
                .collect();
            let mut witnesses: Vec<NpcId> = Vec::new();
            for s in speakers {
                let listeners: Vec<NpcId> = world
                    .npcs
                    .values()
                    .filter(|n| !n.dead && n.id != s)
                    .take(2)
                    .map(|n| n.id)
                    .collect();
                let line = format!("Have you heard about {}?", names.join(" and "));
                let ev = Event::Spoken {
                    speaker: s,
                    listeners: listeners.clone(),
                    line,
                    when: now,
                };
                apply_event(world, &ev);
                events.push(ev);
                witnesses.extend(listeners);
            }
            let summary = format!("rumors of {} between {}", taboo, names.join(" and "));
            let incident = Incident {
                kind: ripe.kind.clone(),
                when: now,
                location,
                principals: participants.clone(),
                witnesses,
                evidence: Vec::new(),
                public_facts: vec![public_fact],
                summary,
            };
            (incident, events)
        }
    }
}

/// Used when no organic ripe-moment fires: nudges a high-grievance edge to
/// the threshold so detection succeeds on the next call. Returns true if it
/// found anything to nudge.
pub(crate) fn nudge_pressure(world: &mut World, rng: &mut ChaCha8Rng) -> bool {
    // Find the highest-grievance edge and bump it just over the death threshold.
    let mut top: Option<((NpcId, NpcId), i32)> = None;
    for ((s, o), edge) in world.relationships_iter() {
        if top.map(|(_, g)| edge.grievance > g).unwrap_or(true) {
            top = Some(((*s, *o), edge.grievance));
        }
    }
    let Some(((subj, obj), cur)) = top else {
        return false;
    };
    let bump = (DEATH_GRIEVANCE_THRESHOLD + 5 - cur).max(0);
    if bump == 0 {
        return true;
    }
    let _ = rng; // reserved for future randomization
    let ev = Event::RelationshipShift {
        subject: subj,
        object: obj,
        delta: EdgeDelta {
            affection: 0,
            trust: 0,
            debt: 0,
            grievance: bump,
            attraction: 0,
        },
        cause: Cause {
            summary: "an old wound finally festered".into(),
        },
        when: Time {
            day: world.day,
            minute: 0,
        },
    };
    apply_event(world, &ev);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incident_round_trips_through_toml() {
        let i = Incident {
            kind: IncidentKind::Death {
                victim: NpcId(3),
                perpetrator: NpcId(7),
                method: "poison".into(),
            },
            when: Time { day: 1825, minute: 720 },
            location: LocationId(1),
            principals: vec![NpcId(7), NpcId(3)],
            witnesses: vec![NpcId(4), NpcId(5)],
            evidence: vec![FactId(1), FactId(2)],
            public_facts: vec![FactId(1)],
            summary: "a death at the mill".into(),
        };
        let body = toml::to_string_pretty(&i).unwrap();
        let back: Incident = toml::from_str(&body).unwrap();
        assert_eq!(i, back);
    }

    #[test]
    fn scandal_kind_round_trips() {
        let i = Incident {
            kind: IncidentKind::Scandal {
                participants: vec![NpcId(1), NpcId(2)],
                taboo: "adultery".into(),
            },
            when: Time { day: 100, minute: 0 },
            location: LocationId(2),
            principals: vec![NpcId(1), NpcId(2)],
            witnesses: vec![NpcId(3)],
            evidence: vec![],
            public_facts: vec![FactId(5)],
            summary: "a rumored affair".into(),
        };
        let body = toml::to_string_pretty(&i).unwrap();
        let back: Incident = toml::from_str(&body).unwrap();
        assert_eq!(i, back);
    }

    use crate::engine::EdgeDelta;
    use crate::systems::apply_event;
    use crate::world::{Location, Npc};
    use rand::SeedableRng;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0)
    }

    fn seeded_world() -> World {
        let mut w = World::new(0);
        w.insert_location(Location::new(LocationId(0), "elsewhere"));
        let mut tavern = Location::new(LocationId(1), "tavern");
        tavern.adjacent = vec![];
        w.insert_location(tavern);
        let mut a = Npc::new(NpcId(1), "Aldwin");
        a.location = Some(LocationId(1));
        a.age = 35;
        w.insert_npc(a);
        let mut b = Npc::new(NpcId(2), "Bryn");
        b.location = Some(LocationId(1));
        b.age = 32;
        w.insert_npc(b);
        let mut c = Npc::new(NpcId(3), "Cera");
        c.location = Some(LocationId(1));
        c.age = 28;
        w.insert_npc(c);
        w
    }

    #[test]
    fn detect_ripe_returns_none_on_quiet_world() {
        let w = seeded_world();
        assert!(detect_ripe(&w).is_none());
    }

    #[test]
    fn detect_ripe_returns_death_on_high_grievance() {
        let mut w = seeded_world();
        let ev = Event::RelationshipShift {
            subject: NpcId(1),
            object: NpcId(2),
            delta: EdgeDelta {
                affection: 0,
                trust: 0,
                debt: 0,
                grievance: 90,
                attraction: 0,
            },
            cause: Cause {
                summary: "old wound".into(),
            },
            when: Time { day: 0, minute: 0 },
        };
        apply_event(&mut w, &ev);
        let ripe = detect_ripe(&w).expect("ripe");
        match ripe.kind {
            IncidentKind::Death {
                victim,
                perpetrator,
                ..
            } => {
                assert_eq!(perpetrator, NpcId(1));
                assert_eq!(victim, NpcId(2));
            }
            other => panic!("expected Death, got {:?}", other),
        }
    }

    #[test]
    fn resolve_death_marks_victim_dead_and_records_witnesses() {
        let mut w = seeded_world();
        let ev = Event::RelationshipShift {
            subject: NpcId(1),
            object: NpcId(2),
            delta: EdgeDelta {
                affection: 0,
                trust: 0,
                debt: 0,
                grievance: 95,
                attraction: 0,
            },
            cause: Cause {
                summary: "old wound".into(),
            },
            when: Time { day: 0, minute: 0 },
        };
        apply_event(&mut w, &ev);
        let ripe = detect_ripe(&w).unwrap();
        let (incident, events) = resolve(&mut w, ripe, &mut rng());
        assert!(events.iter().any(|e| matches!(e, Event::Died { who, .. } if *who == NpcId(2))));
        assert!(w.npc(NpcId(2)).unwrap().dead);
        // Public facts: just the death, not the murder.
        assert_eq!(incident.public_facts.len(), 1);
        let pf = w.fact(incident.public_facts[0]).unwrap();
        assert_eq!(pf.kind, "death");
        // Witnesses include the perpetrator at least.
        assert!(incident.witnesses.contains(&NpcId(1)));
    }

    #[test]
    fn resolve_disappearance_moves_to_elsewhere() {
        let mut w = seeded_world();
        // Push safety high for NPC 3.
        w.npcs.get_mut(&NpcId(3)).unwrap().needs.safety = 0.95;
        let ripe = detect_ripe(&w).expect("disappearance");
        let (_incident, events) = resolve(&mut w, ripe, &mut rng());
        let moved = events.iter().find(|e| {
            matches!(e,
                Event::Moved { who, to, .. } if *who == NpcId(3) && *to == LocationId(0))
        });
        assert!(moved.is_some(), "expected a Moved to elsewhere");
        assert_eq!(w.npc(NpcId(3)).unwrap().location, Some(LocationId(0)));
    }

    #[test]
    fn is_correct_accusation_matches_perpetrator_for_death() {
        let i = Incident {
            kind: IncidentKind::Death {
                victim: NpcId(3),
                perpetrator: NpcId(7),
                method: "poison".into(),
            },
            when: Time { day: 1, minute: 0 },
            location: LocationId(1),
            principals: vec![NpcId(7), NpcId(3)],
            witnesses: vec![],
            evidence: vec![],
            public_facts: vec![],
            summary: "".into(),
        };
        assert!(i.is_correct_accusation(NpcId(7)));
        assert!(!i.is_correct_accusation(NpcId(3)));
        assert!(!i.is_correct_accusation(NpcId(99)));
        assert_eq!(i.primary_culprit(), NpcId(7));
    }

    #[test]
    fn is_correct_accusation_matches_any_scandal_participant() {
        let i = Incident {
            kind: IncidentKind::Scandal {
                participants: vec![NpcId(2), NpcId(5)],
                taboo: "adultery".into(),
            },
            when: Time { day: 1, minute: 0 },
            location: LocationId(1),
            principals: vec![NpcId(2), NpcId(5)],
            witnesses: vec![],
            evidence: vec![],
            public_facts: vec![],
            summary: "".into(),
        };
        assert!(i.is_correct_accusation(NpcId(2)));
        assert!(i.is_correct_accusation(NpcId(5)));
        assert!(!i.is_correct_accusation(NpcId(7)));
        assert_eq!(i.primary_culprit(), NpcId(2));
    }

    #[test]
    fn case_outcome_round_trips_through_toml() {
        for outcome in [
            CaseOutcome::Solved { target: NpcId(3) },
            CaseOutcome::MisAccused {
                target: NpcId(4),
                actual: NpcId(7),
            },
            CaseOutcome::Unsolved,
            CaseOutcome::LeftTown,
            CaseOutcome::Died,
        ] {
            let body = toml::to_string_pretty(&outcome).unwrap();
            let back: CaseOutcome = toml::from_str(&body).unwrap();
            assert_eq!(outcome, back);
        }
    }

    #[test]
    fn nudge_pressure_promotes_lower_grievance_above_threshold() {
        let mut w = seeded_world();
        let ev = Event::RelationshipShift {
            subject: NpcId(1),
            object: NpcId(2),
            delta: EdgeDelta {
                affection: 0,
                trust: 0,
                debt: 0,
                grievance: 30,
                attraction: 0,
            },
            cause: Cause {
                summary: "petty".into(),
            },
            when: Time { day: 0, minute: 0 },
        };
        apply_event(&mut w, &ev);
        assert!(detect_ripe(&w).is_none());
        assert!(nudge_pressure(&mut w, &mut rng()));
        let ripe = detect_ripe(&w).expect("ripe after nudge");
        assert!(matches!(ripe.kind, IncidentKind::Death { .. }));
    }
}
