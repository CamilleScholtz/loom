//! Integration tests for the Phase 6 play loop. We exercise the engine-level
//! parts of the loop (Action, tick_scene, observe, notebook persistence) without
//! the TUI. The LLM is the `FakeClient` so these tests are deterministic and
//! free.

use std::path::PathBuf;

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use tempfile::TempDir;

use loom::content::ContentRegistry;
use loom::engine::{
    self, Action, Event, EventLog, Fact, Time, available_actions,
};
use loom::engine::worldgen;
use loom::storage::SaveDir;
use loom::systems::apply_event;
use loom::world::{NpcId, PlayerCharacter};

fn registry() -> ContentRegistry {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("content");
    ContentRegistry::load(&root, "medieval", "investigator").unwrap()
}

fn fake_player() -> PlayerCharacter {
    PlayerCharacter {
        id: NpcId(0),
        name: "Quintus Wren".into(),
        vocation: "investigator".into(),
        virtue: "Shrewd".into(),
        vice: "Suspicious".into(),
        inclination: "Scholarly".into(),
        background: "Itinerant scholar".into(),
    }
}

#[test]
fn observe_appends_witnessed_event_and_notebook_line() {
    let reg = registry();
    let result = worldgen::run(&reg, 7, &fake_player()).unwrap();
    let mut world = result.world;
    let player_loc = world.npc(NpcId(0)).and_then(|p| p.location).unwrap();
    let dir = TempDir::new().unwrap();
    let save = SaveDir::open(dir.path()).unwrap();
    let mut log = EventLog::open(save.event_log_path()).unwrap();

    let before = world
        .npc(NpcId(0))
        .map(|p| p.knowledge.len())
        .unwrap_or(0);

    let now = Time {
        day: world.day,
        minute: world.clock_minutes,
    };
    let here_name = world.location(player_loc).map(|l| l.name.clone()).unwrap();
    let summary = format!("{} in the morning: present are some folk", here_name);
    let fact_id = world.intern_fact(Fact {
        kind: "scene".into(),
        summary: summary.clone(),
    });
    let ev = Event::Witnessed {
        observer: NpcId(0),
        fact: fact_id,
        when: now,
    };
    apply_event(&mut world, &ev);
    log.append(&ev).unwrap();
    save.notebook_append(&format!("- **Day {} / morning, {}.** here.", world.day, here_name))
        .unwrap();

    let after = world.npc(NpcId(0)).unwrap().knowledge.len();
    assert_eq!(after, before + 1);

    let body = save.notebook_read().unwrap();
    assert!(body.contains(&here_name));
}

#[test]
fn four_scene_ticks_with_clock_at_morning_do_not_roll_day() {
    let reg = registry();
    let result = worldgen::run(&reg, 7, &fake_player()).unwrap();
    let mut world = result.world;
    world.clock_minutes = 480; // 08:00
    let start_day = world.day;
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let now = Time {
        day: world.day,
        minute: world.clock_minutes,
    };
    for _ in 0..4 {
        let (_evs, rolled) = engine::sim::tick_scene(&mut world, &reg, &mut rng, now);
        assert!(!rolled, "08:00 + 4×3h = 20:00 should not cross midnight");
    }
    // 480 + 4*180 = 1200.
    assert_eq!(world.clock_minutes, 1200);
    assert_eq!(world.day, start_day);
}

#[test]
fn eight_scene_ticks_with_clock_at_morning_roll_day_once() {
    let reg = registry();
    let result = worldgen::run(&reg, 7, &fake_player()).unwrap();
    let mut world = result.world;
    world.clock_minutes = 480;
    let start_day = world.day;
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let now = Time {
        day: world.day,
        minute: world.clock_minutes,
    };
    let mut rolls = 0;
    for _ in 0..8 {
        let (_evs, rolled) = engine::sim::tick_scene(&mut world, &reg, &mut rng, now);
        if rolled {
            rolls += 1;
        }
    }
    assert_eq!(rolls, 1);
    assert_eq!(world.day, start_day + 1);
    assert_eq!(world.clock_minutes, 480);
}

#[test]
fn available_actions_reflect_adjacent_locations_and_present_npcs() {
    let reg = registry();
    let result = worldgen::run(&reg, 7, &fake_player()).unwrap();
    let world = result.world;
    let here = world.npc(NpcId(0)).and_then(|p| p.location).unwrap();
    let acts = available_actions(&world, here);
    let n_move = acts
        .iter()
        .filter(|a| matches!(a, Action::Move(_)))
        .count();
    let adj = world.location(here).map(|l| l.adjacent.len()).unwrap_or(0);
    // Move count may be slightly fewer than adjacency if any adjacent is the
    // off-map sentinel — but should be > 0 for a worldgen-produced village.
    assert!(n_move > 0);
    assert!(n_move <= adj);
    assert!(acts.contains(&Action::Observe));
    assert!(acts.contains(&Action::OpenNotebook));
    assert!(acts.contains(&Action::Wait));
}

#[test]
fn notebook_round_trip_through_save_dir() {
    let dir = TempDir::new().unwrap();
    let save = SaveDir::open(dir.path()).unwrap();
    assert_eq!(save.notebook_read().unwrap(), "");
    save.notebook_append("- first entry").unwrap();
    save.notebook_append("- second entry").unwrap();
    let body = save.notebook_read().unwrap();
    assert!(body.contains("first entry"));
    assert!(body.contains("second entry"));
}

#[test]
fn dialogue_path_emits_two_spoken_events() {
    let reg = registry();
    let result = worldgen::run(&reg, 7, &fake_player()).unwrap();
    let mut world = result.world;
    let here = world.npc(NpcId(0)).and_then(|p| p.location).unwrap();
    // Find any other NPC at the player's location, or move one there for the test.
    let target_npc = match world
        .npcs
        .values()
        .find(|n| !n.dead && n.id.0 != 0 && n.location == Some(here))
        .map(|n| n.id)
    {
        Some(id) => id,
        None => {
            // Move the first non-player NPC to the player's location.
            let id = world
                .npcs
                .values()
                .find(|n| !n.dead && n.id.0 != 0)
                .map(|n| n.id)
                .expect("at least one other NPC");
            let ev = Event::Moved {
                who: id,
                to: here,
                when: Time { day: world.day, minute: world.clock_minutes },
            };
            apply_event(&mut world, &ev);
            id
        }
    };

    let dir = TempDir::new().unwrap();
    let save = SaveDir::open(dir.path()).unwrap();
    let mut log = EventLog::open(save.event_log_path()).unwrap();
    let now = Time {
        day: world.day,
        minute: world.clock_minutes,
    };

    let ev1 = Event::Spoken {
        speaker: NpcId(0),
        listeners: vec![target_npc],
        line: "Where were you last night?".into(),
        when: now,
    };
    apply_event(&mut world, &ev1);
    log.append(&ev1).unwrap();
    let ev2 = Event::Spoken {
        speaker: target_npc,
        listeners: vec![NpcId(0)],
        line: "[stub reply]".into(),
        when: now,
    };
    apply_event(&mut world, &ev2);
    log.append(&ev2).unwrap();
    drop(log);

    let events = EventLog::read_all(save.event_log_path()).unwrap();
    let spokes: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::Spoken { .. }))
        .collect();
    assert!(spokes.len() >= 2);
}
