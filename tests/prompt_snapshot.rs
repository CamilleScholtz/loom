//! Snapshot tests for the Phase 6 prompt builders. Builders are pure functions
//! of state; snapshotting their output catches accidental prompt drift.

use std::path::PathBuf;

use book::config::DEFAULT_MODEL;
use book::content::ContentRegistry;
use book::engine::{Action, worldgen};
use book::llm::builders;
use book::world::{NpcId, PlayerCharacter};

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
fn scene_open_prompt_seed_42() {
    let result = worldgen::run(&registry(), 42, &fake_player()).unwrap();
    let here = result.world.npc(NpcId(0)).and_then(|p| p.location).unwrap();
    let req = builders::scene_open_request(&result.world, &registry(), here, DEFAULT_MODEL);
    let sys = &req.messages[0].content;
    insta::assert_snapshot!("scene_open_seed_42", sys);
}

#[test]
fn options_prompt_seed_42_standard_action_set() {
    let result = worldgen::run(&registry(), 42, &fake_player()).unwrap();
    let here = result.world.npc(NpcId(0)).and_then(|p| p.location).unwrap();
    // A small fixed action set, independent of present NPCs/adjacency so the
    // snapshot is stable as worldgen tuning shifts: just the universal three.
    let acts = vec![Action::Observe, Action::OpenNotebook, Action::Wait];
    let req = builders::options_request(&result.world, &registry(), here, &acts, DEFAULT_MODEL);
    let sys = &req.messages[0].content;
    insta::assert_snapshot!("options_seed_42_universal", sys);
}
