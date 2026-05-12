use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::engine::{Distortion, Fact, Incident, Time};
use crate::systems::{Edge, KnowledgeEdge, KnowledgeSource};
use crate::world::{FactId, Location, LocationId, Npc, NpcId, PlayerCharacter, World};

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("path is not a directory: {0}")]
    NotADirectory(PathBuf),
}

pub struct SaveDir {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct WorldMeta {
    #[serde(with = "u64_str")]
    seed: u64,
    day: u32,
    #[serde(with = "u64_str")]
    event_index: u64,
    clock_minutes: u32,
    next_fact_id: u32,
    #[serde(default)]
    village_name: String,
    #[serde(default = "default_place_kind_meta")]
    place_kind: String,
    #[serde(default = "default_inhabitant_singular_meta")]
    inhabitant_singular: String,
    #[serde(default = "default_inhabitant_plural_meta")]
    inhabitant_plural: String,
    #[serde(default)]
    start_day: u32,
    #[serde(default)]
    deadline_day: u32,
    npcs: Vec<NpcId>,
    locations: Vec<LocationId>,
}

fn default_place_kind_meta() -> String {
    "village".into()
}
fn default_inhabitant_singular_meta() -> String {
    "villager".into()
}
fn default_inhabitant_plural_meta() -> String {
    "villagers".into()
}

#[derive(Serialize, Deserialize)]
struct FactsFile {
    #[serde(default, rename = "fact")]
    facts: Vec<FactEntry>,
}

#[derive(Serialize, Deserialize)]
struct FactEntry {
    id: FactId,
    kind: String,
    summary: String,
}

#[derive(Serialize, Deserialize, Default)]
struct RelationshipsFile {
    #[serde(default, rename = "edge")]
    edges: Vec<RelationshipEntry>,
}

#[derive(Serialize, Deserialize)]
struct RelationshipEntry {
    object: NpcId,
    affection: i32,
    trust: i32,
    debt: i32,
    grievance: i32,
    #[serde(default)]
    attraction: i32,
    #[serde(default)]
    stage: crate::systems::relationships::RomanceStage,
    #[serde(default)]
    stage_since: crate::engine::Time,
}

#[derive(Serialize, Deserialize, Default)]
struct KnowledgeFile {
    #[serde(default, rename = "edge")]
    edges: Vec<KnowledgeEntry>,
}

#[derive(Serialize, Deserialize)]
struct KnowledgeEntry {
    fact: FactId,
    source: KnowledgeSource,
    confidence: f32,
    distortion: Distortion,
    acquired_at: Time,
}

impl SaveDir {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)
            .with_context(|| format!("creating save dir {}", root.display()))?;
        if !root.is_dir() {
            return Err(StorageError::NotADirectory(root).into());
        }
        fs::create_dir_all(root.join("characters"))?;
        fs::create_dir_all(root.join("locations"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn event_log_path(&self) -> PathBuf {
        self.root.join("events.log")
    }

    pub fn world_toml_path(&self) -> PathBuf {
        self.root.join("world.toml")
    }

    pub fn facts_toml_path(&self) -> PathBuf {
        self.root.join("facts.toml")
    }

    pub fn case_toml_path(&self) -> PathBuf {
        self.root.join("case.toml")
    }

    pub fn player_toml_path(&self) -> PathBuf {
        self.root.join("player.toml")
    }

    pub fn notebook_path(&self) -> PathBuf {
        self.root.join("notebook.md")
    }

    pub fn notebook_append(&self, line: &str) -> Result<()> {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.notebook_path())
            .with_context(|| format!("opening {}", self.notebook_path().display()))?;
        file.write_all(line.as_bytes())?;
        if !line.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        Ok(())
    }

    pub fn notebook_read(&self) -> Result<String> {
        let path = self.notebook_path();
        if !path.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }

    pub fn characters_dir(&self) -> PathBuf {
        self.root.join("characters")
    }

    pub fn locations_dir(&self) -> PathBuf {
        self.root.join("locations")
    }

    fn character_dir(&self, id: NpcId, name: &str) -> PathBuf {
        self.characters_dir()
            .join(format!("{}-{}", id.0, slug(name)))
    }

    fn location_file(&self, id: LocationId, name: &str) -> PathBuf {
        self.locations_dir()
            .join(format!("{}-{}.toml", id.0, slug(name)))
    }

    pub fn save_world(&self, world: &World) -> Result<()> {
        clear_dir_contents(&self.characters_dir())?;
        clear_dir_contents(&self.locations_dir())?;

        let meta = WorldMeta {
            seed: world.seed,
            day: world.day,
            event_index: world.event_index,
            clock_minutes: world.clock_minutes,
            next_fact_id: world.next_fact_id,
            village_name: world.village_name.clone(),
            place_kind: world.place_kind.clone(),
            inhabitant_singular: world.inhabitant_singular.clone(),
            inhabitant_plural: world.inhabitant_plural.clone(),
            start_day: world.start_day,
            deadline_day: world.deadline_day,
            npcs: world.npcs.keys().copied().collect(),
            locations: world.locations.keys().copied().collect(),
        };
        write_toml(&self.world_toml_path(), &meta)?;

        let facts_file = FactsFile {
            facts: world
                .facts
                .iter()
                .map(|(id, f)| FactEntry {
                    id: *id,
                    kind: f.kind.clone(),
                    summary: f.summary.clone(),
                })
                .collect(),
        };
        write_toml(&self.facts_toml_path(), &facts_file)?;

        for npc in world.npcs.values() {
            let dir = self.character_dir(npc.id, &npc.name);
            fs::create_dir_all(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;
            write_toml(&dir.join("profile.toml"), npc)?;

            // Relationships outgoing from this NPC.
            let edges: Vec<RelationshipEntry> = world
                .relationships_from(npc.id)
                .map(|(object, e)| RelationshipEntry {
                    object,
                    affection: e.affection,
                    trust: e.trust,
                    debt: e.debt,
                    grievance: e.grievance,
                    attraction: e.attraction,
                    stage: e.stage,
                    stage_since: e.stage_since,
                })
                .collect();
            write_toml(&dir.join("relationships.toml"), &RelationshipsFile { edges })?;

            // Knowledge edges held by this NPC.
            let knowledge: Vec<KnowledgeEntry> = npc
                .knowledge
                .iter()
                .map(|(fact, k)| KnowledgeEntry {
                    fact: *fact,
                    source: k.source,
                    confidence: k.confidence,
                    distortion: k.distortion,
                    acquired_at: k.acquired_at,
                })
                .collect();
            write_toml(&dir.join("knowledge.toml"), &KnowledgeFile { edges: knowledge })?;
        }
        for location in world.locations.values() {
            write_toml(&self.location_file(location.id, &location.name), location)?;
        }
        Ok(())
    }

    pub fn load_world(&self) -> Result<World> {
        let meta: WorldMeta = read_toml(&self.world_toml_path())?;

        let mut npcs = BTreeMap::new();
        let mut relationships: BTreeMap<(NpcId, NpcId), Edge> = BTreeMap::new();
        for id in &meta.npcs {
            let dir = find_character_dir(&self.characters_dir(), *id)?;
            let mut npc: Npc = read_toml(&dir.join("profile.toml"))?;

            let rel_path = dir.join("relationships.toml");
            if rel_path.exists() {
                let file: RelationshipsFile = read_toml(&rel_path)?;
                for e in file.edges {
                    relationships.insert(
                        (*id, e.object),
                        Edge {
                            affection: e.affection,
                            trust: e.trust,
                            debt: e.debt,
                            grievance: e.grievance,
                            attraction: e.attraction,
                            stage: e.stage,
                            stage_since: e.stage_since,
                        },
                    );
                }
            }

            let know_path = dir.join("knowledge.toml");
            if know_path.exists() {
                let file: KnowledgeFile = read_toml(&know_path)?;
                for e in file.edges {
                    npc.knowledge.insert(
                        e.fact,
                        KnowledgeEdge {
                            source: e.source,
                            confidence: e.confidence,
                            distortion: e.distortion,
                            acquired_at: e.acquired_at,
                        },
                    );
                }
            }

            npcs.insert(*id, npc);
        }

        let mut locations = BTreeMap::new();
        for id in &meta.locations {
            let file = find_location_file(&self.locations_dir(), *id)?;
            let location: Location = read_toml(&file)?;
            locations.insert(*id, location);
        }

        let facts = if self.facts_toml_path().exists() {
            let file: FactsFile = read_toml(&self.facts_toml_path())?;
            file.facts
                .into_iter()
                .map(|e| {
                    (
                        e.id,
                        Fact {
                            kind: e.kind,
                            summary: e.summary,
                        },
                    )
                })
                .collect()
        } else {
            BTreeMap::new()
        };

        Ok(World {
            seed: meta.seed,
            day: meta.day,
            event_index: meta.event_index,
            clock_minutes: meta.clock_minutes,
            next_fact_id: meta.next_fact_id,
            village_name: meta.village_name,
            place_kind: meta.place_kind,
            inhabitant_singular: meta.inhabitant_singular,
            inhabitant_plural: meta.inhabitant_plural,
            start_day: meta.start_day,
            deadline_day: meta.deadline_day,
            npcs,
            locations,
            facts,
            relationships,
        })
    }

    pub fn save_player(&self, player: &PlayerCharacter) -> Result<()> {
        write_toml(&self.player_toml_path(), player)
    }

    pub fn load_player(&self) -> Result<PlayerCharacter> {
        read_toml(&self.player_toml_path())
    }

    pub fn save_case(&self, incident: &Incident) -> Result<()> {
        write_toml(&self.case_toml_path(), incident)
    }

    pub fn load_case(&self) -> Result<Incident> {
        read_toml(&self.case_toml_path())
    }

    fn setting_marker_path(&self) -> PathBuf {
        self.root.join("setting")
    }

    /// Record which content-setting pack this save was created with, so resume
    /// can rehydrate the correct ContentRegistry.
    pub fn save_setting_marker(&self, setting: &str) -> Result<()> {
        let path = self.setting_marker_path();
        fs::write(&path, setting)
            .with_context(|| format!("writing setting marker {}", path.display()))?;
        Ok(())
    }

    /// Read the setting marker, if present. Saves created before this field
    /// existed return `None`; callers should fall back to the CLI default.
    pub fn load_setting_marker(&self) -> Option<String> {
        let path = self.setting_marker_path();
        if !path.is_file() {
            return None;
        }
        fs::read_to_string(&path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

/// Lightweight summary of a save slot for the save-browser UI. Cheap to build
/// (reads only `player.toml`, `world.toml`, and the small `setting` marker);
/// does not deserialize the full NPC / location graph.
#[derive(Clone, Debug)]
pub struct SaveSlot {
    pub path: PathBuf,
    pub slot_name: String,
    pub player_name: String,
    pub village_name: String,
    pub day: u32,
    pub deadline_day: u32,
    /// Setting pack this save was created with, when known. Older saves
    /// written before the marker existed report `None`.
    pub setting_name: Option<String>,
}

/// Scan `parent` for subdirectories that look like saves — i.e. each one has a
/// `player.toml` and a `world.toml` that we can summarize. Skips entries that
/// don't parse, so a half-written slot doesn't block the rest of the list.
/// Sorted by slot name (so `current` comes before `slot-2025-01`, etc.).
pub fn list_save_slots(parent: &Path) -> Result<Vec<SaveSlot>> {
    if !parent.is_dir() {
        return Ok(Vec::new());
    }
    let mut slots: Vec<SaveSlot> = Vec::new();
    for entry in fs::read_dir(parent)
        .with_context(|| format!("reading {}", parent.display()))?
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let player_path = path.join("player.toml");
        let world_path = path.join("world.toml");
        if !player_path.is_file() || !world_path.is_file() {
            continue;
        }
        let player: PlayerCharacter = match read_toml(&player_path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let meta: WorldMeta = match read_toml(&world_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let slot_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let setting_name = {
            let marker = path.join("setting");
            if marker.is_file() {
                fs::read_to_string(&marker)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            } else {
                None
            }
        };
        slots.push(SaveSlot {
            path: path.clone(),
            slot_name,
            player_name: player.name,
            village_name: meta.village_name,
            day: meta.day,
            deadline_day: meta.deadline_day,
            setting_name,
        });
    }
    slots.sort_by(|a, b| a.slot_name.cmp(&b.slot_name));
    Ok(slots)
}

fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 32 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("unnamed");
    }
    out
}

/// TOML integers are `i64`, but random seeds and ever-growing counters can
/// exceed `i64::MAX`. Serialize these `u64` fields as strings; accept either
/// form on deserialize so saves written by older versions still load.
mod u64_str {
    use serde::de::{self, Deserializer, Visitor};
    use serde::Serializer;
    use std::fmt;

    pub fn serialize<S: Serializer>(value: &u64, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<u64, D::Error> {
        de.deserialize_any(U64OrStr)
    }

    struct U64OrStr;
    impl<'de> Visitor<'de> for U64OrStr {
        type Value = u64;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a u64 as integer or string")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
            if v < 0 {
                return Err(E::custom("negative integer for u64 field"));
            }
            Ok(v as u64)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
            v.parse().map_err(E::custom)
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<u64, E> {
            self.visit_str(&v)
        }
    }
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = toml::to_string_pretty(value)
        .with_context(|| format!("serializing toml for {}", path.display()))?;
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let value = toml::from_str(&body)
        .with_context(|| format!("parsing toml at {}", path.display()))?;
    Ok(value)
}

fn clear_dir_contents(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn find_character_dir(parent: &Path, id: NpcId) -> Result<PathBuf> {
    let prefix = format!("{}-", id.0);
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(&prefix) {
                    return Ok(entry.path());
                }
            }
        }
    }
    anyhow::bail!(
        "no character directory for NpcId({}) in {}",
        id.0,
        parent.display()
    )
}

fn find_location_file(parent: &Path, id: LocationId) -> Result<PathBuf> {
    let prefix = format!("{}-", id.0);
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(&prefix) && name.ends_with(".toml") {
                    return Ok(entry.path());
                }
            }
        }
    }
    anyhow::bail!(
        "no location file for LocationId({}) in {}",
        id.0,
        parent.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_world() -> World {
        use crate::engine::EdgeDelta;
        use crate::systems::{
            relationships::apply_relationship_shift, traits::Value, KnowledgeSource,
        };

        let mut world = World::new(42);
        world.day = 3;
        world.event_index = 17;
        world.clock_minutes = 510;
        world.insert_location(Location::new(LocationId(1), "The Tavern"));
        world.insert_location(Location::new(LocationId(2), "Miller's House"));

        let mut aldwin = Npc::new(NpcId(1), "Aldwin the Miller");
        aldwin.location = Some(LocationId(2));
        aldwin.traits.virtues.push("Honest".into());
        aldwin.traits.vices.push("Greedy".into());
        aldwin.values.set(Value::Family, 0.6);
        aldwin.values.set(Value::Reputation, 0.4);
        aldwin.needs.hunger = 0.35;
        aldwin.reputation.honor = 0.4;
        world.insert_npc(aldwin);

        let mut bryn = Npc::new(NpcId(2), "Bryn");
        bryn.location = Some(LocationId(1));
        world.insert_npc(bryn);

        let f1 = world.intern_fact(Fact {
            kind: "theft".into(),
            summary: "the bread is missing".into(),
        });
        let _f2 = world.intern_fact(Fact {
            kind: "rumor".into(),
            summary: "the miller's daughter was seen at the well".into(),
        });

        // A relationship in each direction.
        apply_relationship_shift(
            &mut world,
            NpcId(1),
            NpcId(2),
            &EdgeDelta {
                affection: 15,
                trust: 5,
                debt: 0,
                grievance: 0,
                attraction: 0,
            },
        );
        apply_relationship_shift(
            &mut world,
            NpcId(2),
            NpcId(1),
            &EdgeDelta {
                affection: -10,
                trust: -5,
                debt: 3,
                grievance: 20,
                attraction: 0,
            },
        );

        // One knowledge edge.
        world.npcs.get_mut(&NpcId(2)).unwrap().knowledge.insert(
            f1,
            KnowledgeEdge {
                source: KnowledgeSource::ToldBy { teller: NpcId(1) },
                confidence: 0.7,
                distortion: Distortion { level: 1 },
                acquired_at: Time { day: 3, minute: 480 },
            },
        );

        world
    }

    #[test]
    fn slug_basics() {
        assert_eq!(slug("Aldwin the Miller"), "aldwin-the-miller");
        assert_eq!(slug("  spaces   here  "), "spaces-here");
        assert_eq!(slug("!!!"), "unnamed");
        assert_eq!(slug("Bryn"), "bryn");
        let long = "a".repeat(80);
        assert!(slug(&long).len() <= 32);
    }

    #[test]
    fn world_round_trips() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        let world = sample_world();

        let save_a = SaveDir::open(dir_a.path()).unwrap();
        save_a.save_world(&world).unwrap();

        let loaded = save_a.load_world().unwrap();
        assert_eq!(world, loaded);

        let save_b = SaveDir::open(dir_b.path()).unwrap();
        save_b.save_world(&loaded).unwrap();

        assert_dirs_byte_identical(dir_a.path(), dir_b.path());
    }

