use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use super::ids::{FactId, LocationId, NpcId};
use super::sex::Sex;
use crate::engine::Time;
use crate::systems::{
    ActiveGoal, KnowledgeEdge, Mood, MoodImprint, Needs, Reputation, TraitSet, ValueWeights,
};

/// One episodic memory held by an NPC. Unlike `MoodImprint`s (short-term
/// emotional weight on the valence calculation), memorable events are the
/// chronological narrative log that drives `history.md` rendering and surfaces
/// back into `npc_voice` so the LLM can speak from what the character recalls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemorableEvent {
    pub when: Time,
    pub summary: String,
    pub valence: f32,
    #[serde(default)]
    pub related_to: Option<NpcId>,
    #[serde(default)]
    pub fact: Option<FactId>,
}

/// A fact this NPC actively protects from disclosure. `fact` refers into the
/// world fact registry; `category` matches a category declared in the setting
/// pack's `secrets.toml`, which lets engine rules weight reactions when the
/// player probes around the secret's subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    pub fact: FactId,
    pub category: String,
}

/// A promise this NPC has made to someone — typically the player, via the
/// `promise` proposal during dialogue. The simulation tracks these so a future
/// tick can score honored/broken commitments and let trust drift accordingly.
/// `by_day` is the absolute in-world day the promise is due; `None` means
/// open-ended ("someday").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Promise {
    pub promisee: NpcId,
    pub summary: String,
    #[serde(default)]
    pub by_day: Option<u32>,
    pub made_at: Time,
}

/// One block of a per-NPC daily schedule override. `start_hour..end_hour` is
/// the half-open window in 24-hour time; `end_hour <= start_hour` wraps past
/// midnight. `location_kind` is matched against `Location.kind`. `weight`
/// breaks ties when multiple matching locations exist.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduleBlock {
    pub start_hour: u8,
    pub end_hour: u8,
    pub location_kind: String,
    #[serde(default = "default_weight")]
    pub weight: f32,
}

fn default_weight() -> f32 {
    1.0
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    #[serde(default, rename = "block")]
    pub blocks: Vec<ScheduleBlock>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Npc {
    pub id: NpcId,
    pub name: String,
    pub location: Option<LocationId>,
    #[serde(default)]
    pub traits: TraitSet,
    #[serde(default)]
    pub values: ValueWeights,
    #[serde(default)]
    pub needs: Needs,
    #[serde(default)]
    pub mood: Mood,
    #[serde(default)]
    pub mood_imprints: VecDeque<MoodImprint>,
    #[serde(default)]
    pub reputation: Reputation,
    #[serde(default)]
    pub goals: Vec<ActiveGoal>,
    /// Knowledge is persisted out-of-band in `characters/<id>-<slug>/knowledge.toml`
    /// because TOML map keys must be strings; `BTreeMap<FactId, _>` does not
    /// serialize directly. The in-memory shape is keyed by FactId for fast lookup.
    #[serde(skip)]
    pub knowledge: BTreeMap<FactId, KnowledgeEdge>,
    // ---- Phase 5: demographics + structural ties ----
    #[serde(default)]
    pub age: u32,
    #[serde(default)]
    pub sex: Sex,
    #[serde(default)]
    pub attracted_to: Vec<Sex>,
    #[serde(default)]
    pub spouse: Option<NpcId>,
    #[serde(default)]
    pub parents: Vec<NpcId>,
    #[serde(default)]
    pub children: Vec<NpcId>,
    #[serde(default)]
    pub employer: Option<NpcId>,
    #[serde(default)]
    pub occupation: Option<String>,
    #[serde(default)]
    pub dead: bool,
    /// Chronological log of memories this NPC carries. Appended to by the
    /// interpretation layer when the player line lands on them, by worldgen
    /// during the prelude years, and by future engine rules. Bounded only by
    /// pruning at save-time, not by structural cap.
    #[serde(default)]
    pub memorable_events: Vec<MemorableEvent>,
    /// Facts this NPC holds and actively protects. Worldgen seeds these from
    /// the setting pack's `secrets.toml`; the knowledge graph tracks who else
    /// has come to know each one, so a secret stops being one once it spreads.
    #[serde(default)]
    pub secrets: Vec<Secret>,
    /// Optional per-NPC daily schedule override. When `None`, the NPC follows
    /// their occupation's default. When `Some`, the override blocks dominate
    /// — the activity pass routes them according to the first matching block
    /// for the current hour.
    #[serde(default)]
    pub daily_schedule: Option<Schedule>,
    /// Items this NPC carries. Plain strings — the engine treats them as
    /// surface flavor (per the GAME.md rule). Mutated only by
    /// `Event::HandedOver`. The player is NPC(0); their inventory lives here
    /// too. Notebook entries are the primary surface; there is no inventory UI
    /// in v1.
    #[serde(default)]
    pub inventory: Vec<String>,
    /// Commitments this NPC has voiced — typically through the `promise`
    /// dialogue proposal. Mutated only by `Event::PromiseMade`.
    #[serde(default)]
    pub promises: Vec<Promise>,
}

impl Npc {
    pub fn new(id: NpcId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            location: None,
            traits: TraitSet::default(),
            values: ValueWeights::default(),
            needs: Needs::default(),
            mood: Mood::default(),
            mood_imprints: VecDeque::new(),
            reputation: Reputation::default(),
            goals: Vec::new(),
            knowledge: BTreeMap::new(),
            age: 0,
            sex: Sex::default(),
            attracted_to: Vec::new(),
            spouse: None,
            parents: Vec::new(),
            children: Vec::new(),
            employer: None,
            occupation: None,
            dead: false,
            memorable_events: Vec::new(),
            secrets: Vec::new(),
            daily_schedule: None,
            inventory: Vec::new(),
            promises: Vec::new(),
        }
    }
}
