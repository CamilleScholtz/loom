//! Engine-side dispatcher for the bounded "tools" the LLM may call during NPC
//! dialogue (see `llm::interpret::Proposal`). The LLM proposes; the engine
//! validates against the current world state and translates each proposal into
//! a list of `Event`s, notebook bullets, and follow-up `Action`s the next scene
//! should surface to the player.
//!
//! Per the GAME.md / TECH.md architectural rule, this layer is the *only* path
//! by which a dialogue tool changes world state. Invalid ids are silently
//! dropped — the LLM cannot invent NPCs, locations, or arrival times that the
//! world doesn't already know about. What the LLM controls is surface text
//! (item names, promise summaries, reasons); everything structural is filtered
//! through here.

use crate::engine::action::Action;
use crate::engine::event::{Cause, EdgeDelta, Event, Time};
use crate::llm::interpret::{Proposal, ProposalScope};
use crate::world::{LocationId, MemorableEvent, NpcId, World};

/// Result of applying a single dialogue turn's proposals. `events` and
/// `notebook` are ready for the caller to write through `apply_event` and
/// `notebook_append`; `followups` and `dialogue_ended` shape the scene that
/// follows.
#[derive(Debug, Default)]
pub struct AppliedProposals {
    pub events: Vec<Event>,
    pub notebook: Vec<String>,
    /// Extra actions the next scene should offer the player (e.g.
    /// `FollowNpc`). Order is preserved.
    pub followups: Vec<Action>,
    /// True if any proposal asked the conversation to end. Caller is expected
    /// to drop the in-scene dialogue history with this NPC.
    pub dialogue_ended: bool,
}

/// Apply every `Proposal` voiced by `npc` toward the player on this turn.
/// Reads-only on `world` — emitted events are returned to the caller, who is
/// expected to feed them through `apply_event` and append them to the event
/// log in arrival order.
pub fn apply_proposals(
    world: &World,
    npc: NpcId,
    proposals: &[Proposal],
    when: Time,
) -> AppliedProposals {
    let mut out = AppliedProposals::default();
    let here = world.npc(npc).and_then(|n| n.location);
    let npc_name = world
        .npc(npc)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| "they".into());

    for prop in proposals {
        let events_before = out.events.len();
        let followups_before = out.followups.len();
        match prop {
            Proposal::Relocate { to, scope, why } => {
                apply_relocate(world, npc, *to, *scope, why.as_deref(), here, &npc_name, when, &mut out);
            }
            Proposal::Fetch { who, why } => {
                apply_fetch(world, npc, NpcId(*who), why.as_deref(), here, &npc_name, when, &mut out);
            }
            Proposal::HandOver { item, why } => {
                apply_hand_over(npc, item, why.as_deref(), &npc_name, when, &mut out);
            }
            Proposal::Promise { summary, by_day } => {
                apply_promise(npc, summary, *by_day, &npc_name, when, &mut out);
            }
            Proposal::BreakOff { reason } => {
                apply_break_off(npc, reason.as_deref(), &npc_name, when, &mut out);
            }
            Proposal::DiscoverLocation {
                name,
                location_kind,
                description,
                relocate,
                why,
            } => {
                apply_discover_location(
                    world,
                    npc,
                    name,
                    location_kind.as_deref(),
                    description.as_deref(),
                    *relocate,
                    why.as_deref(),
                    here,
                    &npc_name,
                    when,
                    &mut out,
                );
            }
        }
        // A proposal that produced zero events AND zero followups is one
        // the engine dropped. Surface it loudly at WARN so prompt-tuning has
        // visibility — the alternative is silent disagreement between what
        // the NPC said and what happened.
        let produced_events = out.events.len() > events_before;
        let produced_followup = out.followups.len() > followups_before;
        if !produced_events && !produced_followup {
            tracing::warn!(
                proposal = ?summarize_proposal(prop),
                npc = npc.0,
                npc_name = %npc_name,
                "proposal silently dropped — invalid id, unreachable destination, or empty payload"
            );
        }
    }
    out
}

