use std::collections::BTreeSet;
use std::path::PathBuf;

use tempfile::TempDir;

use loom::content::ContentRegistry;
use loom::engine::worldgen;
use loom::storage::SaveDir;
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
        sex: loom::world::Sex::Female,
    }
}

fn assert_world_valid(world: &loom::world::World) {
    let npc_ids: BTreeSet<NpcId> = world.npcs.keys().copied().collect();
    let loc_ids: BTreeSet<_> = world.locations.keys().copied().collect();
    let fact_ids: BTreeSet<_> = world.facts.keys().copied().collect();

    for npc in world.npcs.values() {
        if let Some(loc) = npc.location {
            assert!(loc_ids.contains(&loc), "{} at unknown location", npc.name);
        }
        if let Some(s) = npc.spouse {
            assert!(npc_ids.contains(&s), "{} spouse is unknown", npc.name);
        }
        for p in &npc.parents {
            assert!(npc_ids.contains(p), "{} parent is unknown", npc.name);
        }
        for c in &npc.children {
            assert!(npc_ids.contains(c), "{} child is unknown", npc.name);
        }
        if let Some(e) = npc.employer {
            assert!(npc_ids.contains(&e), "{} employer is unknown", npc.name);
        }
        for fact in npc.knowledge.keys() {
            assert!(
                fact_ids.contains(fact),
                "{} knows fact {:?} that doesn't exist",
                npc.name,
                fact
            );
        }
    }

    for (&(subj, obj), _) in world.relationships_iter() {
        assert!(npc_ids.contains(&subj), "relationship subject {:?} unknown", subj);
        assert!(npc_ids.contains(&obj), "relationship object {:?} unknown", obj);
    }

    for (id, loc) in &world.locations {
        for nbr in &loc.adjacent {
            assert!(
                loc_ids.contains(nbr),
                "location {} adjacent to unknown {}",
                id.0,
                nbr.0
            );
        }
    }
}

#[test]
fn three_seeds_produce_valid_worlds() {
    let reg = registry();
    for seed in [1u64, 42, 7777] {
        let r = worldgen::run(&reg, seed, &fake_player())
            .unwrap_or_else(|e| panic!("worldgen failed for seed {}: {}", seed, e));
        assert!(!r.world.village_name.is_empty());
        assert!(r.world.npc(NpcId(0)).is_some(), "player not placed for seed {}", seed);
        assert!(r.world.deadline_day > r.world.start_day);
        assert_world_valid(&r.world);
    }
}

#[test]
fn save_and_reload_round_trips() {
    let reg = registry();
    let dir = TempDir::new().unwrap();
    let r = worldgen::run(&reg, 123, &fake_player()).unwrap();

    let save = SaveDir::open(dir.path()).unwrap();
    save.save_world(&r.world).unwrap();
    save.save_case(&r.incident).unwrap();
    save.save_player(&fake_player()).unwrap();

    let loaded = save.load_world().unwrap();
    assert_eq!(r.world, loaded);
    let case_back = save.load_case().unwrap();
    assert_eq!(r.incident, case_back);

    // Re-save and verify the world file is byte-identical (relationships,
    // knowledge per-NPC, facts.toml all stable across save/load/save).
    let dir2 = TempDir::new().unwrap();
    let save2 = SaveDir::open(dir2.path()).unwrap();
    save2.save_world(&loaded).unwrap();
    let a = std::fs::read(save.world_toml_path()).unwrap();
    let b = std::fs::read(save2.world_toml_path()).unwrap();
    assert_eq!(a, b, "world.toml differs after round-trip");
    let a = std::fs::read(save.facts_toml_path()).unwrap();
    let b = std::fs::read(save2.facts_toml_path()).unwrap();
    assert_eq!(a, b, "facts.toml differs after round-trip");
}

#[test]
fn same_seed_produces_same_world() {
    let reg = registry();
    let a = worldgen::run(&reg, 555, &fake_player()).unwrap();
    let b = worldgen::run(&reg, 555, &fake_player()).unwrap();
    assert_eq!(a.world.village_name, b.world.village_name);
    assert_eq!(a.world.npcs.len(), b.world.npcs.len());
    assert_eq!(a.incident, b.incident);
    assert_eq!(a.player_hook, b.player_hook);
}