    #[test]
    fn world_round_trips_high_bit_seed() {
        // Regression: an OsRng-generated seed with the high bit set used to
        // crash with "out-of-range value for u64 type" because toml integers
        // are i64. WorldMeta now serializes seed/event_index as strings.
        let dir = TempDir::new().unwrap();
        let mut world = sample_world();
        world.seed = u64::MAX - 7;
        world.event_index = u64::MAX - 1;
        let save = SaveDir::open(dir.path()).unwrap();
        save.save_world(&world).unwrap();
        let loaded = save.load_world().unwrap();
        assert_eq!(loaded.seed, u64::MAX - 7);
        assert_eq!(loaded.event_index, u64::MAX - 1);
    }

    #[test]
    fn world_meta_back_compat_integer_seed() {
        // Saves written before the string-encoding change had `seed = 42`.
        // The deserialize_any path must still accept the integer form.
        let dir = TempDir::new().unwrap();
        let save = SaveDir::open(dir.path()).unwrap();
        let world_toml = r#"
seed = 42
day = 0
event_index = 0
clock_minutes = 0
next_fact_id = 1
village_name = ""
start_day = 0
deadline_day = 0
npcs = []
locations = []
"#;
        std::fs::write(save.world_toml_path(), world_toml).unwrap();
        // No facts, no characters, no locations — load_world should succeed.
        let facts_toml = "fact = []\n";
        std::fs::write(save.facts_toml_path(), facts_toml).unwrap();
        let loaded = save.load_world().unwrap();
        assert_eq!(loaded.seed, 42);
        assert_eq!(loaded.event_index, 0);
    }

