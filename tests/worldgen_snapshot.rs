use std::path::PathBuf;

use book::content::ContentRegistry;
use book::engine::worldgen;
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

#[derive(serde::Serialize)]
struct NpcRow {
    id: u32,
    name: String,
    age: u32,
    sex: String,
    occupation: Option<String>,
    spouse: Option<u32>,
    n_parents: usize,
    n_children: usize,
    location_kind: String,
}

#[derive(serde::Serialize)]
struct LocationRow {
    id: u32,
    name: String,
    kind: String,
    adjacent: Vec<u32>,
}

#[derive(serde::Serialize)]
struct IncidentSummary {
    village: String,
    kind: &'static str,
    n_principals: usize,
    n_witnesses: usize,
    n_public_facts: usize,
    summary: String,
    deadline_days: u32,
}

fn npc_roster(world: &book::world::World) -> Vec<NpcRow> {
    let mut rows: Vec<NpcRow> = world
        .npcs
        .values()
        .map(|n| {
            let location_kind = n
                .location
                .and_then(|id| world.location(id))
                .map(|l| l.kind.clone())
                .unwrap_or_default();
            NpcRow {
                id: n.id.0,
                name: n.name.clone(),
                age: n.age,
                sex: format!("{:?}", n.sex),
                occupation: n.occupation.clone(),
                spouse: n.spouse.map(|s| s.0),
                n_parents: n.parents.len(),
                n_children: n.children.len(),
                location_kind,
            }
        })
        .collect();
    rows.sort_by_key(|r| r.id);
    rows
}

fn location_list(world: &book::world::World) -> Vec<LocationRow> {
    let mut rows: Vec<LocationRow> = world
        .locations
        .values()
        .map(|l| {
            let mut adj: Vec<u32> = l.adjacent.iter().map(|id| id.0).collect();
            adj.sort();
            LocationRow {
                id: l.id.0,
                name: l.name.clone(),
                kind: l.kind.clone(),
                adjacent: adj,
            }
        })
        .collect();
    rows.sort_by_key(|r| r.id);
    rows
}

fn incident_summary(result: &worldgen::WorldgenResult) -> IncidentSummary {
    use book::engine::IncidentKind;
    let kind = match &result.incident.kind {
        IncidentKind::Death { .. } => "death",
        IncidentKind::Disappearance { .. } => "disappearance",
        IncidentKind::Scandal { .. } => "scandal",
    };
    IncidentSummary {
        village: result.world.village_name.clone(),
        kind,
        n_principals: result.incident.principals.len(),
        n_witnesses: result.incident.witnesses.len(),
        n_public_facts: result.incident.public_facts.len(),
        summary: result.incident.summary.clone(),
        deadline_days: result
            .world
            .deadline_day
            .saturating_sub(result.world.start_day),
    }
}

#[test]
fn village_seed_42_npc_roster() {
    let result = worldgen::run(&registry(), 42, &fake_player()).unwrap();
    insta::assert_yaml_snapshot!("village_seed_42_npc_roster", npc_roster(&result.world));
}

#[test]
fn village_seed_42_locations() {
    let result = worldgen::run(&registry(), 42, &fake_player()).unwrap();
    insta::assert_yaml_snapshot!("village_seed_42_locations", location_list(&result.world));
}

#[test]
fn village_seed_42_incident_summary() {
    let result = worldgen::run(&registry(), 42, &fake_player()).unwrap();
    insta::assert_yaml_snapshot!("village_seed_42_incident_summary", incident_summary(&result));
}

#[test]
fn village_seed_42_seeds_secrets_from_the_pack() {
    let reg = registry();
    let result = worldgen::run(&reg, 42, &fake_player()).unwrap();
    let pack_categories: std::collections::BTreeSet<&str> = reg
        .setting
        .secrets
        .categories
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let adults: Vec<&book::world::Npc> = result
        .world
        .npcs
        .values()
        .filter(|n| n.id != NpcId(0) && n.age >= 18 && !n.dead)
        .collect();
    let with_secret = adults.iter().filter(|n| !n.secrets.is_empty()).count();
    let total_adults = adults.len();
    assert!(total_adults > 0, "fixture should produce adult NPCs");
    // Distribution is probabilistic but should clear a generous lower bound
    // on this seed — sanity check, not a precision claim.
    assert!(
        with_secret * 10 >= total_adults * 2,
        "expected at least 20% of {} adults to hold a secret, got {}",
        total_adults,
        with_secret
    );
    // Every assigned category must be one the pack declares — the LLM did
    // not invent these.
    for npc in &adults {
        for s in &npc.secrets {
            assert!(
                pack_categories.contains(s.category.as_str()),
                "NPC {:?} has unknown secret category {:?}",
                npc.name,
                s.category,
            );
        }
    }
}
