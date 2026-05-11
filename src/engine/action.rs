//! Player actions available at any given moment in a scene.
//!
//! The engine decides which actions are mechanically possible; the LLM (in
//! `llm::builders::options_request`) writes the player-facing text for them.
//! Each action's stable, engine-side label is the input to the options cache.

use crate::world::{LocationId, NpcId, World};

/// One action the player can take in the current scene.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Walk to an adjacent location. Ends the current scene and advances the
    /// in-world clock.
    Move(LocationId),
    /// Stay where you are; let time pass. Ends the current scene.
    Wait,
    /// Strike up a conversation with a co-located NPC. Does not end the scene.
    Interview(NpcId),
    /// Examine the present location and the people in it. Does not end the
    /// scene; produces a `Witnessed` event for the player.
    Observe,
    /// Open the read-only notebook view.
    OpenNotebook,
    /// Open the accusation submode: name a culprit and close the case.
    Accuse,
    /// Walk away from the village. Closes the case as `LeftTown`.
    LeaveTown,
}

impl Action {
    /// True if invoking this action consumes one of the day's four events.
    pub fn ends_scene(&self) -> bool {
        matches!(self, Action::Move(_) | Action::Wait)
    }
}

/// State-aware list of every action mechanically available at `here`.
///
/// Order is stable so the options cache key (a hash of the engine labels)
/// behaves deterministically across rebuilds of the same scene.
pub fn available_actions(world: &World, here: LocationId) -> Vec<Action> {
    let mut out = Vec::new();

    // Movement edges first, sorted by destination id for stability.
    if let Some(loc) = world.location(here) {
        let mut adj = loc.adjacent.clone();
        adj.sort_by_key(|id| id.0);
        for dest in adj {
            // Skip the off-map sentinel as a destination — the player can't
            // just stroll off the world.
            if dest.0 == 0 {
                continue;
            }
            out.push(Action::Move(dest));
        }
    }

    // Interview each co-located non-player, non-dead NPC, in id order.
    let mut present: Vec<NpcId> = world
        .npcs
        .values()
        .filter(|n| !n.dead && n.id.0 != 0 && n.location == Some(here))
        .map(|n| n.id)
        .collect();
    present.sort_by_key(|id| id.0);
    for id in present {
        out.push(Action::Interview(id));
    }

    out.push(Action::Observe);
    out.push(Action::OpenNotebook);
    out.push(Action::Wait);
    out.push(Action::Accuse);
    out.push(Action::LeaveTown);
    out
}

/// Engine-side, stable label for a single action. Used as the cache key for
/// LLM-flavored option text and as the fallback label when the LLM call fails.
pub fn engine_label(world: &World, action: &Action) -> String {
    match action {
        Action::Move(to) => {
            let name = world
                .location(*to)
                .map(|l| l.name.as_str())
                .unwrap_or("somewhere");
            format!("move to {}", name)
        }
        Action::Wait => "wait here a while".to_string(),
        Action::Interview(npc) => {
            let name = world
                .npc(*npc)
                .map(|n| n.name.as_str())
                .unwrap_or("a stranger");
            format!("speak with {}", name)
        }
        Action::Observe => "observe the room".to_string(),
        Action::OpenNotebook => "open the notebook".to_string(),
        Action::Accuse => "name the culprit".to_string(),
        Action::LeaveTown => {
            // The variant name is historical ("town"); the visible label
            // tracks the setting's place_kind so a ship reads "leave the ship".
            let place = if world.place_kind.is_empty() {
                "village"
            } else {
                world.place_kind.as_str()
            };
            format!("leave the {}", place)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Location, Npc, NpcId};

    fn world_two_locs_one_npc() -> World {
        let mut w = World::new(0);
        w.insert_location(Location::new(LocationId(0), "elsewhere"));
        let mut tavern = Location::new(LocationId(1), "tavern");
        tavern.adjacent = vec![LocationId(2), LocationId(0)];
        w.insert_location(tavern);
        let chapel = Location::new(LocationId(2), "chapel");
        w.insert_location(chapel);

        let mut player = Npc::new(NpcId(0), "Player");
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
    fn available_skips_offmap_destinations() {
        let w = world_two_locs_one_npc();
        let acts = available_actions(&w, LocationId(1));
        assert!(acts.iter().any(|a| matches!(a, Action::Move(id) if id.0 == 2)));
        assert!(!acts.iter().any(|a| matches!(a, Action::Move(id) if id.0 == 0)));
    }

    #[test]
    fn available_lists_present_npcs_but_not_player() {
        let w = world_two_locs_one_npc();
        let acts = available_actions(&w, LocationId(1));
        let interview: Vec<&Action> = acts
            .iter()
            .filter(|a| matches!(a, Action::Interview(_)))
            .collect();
        assert_eq!(interview.len(), 1);
        assert_eq!(interview[0], &Action::Interview(NpcId(1)));
    }

    #[test]
    fn available_includes_standard_trio() {
        let w = world_two_locs_one_npc();
        let acts = available_actions(&w, LocationId(1));
        assert!(acts.contains(&Action::Observe));
        assert!(acts.contains(&Action::OpenNotebook));
        assert!(acts.contains(&Action::Wait));
        assert!(acts.contains(&Action::Accuse));
        assert!(acts.contains(&Action::LeaveTown));
    }

    #[test]
    fn ends_scene_classification() {
        assert!(Action::Move(LocationId(2)).ends_scene());
        assert!(Action::Wait.ends_scene());
        assert!(!Action::Interview(NpcId(1)).ends_scene());
        assert!(!Action::Observe.ends_scene());
        assert!(!Action::OpenNotebook.ends_scene());
    }

    #[test]
    fn engine_labels_render_useful_strings() {
        let w = world_two_locs_one_npc();
        assert_eq!(engine_label(&w, &Action::Move(LocationId(2))), "move to chapel");
        assert_eq!(engine_label(&w, &Action::Interview(NpcId(1))), "speak with Aldwin");
        assert_eq!(engine_label(&w, &Action::Wait), "wait here a while");
        assert_eq!(engine_label(&w, &Action::Observe), "observe the room");
        assert_eq!(engine_label(&w, &Action::OpenNotebook), "open the notebook");
        assert_eq!(engine_label(&w, &Action::Accuse), "name the culprit");
        // Default place_kind on a fresh World is "village" — see `World::new`.
        assert_eq!(engine_label(&w, &Action::LeaveTown), "leave the village");
    }
}