/// Compact tracing-friendly label for a Proposal. Used when logging dropped
/// proposals so the warning line is greppable.
fn summarize_proposal(p: &Proposal) -> String {
    match p {
        Proposal::Relocate { to, scope, .. } => format!("relocate(to={}, scope={:?})", to, scope),
        Proposal::Fetch { who, .. } => format!("fetch(who={})", who),
        Proposal::HandOver { item, .. } => format!("hand_over({:?})", item),
        Proposal::Promise { summary, by_day } => {
            format!("promise({:?}, by_day={:?})", summary, by_day)
        }
        Proposal::BreakOff { reason } => format!("break_off({:?})", reason),
        Proposal::DiscoverLocation { name, relocate, .. } => {
            format!("discover_location({:?}, relocate={:?})", name, relocate)
        }
    }
}

fn apply_relocate(
    world: &World,
    npc: NpcId,
    to_raw: u32,
    scope: ProposalScope,
    why: Option<&str>,
    here: Option<LocationId>,
    npc_name: &str,
    when: Time,
    out: &mut AppliedProposals,
) {
    let to = LocationId(to_raw);
    // The destination must exist, must not be the off-map sentinel, and must
    // be reachable from where the NPC stands.
    if to.0 == 0 || world.location(to).is_none() {
        return;
    }
    let Some(origin) = here else { return };
    let reachable = world
        .location(origin)
        .map(|l| l.adjacent.contains(&to))
        .unwrap_or(false);
    if !reachable {
        return;
    }
    let dest_name = world
        .location(to)
        .map(|l| l.name.clone())
        .unwrap_or_else(|| "elsewhere".into());

    match scope {
        ProposalScope::Lead | ProposalScope::Depart => {
            // NPC moves now.
            out.events.push(Event::Moved {
                who: npc,
                to,
                when,
            });
            let why_clause = why.map(|w| format!(" — {}", w)).unwrap_or_default();
            let line = match scope {
                ProposalScope::Lead => format!(
                    "- {} heads for the {}, waving you on{}.",
                    npc_name, dest_name, why_clause
                ),
                ProposalScope::Depart => format!(
                    "- {} walks off toward the {}{}.",
                    npc_name, dest_name, why_clause
                ),
                ProposalScope::Joint => unreachable!(),
            };
            out.notebook.push(line);
            if matches!(scope, ProposalScope::Lead) {
                out.followups
                    .push(Action::FollowNpc { npc, to });
            }
        }
        ProposalScope::Joint => {
            // NPC stays put; player gets a "go together" follow-up.
            let why_clause = why.map(|w| format!(" — {}", w)).unwrap_or_default();
            out.notebook.push(format!(
                "- {} suggests going to the {} together{}.",
                npc_name, dest_name, why_clause
            ));
            out.followups
                .push(Action::FollowNpc { npc, to });
        }
    }
}

fn apply_fetch(
    world: &World,
    npc: NpcId,
    target: NpcId,
    why: Option<&str>,
    here: Option<LocationId>,
    npc_name: &str,
    when: Time,
    out: &mut AppliedProposals,
) {
    // Target must exist, be alive, and not be the player or the speaker.
    let Some(target_npc) = world.npc(target) else {
        return;
    };
    if target_npc.dead || target == npc || target.0 == 0 {
        return;
    }
    let Some(here) = here else { return };
    // No-op if the target is already in the room.
    if target_npc.location == Some(here) {
        return;
    }
    let target_name = target_npc.name.clone();
    // Materialize the arrival as a movement event. v1 is immediate; a
    // future iteration could schedule it a tick out.
    out.events.push(Event::Moved {
        who: target,
        to: here,
        when,
    });
    let why_clause = why.map(|w| format!(" — {}", w)).unwrap_or_default();
    out.notebook.push(format!(
        "- {} goes to fetch {}; {} arrives shortly{}.",
        npc_name, target_name, target_name, why_clause
    ));
}

fn apply_hand_over(
    npc: NpcId,
    item: &str,
    why: Option<&str>,
    npc_name: &str,
    when: Time,
    out: &mut AppliedProposals,
) {
    let item = item.trim();
    if item.is_empty() {
        return;
    }
    out.events.push(Event::HandedOver {
        giver: npc,
        receiver: NpcId(0),
        item: item.to_string(),
        when,
    });
    // A small affection/trust bump on the giver's side — handing something
    // over is a gesture of trust. Magnitudes match the `Comfort` baseline.
    out.events.push(Event::RelationshipShift {
        subject: npc,
        object: NpcId(0),
        delta: EdgeDelta {
            affection: 1,
            trust: 1,
            ..EdgeDelta::default()
        },
        cause: Cause {
            summary: format!("I gave the player {}", item),
        },
        when,
    });
    let why_clause = why.map(|w| format!(" — {}", w)).unwrap_or_default();
    out.notebook.push(format!(
        "- {} hands you {}{}.",
        npc_name, item, why_clause
    ));
}