    #[test]
    fn case_round_trips() {
        use crate::engine::{IncidentKind};
        let dir = TempDir::new().unwrap();
        let save = SaveDir::open(dir.path()).unwrap();
        let incident = Incident {
            kind: IncidentKind::Death {
                victim: NpcId(3),
                perpetrator: NpcId(7),
                method: "poisoned".into(),
            },
            when: Time {
                day: 1825,
                minute: 720,
            },
            location: LocationId(1),
            principals: vec![NpcId(7), NpcId(3)],
            witnesses: vec![NpcId(4)],
            evidence: vec![FactId(1)],
            public_facts: vec![FactId(2)],
            summary: "a death at the mill".into(),
        };
        save.save_case(&incident).unwrap();
        let back = save.load_case().unwrap();
        assert_eq!(incident, back);
    }

    #[test]
    fn player_round_trips() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        let player = PlayerCharacter {
            id: NpcId(99),
            name: "the player".into(),
            vocation: "investigator".into(),
            virtue: "Kind".into(),
            vice: "Vain".into(),
            inclination: "Scholarly".into(),
            background: "Itinerant scholar".into(),
            sex: crate::world::Sex::Female,
        };

        let save_a = SaveDir::open(dir_a.path()).unwrap();
        save_a.save_player(&player).unwrap();
        let loaded = save_a.load_player().unwrap();
        assert_eq!(player, loaded);