fn apply_promise(
    npc: NpcId,
    summary: &str,
    by_day: Option<u32>,
    npc_name: &str,
    when: Time,
    out: &mut AppliedProposals,
) {
    let summary = summary.trim();
    if summary.is_empty() {
        return;
    }
    out.events.push(Event::PromiseMade {
        promisor: npc,
        promisee: NpcId(0),
        summary: summary.to_string(),
        by_day,
        when,
    });
    let due = match by_day {
        Some(d) => format!(" (by day {})", d),
        None => String::new(),
    };
    out.notebook.push(format!(
        "- {} promises: \"{}\"{}.",
        npc_name, summary, due
    ));
}

#[allow(clippy::too_many_arguments)]
fn apply_discover_location(
    world: &World,
    npc: NpcId,
    name: &str,
    location_kind: Option<&str>,
    description: Option<&str>,
    relocate: Option<ProposalScope>,
    why: Option<&str>,
    here: Option<LocationId>,
    npc_name: &str,
    when: Time,
    out: &mut AppliedProposals,
) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let Some(anchor) = here else { return };
    if world.location(anchor).is_none() {
        return;
    }
    // De-dup: if a location with this name already exists (case-insensitive
    // on the trimmed string), don't allocate a fresh node. The LLM may have
    // forgotten that the place was already on the map. In that case, fall
    // through to treating the proposal as a `Relocate` to the existing id
    // — provided relocate was requested and the existing location is
    // reachable.
    let existing: Option<LocationId> = world
        .locations
        .iter()
        .find(|(_, l)| l.name.eq_ignore_ascii_case(name))
        .map(|(id, _)| *id);
    if let Some(existing_id) = existing {
        if let Some(scope) = relocate {
            apply_relocate(world, npc, existing_id.0, scope, why, here, npc_name, when, out);
        }
        return;
    }

    // Allocate the next id — one past the current max.
    let next_id_n = world
        .locations
        .keys()
        .map(|id| id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .expect("LocationId overflow");
    let new_id = LocationId(next_id_n);

    let loc_kind = location_kind.unwrap_or("").trim().to_string();
    let desc = description.unwrap_or("").trim().to_string();
    out.events.push(Event::LocationDiscovered {
        id: new_id,
        name: name.to_string(),
        location_kind: loc_kind,
        description: desc,
        adjacent_to: anchor,
        when,
    });
    let why_clause = why.map(|w| format!(" — {}", w)).unwrap_or_default();
    out.notebook.push(format!(
        "- {} names a new place: {}{}.",
        npc_name, name, why_clause
    ));

    // Optional combined relocate: emit the move events directly, since the
    // freshly-created location is by construction adjacent to `anchor`. We
    // don't route back through `apply_relocate` because the world hasn't
    // been mutated yet (only events emitted); its reachability check would
    // fail against the un-mutated world.
    if let Some(scope) = relocate {
        match scope {
            ProposalScope::Lead | ProposalScope::Depart => {
                out.events.push(Event::Moved {
                    who: npc,
                    to: new_id,
                    when,
                });
                let line = match scope {
                    ProposalScope::Lead => format!("- {} heads for the {}, waving you on.", npc_name, name),
                    ProposalScope::Depart => format!("- {} walks off toward the {}.", npc_name, name),
                    ProposalScope::Joint => unreachable!(),
                };
                out.notebook.push(line);
                if matches!(scope, ProposalScope::Lead) {
                    out.followups.push(Action::FollowNpc {
                        npc,
                        to: new_id,
                    });
                }
            }
            ProposalScope::Joint => {
                out.notebook.push(format!(
                    "- {} suggests going to the {} together.",
                    npc_name, name
                ));
                out.followups.push(Action::FollowNpc {
                    npc,
                    to: new_id,
                });
            }
        }
    }
}

fn apply_break_off(
    npc: NpcId,
    reason: Option<&str>,
    npc_name: &str,
    when: Time,
    out: &mut AppliedProposals,
) {
    let reason_clause = reason.map(|r| format!(" — {}", r)).unwrap_or_default();
    // Record the break-off on the NPC's memory so the next npc_voice call
    // can surface it ("they broke off the last conversation").
    out.events.push(Event::Remembered {
        holder: npc,
        event: MemorableEvent {
            when,
            summary: format!("I broke off the conversation{}", reason_clause),
            valence: -0.2,
            related_to: Some(NpcId(0)),
            fact: None,
        },
        when,
    });
    out.notebook.push(format!(
        "- {} ends the conversation{}.",
        npc_name, reason_clause
    ));
    out.dialogue_ended = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Location, Npc, NpcId};

    fn t() -> Time {
        Time { day: 3, minute: 600 }
    }

    fn two_loc_world() -> World {
        let mut w = World::new(0);
        let mut tavern = Location::new(LocationId(1), "tavern");
        tavern.adjacent = vec![LocationId(2)];
        w.insert_location(tavern);
        let mut chapel = Location::new(LocationId(2), "chapel");
        chapel.adjacent = vec![LocationId(1)];
        w.insert_location(chapel);
        let mut player = Npc::new(NpcId(0), "Wren");
        player.location = Some(LocationId(1));
        w.insert_npc(player);
        let mut aldwin = Npc::new(NpcId(1), "Aldwin");
        aldwin.location = Some(LocationId(1));
        w.insert_npc(aldwin);
        let mut bryn = Npc::new(NpcId(2), "Bryn");
        bryn.location = Some(LocationId(2));
        w.insert_npc(bryn);
        w
    }

    #[test]
    fn relocate_lead_emits_npc_move_and_follow_action() {
        let w = two_loc_world();
        let prop = vec![Proposal::Relocate {
            to: 2,
            scope: ProposalScope::Lead,
            why: Some("come see this".into()),
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        assert_eq!(out.events.len(), 1);
        assert!(matches!(
            out.events[0],
            Event::Moved { who: NpcId(1), to: LocationId(2), .. }
        ));
        assert_eq!(out.followups.len(), 1);
        assert!(matches!(
            out.followups[0],
            Action::FollowNpc { npc: NpcId(1), to: LocationId(2) }
        ));
        assert!(!out.dialogue_ended);
    }

    #[test]
    fn relocate_depart_emits_move_but_no_follow_action() {
        let w = two_loc_world();
        let prop = vec![Proposal::Relocate {
            to: 2,
            scope: ProposalScope::Depart,
            why: None,
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        assert_eq!(out.events.len(), 1);
        assert!(out.followups.is_empty());
    }

    #[test]
    fn relocate_joint_offers_followup_without_moving_npc() {
        let w = two_loc_world();
        let prop = vec![Proposal::Relocate {
            to: 2,
            scope: ProposalScope::Joint,
            why: None,
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        assert!(out.events.is_empty(), "joint waits for player consent");
        assert_eq!(out.followups.len(), 1);
    }

    #[test]
    fn relocate_silently_drops_unreachable_destination() {
        let mut w = two_loc_world();
        // Sever the edge.
        w.locations.get_mut(&LocationId(1)).unwrap().adjacent.clear();
        let prop = vec![Proposal::Relocate {
            to: 2,
            scope: ProposalScope::Lead,
            why: None,
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        assert!(out.events.is_empty());
        assert!(out.followups.is_empty());
    }

    #[test]
    fn relocate_drops_offmap_and_unknown_locations() {
        let w = two_loc_world();
        for to in [0u32, 99u32] {
            let out = apply_proposals(
                &w,
                NpcId(1),
                &[Proposal::Relocate {
                    to,
                    scope: ProposalScope::Lead,
                    why: None,
                }],
                t(),
            );
            assert!(out.events.is_empty());
        }
    }

    #[test]
    fn fetch_moves_named_npc_to_current_room() {
        let w = two_loc_world();
        let prop = vec![Proposal::Fetch {
            who: 2,
            why: Some("you should hear this".into()),
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        assert_eq!(out.events.len(), 1);
        assert!(matches!(
            out.events[0],
            Event::Moved { who: NpcId(2), to: LocationId(1), .. }
        ));
    }

    #[test]
    fn fetch_drops_unknown_or_player_or_self() {
        let w = two_loc_world();
        for who in [0u32, 1u32, 99u32] {
            let out = apply_proposals(
                &w,
                NpcId(1),
                &[Proposal::Fetch { who, why: None }],
                t(),
            );
            assert!(out.events.is_empty(), "should drop for who={}", who);
        }
    }

    #[test]
    fn fetch_is_noop_when_target_already_here() {
        let mut w = two_loc_world();
        w.npcs.get_mut(&NpcId(2)).unwrap().location = Some(LocationId(1));
        let out = apply_proposals(
            &w,
            NpcId(1),
            &[Proposal::Fetch { who: 2, why: None }],
            t(),
        );
        assert!(out.events.is_empty());
    }

    #[test]
    fn hand_over_emits_transfer_and_small_trust_bump() {
        let w = two_loc_world();
        let out = apply_proposals(
            &w,
            NpcId(1),
            &[Proposal::HandOver {
                item: "a folded note".into(),
                why: None,
            }],
            t(),
        );
        assert_eq!(out.events.len(), 2);
        assert!(matches!(out.events[0], Event::HandedOver { .. }));
        match &out.events[1] {
            Event::RelationshipShift { subject, object, delta, .. } => {
                assert_eq!(*subject, NpcId(1));
                assert_eq!(*object, NpcId(0));
                assert!(delta.trust > 0);
                assert!(delta.affection > 0);
            }
            _ => panic!("second event should be relationship shift"),
        }
    }

    #[test]
    fn hand_over_drops_empty_item() {
        let w = two_loc_world();
        let out = apply_proposals(
            &w,
            NpcId(1),
            &[Proposal::HandOver {
                item: "   ".into(),
                why: None,
            }],
            t(),
        );
        assert!(out.events.is_empty());
    }

    #[test]
    fn promise_emits_promise_made_and_notebook_line() {
        let w = two_loc_world();
        let out = apply_proposals(
            &w,
            NpcId(1),
            &[Proposal::Promise {
                summary: "I will testify".into(),
                by_day: Some(7),
            }],
            t(),
        );
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            Event::PromiseMade { promisor, promisee, summary, by_day, .. } => {
                assert_eq!(*promisor, NpcId(1));
                assert_eq!(*promisee, NpcId(0));
                assert!(summary.contains("testify"));
                assert_eq!(*by_day, Some(7));
            }
            _ => panic!(),
        }
        assert!(out.notebook[0].contains("promises"));
    }

    #[test]
    fn break_off_records_memorable_and_flags_dialogue_ended() {
        let w = two_loc_world();
        let out = apply_proposals(
            &w,
            NpcId(1),
            &[Proposal::BreakOff {
                reason: Some("too tired".into()),
            }],
            t(),
        );
        assert!(out.dialogue_ended);
        assert_eq!(out.events.len(), 1);
        assert!(matches!(out.events[0], Event::Remembered { .. }));
        assert!(out.notebook[0].contains("ends the conversation"));
    }

    #[test]
    fn discover_location_emits_location_discovered_with_fresh_id() {
        let w = two_loc_world();
        let prop = vec![Proposal::DiscoverLocation {
            name: "the bunks".into(),
            location_kind: Some("bunk".into()),
            description: Some("close-stacked racks, dim lamp".into()),
            relocate: None,
            why: Some("close to here".into()),
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            Event::LocationDiscovered {
                id,
                name,
                location_kind,
                description,
                adjacent_to,
                ..
            } => {
                // Next id past the existing 0, 1, 2 → 3.
                assert_eq!(id.0, 3);
                assert_eq!(name, "the bunks");
                assert_eq!(location_kind, "bunk");
                assert!(description.contains("racks"));
                assert_eq!(*adjacent_to, LocationId(1));
            }
            _ => panic!("expected LocationDiscovered"),
        }
        assert!(out.notebook[0].contains("names a new place"));
    }

    #[test]
    fn discover_location_with_lead_relocate_emits_move_and_followup() {
        let w = two_loc_world();
        let prop = vec![Proposal::DiscoverLocation {
            name: "the bunks".into(),
            location_kind: None,
            description: None,
            relocate: Some(ProposalScope::Lead),
            why: None,
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        // LocationDiscovered + Moved
        assert_eq!(out.events.len(), 2);
        match &out.events[0] {
            Event::LocationDiscovered { id, .. } => assert_eq!(id.0, 3),
            _ => panic!(),
        }
        match &out.events[1] {
            Event::Moved { who, to, .. } => {
                assert_eq!(*who, NpcId(1));
                assert_eq!(to.0, 3);
            }
            _ => panic!(),
        }
        // Lead → FollowNpc follow-up at the new id.
        assert_eq!(out.followups.len(), 1);
        assert!(matches!(
            out.followups[0],
            Action::FollowNpc { npc: NpcId(1), to } if to.0 == 3
        ));
    }

    #[test]
    fn discover_location_with_joint_relocate_offers_followup_without_moving() {
        let w = two_loc_world();
        let prop = vec![Proposal::DiscoverLocation {
            name: "the storage hatch".into(),
            location_kind: None,
            description: None,
            relocate: Some(ProposalScope::Joint),
            why: None,
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        // LocationDiscovered only — joint waits for player consent.
        assert_eq!(out.events.len(), 1);
        assert!(matches!(out.events[0], Event::LocationDiscovered { .. }));
        assert_eq!(out.followups.len(), 1);
    }

    #[test]
    fn discover_location_dedups_against_existing_name_case_insensitive() {
        let w = two_loc_world();
        // "chapel" is at id 2 — discovery names "Chapel" with relocate=lead.
        // Engine should NOT create a new node; instead it should route to the
        // existing id as a normal Relocate.
        let prop = vec![Proposal::DiscoverLocation {
            name: "Chapel".into(),
            location_kind: None,
            description: None,
            relocate: Some(ProposalScope::Lead),
            why: None,
        }];
        let out = apply_proposals(&w, NpcId(1), &prop, t());
        assert_eq!(out.events.len(), 1);
        match &out.events[0] {
            Event::Moved { who, to, .. } => {
                assert_eq!(*who, NpcId(1));
                assert_eq!(*to, LocationId(2));
            }
            _ => panic!("expected Moved into existing chapel, got {:?}", out.events[0]),
        }
    }

    #[test]
    fn discover_location_drops_empty_name() {
        let w = two_loc_world();
        let out = apply_proposals(
            &w,
            NpcId(1),
            &[Proposal::DiscoverLocation {
                name: "   ".into(),
                location_kind: None,
                description: None,
                relocate: None,
                why: None,
            }],
            t(),
        );
        assert!(out.events.is_empty());
    }

    #[test]
    fn discover_location_applied_via_apply_event_inserts_with_adjacency() {
        use crate::systems::apply_event;
        let mut w = two_loc_world();
        let ev = Event::LocationDiscovered {
            id: LocationId(3),
            name: "the bunks".into(),
            location_kind: "bunk".into(),
            description: "narrow racks".into(),
            adjacent_to: LocationId(1),
            when: t(),
        };
        apply_event(&mut w, &ev);
        // New location exists with the right name.
        assert_eq!(w.location(LocationId(3)).unwrap().name, "the bunks");
        // Bidirectional adjacency.
        assert!(w.location(LocationId(3)).unwrap().adjacent.contains(&LocationId(1)));
        assert!(w.location(LocationId(1)).unwrap().adjacent.contains(&LocationId(3)));
    }

    #[test]
    fn multiple_proposals_apply_in_order() {
        let w = two_loc_world();
        let out = apply_proposals(
            &w,
            NpcId(1),
            &[
                Proposal::HandOver {
                    item: "a key".into(),
                    why: None,
                },
                Proposal::Relocate {
                    to: 2,
                    scope: ProposalScope::Lead,
                    why: None,
                },
            ],
            t(),
        );
        // HandedOver + RelationshipShift + Moved
        assert_eq!(out.events.len(), 3);
        assert!(matches!(out.events[0], Event::HandedOver { .. }));
        assert!(matches!(out.events[1], Event::RelationshipShift { .. }));
        assert!(matches!(out.events[2], Event::Moved { .. }));
        assert_eq!(out.followups.len(), 1);
    }
}