        let save_b = SaveDir::open(dir_b.path()).unwrap();
        save_b.save_player(&loaded).unwrap();

        let a = fs::read(save_a.player_toml_path()).unwrap();
        let b = fs::read(save_b.player_toml_path()).unwrap();
        assert_eq!(a, b);
    }

    fn assert_dirs_byte_identical(a: &Path, b: &Path) {
        let a_files = walk_files(a);
        let b_files = walk_files(b);
        assert_eq!(
            a_files.len(),
            b_files.len(),
            "file count differs: {} vs {}",
            a.display(),
            b.display()
        );
        for (rel_path, a_full) in &a_files {
            let b_full = b.join(rel_path);
            assert!(b_full.exists(), "{} missing in second save", rel_path);
            let a_bytes = fs::read(a_full).unwrap();
            let b_bytes = fs::read(&b_full).unwrap();
            assert_eq!(a_bytes, b_bytes, "contents differ for {}", rel_path);
        }
    }

    fn walk_files(root: &Path) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        fn recurse(root: &Path, current: &Path, out: &mut Vec<(String, PathBuf)>) {
            for entry in fs::read_dir(current).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    recurse(root, &path, out);
                } else {
                    let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                    out.push((rel, path));
                }
            }
        }
        recurse(root, root, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}
