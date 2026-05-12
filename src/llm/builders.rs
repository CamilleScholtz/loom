//! Pure prompt builders for Phase 6 play-loop LLM call sites.
//!
//! Each `*_request` function reads state and returns a `ChatRequest` ready to
//! hand to a `Client`. The system message is the corresponding prompt template
//! (loaded from the content pack at startup) with `{{...}}` placeholders
//! substituted. The user message is a brief "now produce the output" cue so
//! the model has something to anchor on.
//!
//! If a template is missing (a malformed content pack), the builder falls back
//! to a deterministic stub string; this lets the play loop keep running but
//! makes the gap visible in the logs.

use crate::content::ContentRegistry;
use crate::engine::Action;
use crate::engine::action::engine_label;
use crate::engine::{CaseOutcome, Incident};
use crate::llm::interpret::{tool_schema, TOOL_NAME};
use crate::llm::prompts::{lookup, PromptKey};
use crate::llm::{ChatMessage, ChatRequest, Role, ToolDef};
use crate::systems::{Edge, KnowledgeEdge, KnowledgeSource, Needs, Reputation};
use crate::world::{FactId, LocationId, NpcId, World};

/// Replace every `{{name}}` placeholder in `template` with the matching value
/// from `vars`. Generalizes the chain-of-`.replace` pattern already used in
/// `engine::worldgen::render_opening_hook`.
pub fn render_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        let needle = format!("{{{{{}}}}}", key);
        out = out.replace(&needle, value);
    }
    out
}

fn bucket_time(clock_minutes: u32) -> &'static str {
    let h = clock_minutes / 60;
    match h {
        4..=10 => "morning",
        11..=13 => "noon",
        14..=17 => "afternoon",
        18..=21 => "evening",
        _ => "night",
    }
}

/// Coarse 0..=10 mood bucket from a `[-1.0, 1.0]` valence; used as a cache key
/// for scene narration so the same mood produces the same opener.
pub fn mood_bucket(valence: f32) -> u8 {
    let v = valence.clamp(-1.0, 1.0);
    (((v + 1.0) * 5.0).round() as i32).clamp(0, 10) as u8
}

fn mood_label(valence: f32) -> &'static str {
    let bucket = mood_bucket(valence);
    match bucket {
        0..=2 => "bleak",
        3..=4 => "downcast",
        5 => "even",
        6..=7 => "settled",
        _ => "buoyant",
    }
}

fn arousal_label(arousal: f32) -> &'static str {
    let a = arousal.clamp(0.0, 1.0);
    if a < 0.25 {
        "calm"
    } else if a < 0.5 {
        "settled"
    } else if a < 0.75 {
        "restless"
    } else {
        "agitated"
    }
}

fn needs_label(n: &Needs) -> String {
    let mut bits: Vec<&str> = Vec::new();
    if n.hunger > 0.6 {
        bits.push("hungry");
    }
    if n.sleep > 0.6 {
        bits.push("tired");
    }
    if n.safety > 0.6 {
        bits.push("uneasy");
    }
    if n.belonging > 0.6 {
        bits.push("lonely");
    }
    if n.purpose > 0.6 {
        bits.push("aimless");
    }
    if bits.is_empty() {
        "at ease in body".to_string()
    } else {
        bits.join(", ")
    }
}

fn reputation_label(rep: &Reputation) -> String {
    let honor = match rep.honor {
        h if h > 0.4 => "well-regarded",
        h if h > 0.1 => "in good standing",
        h if h < -0.4 => "ill-regarded",
        h if h < -0.1 => "out of favor",
        _ => "unremarked",
    };
    let mut parts: Vec<&str> = vec![honor];
    if rep.menace > 0.5 {
        parts.push("feared");
    } else if rep.menace > 0.2 {
        parts.push("watched warily");
    }
    if rep.competence > 0.5 {
        parts.push("thought capable");
    } else if rep.competence > 0.2 {
        parts.push("thought useful");
    }
    parts.join(", ")
}

fn affection_label(v: i32) -> &'static str {
    match v {
        v if v > 60 => "devoted",
        v if v > 30 => "fond",
        v if v > 10 => "warm",
        v if v > -10 => "neutral",
        v if v > -30 => "cool",
        v if v > -60 => "averse",
        _ => "hostile",
    }
}

fn trust_label(v: i32) -> &'static str {
    match v {
        v if v > 60 => "trusts deeply",
        v if v > 30 => "trusts",
        v if v > 10 => "leans trusting",
        v if v > -10 => "neutral",
        v if v > -30 => "wary",
        v if v > -60 => "distrusts",
        _ => "considers them treacherous",
    }
}

fn grievance_label(v: i32) -> &'static str {
    match v {
        v if v > 60 => "deeply aggrieved",
        v if v > 30 => "holds a grudge",
        v if v > 10 => "holds something against them",
        v if v > 0 => "mild grievance",
        _ => "no grievance",
    }
}

fn attraction_label(v: i32) -> &'static str {
    match v {
        v if v > 60 => "smitten",
        v if v > 30 => "drawn",
        v if v > 10 => "intrigued",
        v if v > -10 => "indifferent",
        v if v > -30 => "put off",
        _ => "averse",
    }
}

/// Render a knowledge edge's source + confidence + distortion as a short tag
/// the model can read at a glance, e.g. `[witnessed, certain]` or
/// `[told, hazy, third-hand]`. The tag is what calibrates the NPC's hedging
/// in the voice prompt.
fn knowledge_tag(edge: &KnowledgeEdge) -> String {
    let source = match edge.source {
        KnowledgeSource::Witnessed => "witnessed",
        KnowledgeSource::ToldBy { .. } => "told",
        KnowledgeSource::Inferred => "inferred",
    };
    let cert = if edge.confidence > 0.8 {
        "certain"
    } else if edge.confidence > 0.6 {
        "fairly sure"
    } else if edge.confidence > 0.4 {
        "uncertain"
    } else {
        "doubtful"
    };
    let distort = match edge.distortion.level {
        0 => "",
        1 => ", second-hand",
        2 => ", third-hand",
        3 => ", hazy",
        _ => ", garbled",
    };
    format!("[{}, {}{}]", source, cert, distort)
}

/// Render reachable locations from `here` as `id: name` lines so the LLM can
/// pick a valid LocationId for a `relocate` proposal. Returns "(nowhere
/// reachable)" when the NPC has no current location or its location has no
/// adjacent edges. The off-map sentinel (id 0) is filtered out — the player
/// can't be invited to step off the world.
fn adjacencies_from(world: &World, here: Option<LocationId>) -> String {
    let Some(here) = here else {
        return "(nowhere reachable)".to_string();
    };
    let Some(loc) = world.location(here) else {
        return "(nowhere reachable)".to_string();
    };
    let mut entries: Vec<(u32, &str)> = loc
        .adjacent
        .iter()
        .filter(|id| id.0 != 0)
        .filter_map(|id| world.location(*id).map(|l| (id.0, l.name.as_str())))
        .collect();
    entries.sort_by_key(|(id, _)| *id);
    if entries.is_empty() {
        return "(nowhere reachable)".to_string();
    }
    entries
        .into_iter()
        .map(|(id, name)| format!("- {}: {}", id, name))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the NPC's family ties by name: spouse, parents, children, employer.
/// Each on its own line. Returns `(no known kin)` when none of these are set,
/// which is the worldgen default for many villagers.
fn family_lines(world: &World, npc: NpcId) -> String {
    let Some(n) = world.npc(npc) else {
        return "(no known kin)".to_string();
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(spouse_id) = n.spouse {
        if let Some(s) = world.npc(spouse_id) {
            out.push(format!("spouse: {}", s.name));
        }
    }
    let parents: Vec<&str> = n
        .parents
        .iter()
        .filter_map(|id| world.npc(*id))
        .map(|x| x.name.as_str())
        .collect();
    if !parents.is_empty() {
        out.push(format!("parents: {}", parents.join(", ")));
    }
    let children: Vec<&str> = n
        .children
        .iter()
        .filter_map(|id| world.npc(*id))
        .map(|x| x.name.as_str())
        .collect();
    if !children.is_empty() {
        out.push(format!("children: {}", children.join(", ")));
    }
    if let Some(emp_id) = n.employer {
        if let Some(e) = world.npc(emp_id) {
            out.push(format!("employer: {}", e.name));
        }
    }
    if out.is_empty() {
        "(no known kin)".to_string()
    } else {
        out.join("\n")
    }
}

/// Up-to-3 most recent player-visible events, oldest-first, as a bulleted list.
/// Surfaces what the NPC could plausibly have heard the player do around town.
fn player_recent_visible_deeds(world: &World) -> String {
    let player = match world.npc(NpcId(0)) {
        Some(p) => p,
        None => return "(nothing of note)".to_string(),
    };
    let mut entries: Vec<(crate::engine::Time, &str)> = player
        .knowledge
        .iter()
        .filter_map(|(fid, k)| {
            world
                .fact(*fid)
                .map(|f| (k.acquired_at, f.summary.as_str()))
        })
        .collect();
    // Most recent first, then truncate, then flip back oldest-first so the
    // reader walks time forward.
    entries.sort_by(|a, b| b.0.day.cmp(&a.0.day).then(b.0.minute.cmp(&a.0.minute)));
    entries.truncate(3);
    entries.reverse();
    if entries.is_empty() {
        return "(nothing of note yet)".to_string();
    }
    entries
        .into_iter()
        .map(|(_, s)| format!("- {}", s))
        .collect::<Vec<_>>()
        .join("\n")
}

fn join_names(world: &World, ids: &[NpcId]) -> String {
    let names: Vec<&str> = ids
        .iter()
        .filter_map(|id| world.npc(*id).map(|n| n.name.as_str()))
        .collect();
    if names.is_empty() {
        return "no one in particular".to_string();
    }
    names.join(", ")
}

fn traits_label(world: &World, npc: NpcId) -> String {
    let Some(n) = world.npc(npc) else {
        return String::new();
    };
    let mut parts: Vec<&str> = Vec::new();
    parts.extend(n.traits.virtues.iter().map(String::as_str));
    parts.extend(n.traits.vices.iter().map(String::as_str));
    parts.extend(n.traits.inclinations.iter().map(String::as_str));
    if parts.is_empty() {
        return "no notable traits".to_string();
    }
    parts.join(", ")
}

fn player_traits(world: &World) -> String {
    traits_label(world, NpcId(0))
}

fn recent_events_summary(world: &World) -> String {
    // Player's most recent witnessed facts (up to 3), most recent first.
    let player = match world.npc(NpcId(0)) {
        Some(p) => p,
        None => return "none".to_string(),
    };
    let mut entries: Vec<(crate::engine::Time, &str)> = player
        .knowledge
        .iter()
        .filter_map(|(fid, k)| {
            world
                .fact(*fid)
                .map(|f| (k.acquired_at, f.summary.as_str()))
        })
        .collect();
    entries.sort_by(|a, b| b.0.day.cmp(&a.0.day).then(b.0.minute.cmp(&a.0.minute)));
    entries.truncate(3);
    if entries.is_empty() {
        return "nothing weighing heavy yet".to_string();
    }
    entries
        .into_iter()
        .map(|(_, s)| s.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Build a `scene_open` prompt for the player arriving at `here`. Variables
/// the template expects: `scene.where`, `scene.time`, `scene.present`,
/// `protagonist.mood`, `protagonist.recent`.
pub fn scene_open_request(
    world: &World,
    content: &ContentRegistry,
    here: LocationId,
    model: &str,
) -> ChatRequest {
    let template = lookup(content, PromptKey::SceneOpen)
        .unwrap_or("You arrive at {{scene.where}} in the {{scene.time}}.");
    let where_name = world
        .location(here)
        .map(|l| l.name.clone())
        .unwrap_or_else(|| "somewhere".to_string());
    let time = bucket_time(world.clock_minutes);
    let present_ids: Vec<NpcId> = world
        .npcs
        .values()
        .filter(|n| !n.dead && n.id.0 != 0 && n.location == Some(here))
        .map(|n| n.id)
        .collect();
    let present = join_names(world, &present_ids);
    let mood = world
        .npc(NpcId(0))
        .map(|p| mood_label(p.mood.valence))
        .unwrap_or("even");
    let recent = recent_events_summary(world);

    let system = render_template(
        template,
        &[
            ("scene.where", &where_name),
            ("scene.time", time),
            ("scene.present", &present),
            ("protagonist.mood", mood),
            ("protagonist.recent", &recent),
        ],
    );
    ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::new(Role::System, system),
            ChatMessage::new(Role::User, "Write the scene paragraph now."),
        ],
        tool: None,
    }
}

/// Stable name of the forced tool the options call site uses. Mirroring the
/// npc-turn pattern: the model is constrained to produce a tightly-shaped JSON
/// object whose `lines` array has exactly one entry per engine action, so the
/// engine never has to guess about ordering or count.
pub const OPTIONS_TOOL_NAME: &str = "phrase_options";

/// JSON-schema for the `phrase_options` tool. `n` is the expected number of
/// option lines; `minItems` and `maxItems` are both set to `n` so the schema
/// enforces "exactly one line per action."
pub fn options_tool_schema(n: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "lines": {
                "type": "array",
                "items": { "type": "string", "maxLength": 200 },
                "minItems": n,
                "maxItems": n,
                "description": "One first-person line per engine action, in the same order as the prompt's <actions> block."
            }
        },
        "required": ["lines"]
    })
}

/// Parse the model's options tool arguments. Returns `Vec<String>` of length
/// `expected` on success. On any failure — missing field, wrong count, malformed
/// JSON — returns an empty vec; callers fall back to bare engine labels.
pub fn parse_options_response(raw: &str, expected: usize) -> Vec<String> {
    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(arr) = parsed.get("lines").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    if out.len() > expected {
        out.truncate(expected);
    }
    if out.len() != expected {
        return Vec::new();
    }
    out
}

/// Build an `options` prompt covering `actions`. The template receives a
/// numbered list of engine labels as `{{actions}}`; the LLM is forced to
/// respond by calling the `phrase_options` tool with an `lines` array sized
/// to match — see [`options_tool_schema`].
pub fn options_request(
    world: &World,
    content: &ContentRegistry,
    here: LocationId,
    actions: &[Action],
    model: &str,
) -> ChatRequest {
    let template = lookup(content, PromptKey::Options).unwrap_or(
        "Available actions:\n{{actions}}\nWrite one inner-monologue line per action.",
    );
    let where_name = world
        .location(here)
        .map(|l| l.name.clone())
        .unwrap_or_else(|| "somewhere".to_string());
    let time = bucket_time(world.clock_minutes);
    let mood = world
        .npc(NpcId(0))
        .map(|p| mood_label(p.mood.valence))
        .unwrap_or("even");
    let labels: Vec<String> = actions.iter().map(|a| engine_label(world, a)).collect();
    // Numbered list keeps the ordering explicit in the prompt, mirroring the
    // schema's `minItems == maxItems == labels.len()`.
    let actions_str = labels
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{}. {}", i + 1, l))
        .collect::<Vec<_>>()
        .join("\n");
    let player_name = world
        .npc(NpcId(0))
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let traits = player_traits(world);

    let system = render_template(
        template,
        &[
            ("protagonist.name", &player_name),
            ("protagonist.background", ""),
            ("protagonist.traits", &traits),
            ("protagonist.mood", mood),
            ("scene.where", &where_name),
            ("scene.time", time),
            ("actions", &actions_str),
        ],
    );
    ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::new(Role::System, system),
            ChatMessage::new(
                Role::User,
                "Now phrase the options. Call the tool with one entry per action, in order.",
            ),
        ],
        tool: Some(ToolDef {
            name: OPTIONS_TOOL_NAME.to_string(),
            description: "Phrase each engine action in the protagonist's voice.".to_string(),
            parameters_schema: options_tool_schema(labels.len()),
            force: true,
        }),
    }
}

/// Marker phrase appended to the system message of a combined npc-turn
/// request. The FakeClient watches for this so dry-run mode can return a
/// schema-conforming JSON stub instead of a free-form line; it has no effect
/// on real models, for whom the surrounding instructions are what matter.
pub const NPC_TURN_SCHEMA_MARKER: &str = "[npc_turn_schema_v1]";

/// Build a combined npc-turn request: same context as `npc_voice_request`,
/// but the model is asked to return a single JSON object containing both a
/// structured classification of the player's line and the NPC's reply. The
/// classifier output is bounded by enum variants the engine knows about; the
/// reply field carries the prose shown to the player.
pub fn npc_turn_request(
    world: &World,
    content: &ContentRegistry,
    npc: NpcId,
    player_line: &str,
    history: &[(bool, String)],
    model: &str,
) -> ChatRequest {
    let base = lookup(content, PromptKey::NpcVoice)
        .unwrap_or("Speak as {{npc.name}}. One short line.");
    let n = world.npc(npc);
    let inhab = if world.inhabitant_singular.is_empty() {
        "villager"
    } else {
        world.inhabitant_singular.as_str()
    };
    let name = n
        .map(|x| x.name.clone())
        .unwrap_or_else(|| format!("a {}", inhab));
    let occ = n
        .and_then(|x| x.occupation.clone())
        .unwrap_or_else(|| inhab.to_string());
    let trts = traits_label(world, npc);
    // Combined mood label: valence bucket plus arousal axis, e.g. "downcast,
    // restless". The agitation axis is its own thing — a calm-bleak character
    // speaks differently from a frantic-bleak one.
    let mood_text = match n {
        Some(x) => format!(
            "{}, {}",
            mood_label(x.mood.valence),
            arousal_label(x.mood.arousal)
        ),
        None => "even, calm".to_string(),
    };
    let needs_text = n
        .map(|x| needs_label(&x.needs))
        .unwrap_or_else(|| "at ease in body".to_string());
    let reputation_text = n
        .map(|x| reputation_label(&x.reputation))
        .unwrap_or_else(|| "unremarked".to_string());
    let family_text = family_lines(world, npc);
    let know = relevant_knowledge(world, npc, player_line);
    let rel = render_edge(world.relationship(npc, NpcId(0)));

    let memories = memories_of_you(world, npc);
    let romance = romance_line(world, npc);
    let secrets = secrets_kept(world, npc);
    let goals = goals_active(world, npc);

    // Scene context: the NPC's own current location and the time of day.
    // (The player is in the same place — that's how this dialogue is happening.)
    let scene_where = n
        .and_then(|x| x.location)
        .and_then(|lid| world.location(lid))
        .map(|l| l.name.clone())
        .unwrap_or_else(|| "elsewhere".to_string());
    let scene_time = bucket_time(world.clock_minutes);
    // Reachable locations from where the NPC stands — surfaces the legal
    // `relocate` destinations so the model can pick a real LocationId.
    let scene_adjacencies = adjacencies_from(world, n.and_then(|x| x.location));

    // Player profile — the NPC reacts to a person, not a faceless interlocutor.
    let player = world.npc(NpcId(0));
    let player_name = player
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "the stranger".into());
    let player_trts = traits_label(world, NpcId(0));
    let player_rep = player
        .map(|p| reputation_label(&p.reputation))
        .unwrap_or_else(|| "unremarked".to_string());
    let player_recent = player_recent_visible_deeds(world);

    let roster = npc_roster(world);

    let system = render_template(
        base,
        &[
            ("npc.name", &name),
            ("npc.occupation", &occ),
            ("npc.traits", &trts),
            ("npc.mood", &mood_text),
            ("npc.needs", &needs_text),
            ("npc.reputation", &reputation_text),
            ("npc.family", &family_text),
            ("npc.knowledge", &know),
            ("npc.relationship", &rel),
            ("npc.memories_of_you", &memories),
            ("npc.romance", &romance),
            ("npc.secrets_kept", &secrets),
            ("npc.goals_active", &goals),
            ("scene.where", &scene_where),
            ("scene.time", scene_time),
            ("scene.adjacencies", &scene_adjacencies),
            ("player.name", &player_name),
            ("player.traits", &player_trts),
            ("player.reputation", &player_rep),
            ("player.recent_visible_deeds", &player_recent),
            ("roster", &roster),
            ("schema_marker", NPC_TURN_SCHEMA_MARKER),
        ],
    );
    let mut messages = vec![ChatMessage::new(Role::System, system)];
    for (is_player, line) in history {
        let role = if *is_player {
            Role::User
        } else {
            Role::Assistant
        };
        messages.push(ChatMessage::new(role, line.clone()));
    }
    messages.push(ChatMessage::new(Role::User, player_line.to_string()));
    ChatRequest {
        model: model.to_string(),
        messages,
        tool: Some(ToolDef {
            name: TOOL_NAME.to_string(),
            description: "Voice the NPC's reply and classify the player's line.".to_string(),
            parameters_schema: tool_schema(),
            force: true,
        }),
    }
}

/// Pick up to ~7 facts the NPC holds, scored by:
///   * confidence floor (so high-confidence "core beliefs" surface even if the
///     player's question doesn't lexically overlap),
///   * token overlap with the player line (4+ char tokens, substring match),
///   * roster-name mention boost: if the player names a villager and the fact
///     summary mentions that villager, surface it.
///
/// On top of the scoring, the top-3 by confidence are guaranteed included as
/// "core beliefs" — the NPC will not look brain-dead when the player asks
/// something well-phrased that doesn't share words with any fact summary.
/// Each line is prefixed with `[source, certainty]` from [`knowledge_tag`] so
/// the LLM hedges in proportion to how the NPC came to hold the fact.
fn relevant_knowledge(world: &World, npc: NpcId, player_line: &str) -> String {
    use std::collections::BTreeSet;

    let Some(n) = world.npc(npc) else {
        return "(no fact recorded)".to_string();
    };
    let line_lc = player_line.to_lowercase();
    let tokens: Vec<String> = line_lc
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 4)
        .map(|t| t.to_string())
        .collect();

    // Roster names (per-word, length-3+) the player's line mentions. So
    // "where was Aldwin last night?" lights up every fact whose summary
    // mentions Aldwin, even if no other word overlaps.
    let mut mentioned: Vec<String> = Vec::new();
    for x in world.npcs.values() {
        for word in x.name.split_whitespace() {
            if word.len() >= 3 {
                let wl = word.to_lowercase();
                if line_lc.contains(&wl) {
                    mentioned.push(wl);
                }
            }
        }
    }
    mentioned.sort();
    mentioned.dedup();

    let mut scored: Vec<(f32, FactId, &KnowledgeEdge, &str)> = Vec::new();
    for (fid, edge) in &n.knowledge {
        let Some(fact) = world.fact(*fid) else {
            continue;
        };
        let summary_lc = fact.summary.to_lowercase();
        let overlap = tokens.iter().filter(|t| summary_lc.contains(t.as_str())).count();
        let mention_hit = mentioned.iter().any(|m| summary_lc.contains(m.as_str()));
        let score = edge.confidence
            + overlap as f32 * 0.5
            + if mention_hit { 1.0 } else { 0.0 };
        scored.push((score, *fid, edge, fact.summary.as_str()));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Top-3 high-confidence facts, guaranteed-included floor.
    let mut by_conf: Vec<(f32, FactId, &KnowledgeEdge, &str)> = n
        .knowledge
        .iter()
        .filter_map(|(fid, k)| {
            world
                .fact(*fid)
                .map(|f| (k.confidence, *fid, k, f.summary.as_str()))
        })
        .collect();
    by_conf.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    by_conf.truncate(3);

    let mut chosen: Vec<(&KnowledgeEdge, &str)> = Vec::new();
    let mut seen: BTreeSet<FactId> = BTreeSet::new();
    for (_, fid, edge, summary) in scored.iter().chain(by_conf.iter()) {
        if seen.insert(*fid) {
            chosen.push((*edge, *summary));
            if chosen.len() >= 7 {
                break;
            }
        }
    }
    if chosen.is_empty() {
        return "(no fact recorded)".to_string();
    }
    chosen
        .into_iter()
        .map(|(edge, summary)| format!("- {} {}", knowledge_tag(edge), summary))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_edge(e: Edge) -> String {
    format!(
        "{} (affection {:+}); {} (trust {:+}); {} (grievance {})",
        affection_label(e.affection),
        e.affection,
        trust_label(e.trust),
        e.trust,
        grievance_label(e.grievance),
        e.grievance,
    )
}

/// Compact "id: name" list of every NPC in the world, used as the closed
/// pick-list for the model's `references` / `asserts.about` fields. The
/// player (id 0) is included so the LLM can reference the protagonist by id
/// when needed. Dead NPCs are kept — gossip and accusations often concern
/// the recently dead.
fn npc_roster(world: &World) -> String {
    let mut entries: Vec<(u32, &str)> = world
        .npcs
        .values()
        .map(|n| (n.id.0, n.name.as_str()))
        .collect();
    entries.sort_by_key(|(id, _)| *id);
    entries
        .into_iter()
        .map(|(id, name)| format!("{}: {}", id, name))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the last few memorable events the NPC carries about the player,
/// oldest-first. Each line is `day N (good|cold|grim): summary` so the LLM can
/// read the valence at a glance without us writing a paragraph.
fn memories_of_you(world: &World, npc: NpcId) -> String {
    let Some(n) = world.npc(npc) else {
        return "(no memory)".to_string();
    };
    let player = NpcId(0);
    let mut entries: Vec<&crate::world::MemorableEvent> = n
        .memorable_events
        .iter()
        .filter(|m| m.related_to == Some(player))
        .collect();
    if entries.is_empty() {
        return "(no memory)".to_string();
    }
    // Oldest-first; cap at 5 to keep the prompt bounded.
    entries.sort_by(|a, b| {
        a.when
            .day
            .cmp(&b.when.day)
            .then(a.when.minute.cmp(&b.when.minute))
    });
    let cut = entries.len().saturating_sub(5);
    entries
        .into_iter()
        .skip(cut)
        .map(|m| {
            let tag = if m.valence > 0.3 {
                "good"
            } else if m.valence < -0.3 {
                "grim"
            } else {
                "cold"
            };
            format!("- day {} ({}): {}", m.when.day, tag, m.summary)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the romance picture: attraction/affection/trust as a compact line,
/// plus a derived compatibility word from `npc.attracted_to` vs player sex.
/// "Indifferent" rather than "incompatible" when the NPC's preferences are
/// simply unspecified, so the LLM doesn't overplay rejection.
fn romance_line(world: &World, npc: NpcId) -> String {
    let edge = world.relationship(npc, NpcId(0));
    let compat = match world.npc(npc) {
        Some(n) if n.attracted_to.is_empty() => "indifferent",
        Some(n) => match world.npc(NpcId(0)).map(|p| p.sex) {
            Some(sex) if n.attracted_to.contains(&sex) => "compatible",
            Some(_) => "incompatible",
            None => "indifferent",
        },
        None => "indifferent",
    };
    format!(
        "{} (attraction {:+}); {} (affection {:+}); {} (trust {:+}); {}; stage: {}",
        attraction_label(edge.attraction),
        edge.attraction,
        affection_label(edge.affection),
        edge.affection,
        trust_label(edge.trust),
        edge.trust,
        compat,
        edge.stage.label(),
    )
}

/// Comma-joined list of secret categories this NPC holds. Categories only —
/// summaries are deliberately withheld so the LLM uses the field to deflect,
/// not to reveal. Engine logic, not the LLM, decides if a secret leaks.
fn secrets_kept(world: &World, npc: NpcId) -> String {
    let Some(n) = world.npc(npc) else {
        return "(none)".to_string();
    };
    if n.secrets.is_empty() {
        return "(none)".to_string();
    }
    let mut cats: Vec<&str> = n.secrets.iter().map(|s| s.category.as_str()).collect();
    cats.sort();
    cats.dedup();
    cats.join(", ")
}

/// Comma-joined kinds of currently-active goals so the LLM can hint at the
/// NPC's agenda without being told the full goal payload. Returns "(none)" if
/// the NPC has no active goal.
fn goals_active(world: &World, npc: NpcId) -> String {
    let Some(n) = world.npc(npc) else {
        return "(none)".to_string();
    };
    if n.goals.is_empty() {
        return "(none)".to_string();
    }
    let mut kinds: Vec<&str> = n.goals.iter().map(|g| g.kind.as_str()).collect();
    kinds.sort();
    kinds.dedup();
    kinds.join(", ")
}

fn place_kind(world: &World) -> &str {
    if world.place_kind.is_empty() {
        "village"
    } else {
        world.place_kind.as_str()
    }
}

fn npc_name(world: &World, id: NpcId) -> String {
    let inhab = if world.inhabitant_singular.is_empty() {
        "villager"
    } else {
        world.inhabitant_singular.as_str()
    };
    world
        .npc(id)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| format!("an unknown {}", inhab))
}

fn outcome_blurb(world: &World, outcome: &CaseOutcome) -> String {
    match outcome {
        CaseOutcome::Solved { target } => {
            format!("the player named {} as the culprit, correctly", npc_name(world, *target))
        }
        CaseOutcome::MisAccused { target, actual } => format!(
            "the player named {}, but the simulation records {} as the culprit",
            npc_name(world, *target),
            npc_name(world, *actual),
        ),
        CaseOutcome::Unsolved => "the deadline arrived without an accusation".into(),
        CaseOutcome::LeftTown => format!(
            "the player walked away from the {} before resolving the case",
            place_kind(world),
        ),
        CaseOutcome::Died => "the player died before the case could be resolved".into(),
    }
}

fn accusation_blurb(world: &World, outcome: &CaseOutcome) -> String {
    match outcome {
        CaseOutcome::Solved { target } | CaseOutcome::MisAccused { target, .. } => {
            npc_name(world, *target)
        }
        _ => "no one".into(),
    }
}

fn player_relationship_lines(world: &World) -> String {
    let mut out: Vec<(String, Edge)> = Vec::new();
    for (other, edge) in world.relationships_from(NpcId(0)) {
        if other == NpcId(0) {
            continue;
        }
        let name = npc_name(world, other);
        out.push((name, *edge));
    }
    if out.is_empty() {
        return "none worth noting".into();
    }
    // Rank by |affection| + |grievance| so the most charged edges surface first.
    out.sort_by(|a, b| {
        let ka = a.1.affection.abs() + a.1.grievance.abs() + a.1.trust.abs();
        let kb = b.1.affection.abs() + b.1.grievance.abs() + b.1.trust.abs();
        kb.cmp(&ka)
    });
    out.truncate(8);
    out.into_iter()
        .map(|(name, e)| format!("- {}: {}", name, render_edge(e)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn player_visible_events(world: &World) -> String {
    let player = match world.npc(NpcId(0)) {
        Some(p) => p,
        None => return "none".into(),
    };
    let mut entries: Vec<(crate::engine::Time, &str)> = player
        .knowledge
        .iter()
        .filter_map(|(fid, k)| {
            world
                .fact(*fid)
                .map(|f| (k.acquired_at, f.summary.as_str()))
        })
        .collect();
    entries.sort_by(|a, b| a.0.day.cmp(&b.0.day).then(a.0.minute.cmp(&b.0.minute)));
    entries.truncate(12);
    if entries.is_empty() {
        return "none recorded".into();
    }
    entries
        .into_iter()
        .map(|(t, s)| format!("- day {}: {}", t.day, s))
        .collect::<Vec<_>>()
        .join("\n")
}

fn private_events(world: &World) -> String {
    let player = world.npc(NpcId(0));
    let known: std::collections::BTreeSet<crate::world::FactId> = player
        .map(|p| p.knowledge.keys().copied().collect())
        .unwrap_or_default();
    let mut entries: Vec<&str> = world
        .facts
        .iter()
        .filter(|(id, _)| !known.contains(id))
        .map(|(_, f)| f.summary.as_str())
        .collect();
    entries.truncate(8);
    if entries.is_empty() {
        return "no hidden facts left untouched".into();
    }
    entries
        .into_iter()
        .map(|s| format!("- {}", s))
        .collect::<Vec<_>>()
        .join("\n")
}

fn final_npcs(world: &World) -> String {
    let mut out: Vec<String> = Vec::new();
    for npc in world.npcs.values() {
        if npc.id == NpcId(0) {
            continue;
        }
        let here = npc
            .location
            .and_then(|l| world.location(l).map(|loc| loc.name.clone()))
            .unwrap_or_else(|| "elsewhere".into());
        let occ = npc
            .occupation
            .clone()
            .map(|o| format!(", {}", o))
            .unwrap_or_default();
        let state = if npc.dead { " (dead)" } else { "" };
        out.push(format!("- {} ({}{}{}, at {})", npc.name, npc.age, occ, state, here));
    }
    if out.is_empty() {
        return "no NPCs survive".into();
    }
    // Keep the prompt bounded; ~30 lines is plenty.
    if out.len() > 30 {
        out.truncate(30);
    }
    out.join("\n")
}

/// Build an `epilogue` prompt that closes a playthrough. Variables:
/// `outcome`, `accusation`, `culprit`, `relationships`, `events.player_visible`,
/// `events.private`, `npcs.final`.
pub fn epilogue_request(
    world: &World,
    content: &ContentRegistry,
    incident: &Incident,
    outcome: &CaseOutcome,
    model: &str,
) -> ChatRequest {
    let template = lookup(content, PromptKey::Epilogue).unwrap_or(
        "The case has ended. Outcome: {{outcome}}. Player accused: {{accusation}}. Actual: {{culprit}}.",
    );
    let outcome_text = outcome_blurb(world, outcome);
    let accusation = accusation_blurb(world, outcome);
    let culprit = npc_name(world, incident.primary_culprit());
    let relationships = player_relationship_lines(world);
    let player_visible = player_visible_events(world);
    let private = private_events(world);
    let npcs_final = final_npcs(world);

    let system = render_template(
        template,
        &[
            ("outcome", &outcome_text),
            ("accusation", &accusation),
            ("culprit", &culprit),
            ("relationships", &relationships),
            ("events.player_visible", &player_visible),
            ("events.private", &private),
            ("npcs.final", &npcs_final),
        ],
    );
    ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::new(Role::System, system),
            ChatMessage::new(Role::User, "Write the epilogue now."),
        ],
        tool: None,
    }
}

/// Deterministic, plain-prose fallback for the epilogue when the LLM call
/// errors or returns empty. Grounded entirely in simulation state.
pub fn fallback_epilogue(world: &World, incident: &Incident, outcome: &CaseOutcome) -> String {
    let mut out = String::new();
    let header = match outcome {
        CaseOutcome::Solved { target } => format!(
            "The case closes. {} stood accused, and the records bear it out.\n\n",
            npc_name(world, *target)
        ),
        CaseOutcome::MisAccused { target, actual } => format!(
            "The case closes. {} stood accused, but the truth lay with {}.\n\n",
            npc_name(world, *target),
            npc_name(world, *actual),
        ),
        CaseOutcome::Unsolved => {
            "The deadline came and went. Nothing was named. The matter sank into rumor.\n\n".into()
        }
        CaseOutcome::LeftTown => format!(
            "You walked away. The {} kept its secrets.\n\n",
            place_kind(world),
        ),
        CaseOutcome::Died => "You did not live to close the case.\n\n".into(),
    };
    out.push_str(&header);
    out.push_str(&format!("The case: {}\n\n", incident.summary));
    out.push_str("After:\n");
    for npc in world.npcs.values() {
        if npc.id == NpcId(0) {
            continue;
        }
        let here = npc
            .location
            .and_then(|l| world.location(l).map(|loc| loc.name.clone()))
            .unwrap_or_else(|| "elsewhere".into());
        let state = if npc.dead { "lies dead" } else { "still walks" };
        out.push_str(&format!("- {}: {}, at {}.\n", npc.name, state, here));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::ContentRegistry;
    use crate::world::{Location, Npc, NpcId};
    use std::path::PathBuf;

    fn registry() -> ContentRegistry {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("content");
        ContentRegistry::load(&root, "medieval", "investigator").unwrap()
    }

    fn seeded_world() -> World {
        let mut w = World::new(0);
        w.insert_location(Location::new(LocationId(1), "the tavern"));
        let mut player = Npc::new(NpcId(0), "Wren");
        player.location = Some(LocationId(1));
        player.traits.virtues.push("Curious".into());
        w.insert_npc(player);
        let mut npc = Npc::new(NpcId(1), "Aldwin");
        npc.location = Some(LocationId(1));
        npc.occupation = Some("carpenter".into());
        npc.traits.vices.push("Suspicious".into());
        w.insert_npc(npc);
        w
    }

    #[test]
    fn render_template_substitutes_doublebrace_keys() {
        let out = render_template(
            "hello {{name}}, it is {{time}}",
            &[("name", "Wren"), ("time", "noon")],
        );
        assert_eq!(out, "hello Wren, it is noon");
    }

    #[test]
    fn mood_bucket_clamps_extremes() {
        assert_eq!(mood_bucket(-2.0), 0);
        assert_eq!(mood_bucket(0.0), 5);
        assert_eq!(mood_bucket(2.0), 10);
    }

    const TEST_MODEL: &str = "test/model";

    #[test]
    fn scene_open_substitutes_location_and_presence() {
        let w = seeded_world();
        let reg = registry();
        let req = scene_open_request(&w, &reg, LocationId(1), TEST_MODEL);
        let sys = &req.messages[0].content;
        assert_eq!(req.model, TEST_MODEL);
        assert!(sys.contains("the tavern"));
        assert!(sys.contains("Aldwin"));
        // No raw template placeholders should leak through.
        assert!(!sys.contains("{{scene.where}}"));
        assert!(!sys.contains("{{scene.present}}"));
    }

    #[test]
    fn options_substitutes_action_labels() {
        let w = seeded_world();
        let reg = registry();
        let acts = vec![Action::Observe, Action::Wait];
        let req = options_request(&w, &reg, LocationId(1), &acts, TEST_MODEL);
        let sys = &req.messages[0].content;
        assert_eq!(req.model, TEST_MODEL);
        assert!(sys.contains("observe the room"));
        assert!(sys.contains("wait here a while"));
    }

    #[test]
    fn npc_turn_includes_relationship_player_line_and_forced_tool() {
        let w = seeded_world();
        let reg = registry();
        let req = npc_turn_request(
            &w,
            &reg,
            NpcId(1),
            "Did you see anyone at the well?",
            &[],
            TEST_MODEL,
        );
        let sys = &req.messages[0].content;
        let user = &req.messages[1].content;
        assert_eq!(req.model, TEST_MODEL);
        assert!(sys.contains("Aldwin"));
        assert!(sys.contains("affection"));
        assert!(sys.contains(NPC_TURN_SCHEMA_MARKER));
        // The schema appendix is gone — structure is enforced by the
        // attached tool. Verify the tool is present and forced.
        let tool = req.tool.as_ref().expect("npc_turn request carries a tool");
        assert_eq!(tool.name, crate::llm::interpret::TOOL_NAME);
        assert!(tool.force);
        assert!(user.contains("the well"));
    }

    #[test]
    fn npc_turn_threads_history_as_alternating_messages() {
        let w = seeded_world();
        let reg = registry();
        let history = vec![
            (true, "Where were you last night?".to_string()),
            (false, "At the mill, like always.".to_string()),
        ];
        let req = npc_turn_request(
            &w,
            &reg,
            NpcId(1),
            "And before that?",
            &history,
            TEST_MODEL,
        );
        assert_eq!(req.messages.len(), 4);
        assert_eq!(req.messages[0].role, Role::System);
        assert_eq!(req.messages[1].role, Role::User);
        assert!(req.messages[1].content.contains("last night"));
        assert_eq!(req.messages[2].role, Role::Assistant);
        assert!(req.messages[2].content.contains("the mill"));
        assert_eq!(req.messages[3].role, Role::User);
        assert!(req.messages[3].content.contains("before that"));
    }

    #[test]
    fn relevant_knowledge_returns_placeholder_when_empty() {
        let w = seeded_world();
        assert_eq!(relevant_knowledge(&w, NpcId(1), "anything"), "(no fact recorded)");
    }

    #[test]
    fn memories_of_you_is_placeholder_when_empty() {
        let w = seeded_world();
        assert_eq!(memories_of_you(&w, NpcId(1)), "(no memory)");
    }

    #[test]
    fn memories_of_you_renders_oldest_first_and_tags_valence() {
        use crate::engine::Time;
        use crate::world::MemorableEvent;
        let mut w = seeded_world();
        if let Some(n) = w.npcs.get_mut(&NpcId(1)) {
            n.memorable_events.push(MemorableEvent {
                when: Time { day: 1, minute: 0 },
                summary: "the player insulted me".into(),
                valence: -0.8,
                related_to: Some(NpcId(0)),
                fact: None,
            });
            n.memorable_events.push(MemorableEvent {
                when: Time { day: 2, minute: 0 },
                summary: "the player was kind to me".into(),
                valence: 0.5,
                related_to: Some(NpcId(0)),
                fact: None,
            });
            // Unrelated to player — should be filtered out.
            n.memorable_events.push(MemorableEvent {
                when: Time { day: 2, minute: 60 },
                summary: "Edith dropped a candle".into(),
                valence: 0.0,
                related_to: Some(NpcId(2)),
                fact: None,
            });
        }
        let out = memories_of_you(&w, NpcId(1));
        let day1 = out.find("day 1").unwrap();
        let day2 = out.find("day 2").unwrap();
        assert!(day1 < day2, "oldest first");
        assert!(out.contains("grim"));
        assert!(out.contains("good"));
        assert!(!out.contains("candle"), "non-player memories filtered out");
    }

    #[test]
    fn memories_of_you_caps_at_five() {
        use crate::engine::Time;
        use crate::world::MemorableEvent;
        let mut w = seeded_world();
        if let Some(n) = w.npcs.get_mut(&NpcId(1)) {
            for d in 1..=8u32 {
                n.memorable_events.push(MemorableEvent {
                    when: Time { day: d, minute: 0 },
                    summary: format!("event {}", d),
                    valence: 0.0,
                    related_to: Some(NpcId(0)),
                    fact: None,
                });
            }
        }
        let out = memories_of_you(&w, NpcId(1));
        assert_eq!(out.lines().count(), 5);
        assert!(!out.contains("event 1"), "oldest got trimmed");
        assert!(out.contains("event 8"), "newest kept");
    }

    #[test]
    fn romance_line_reports_compat_for_known_preference() {
        use crate::world::Sex;
        let mut w = seeded_world();
        // Player defaults Sex::default(); ensure compatible/incompatible cases.
        if let Some(p) = w.npcs.get_mut(&NpcId(0)) {
            p.sex = Sex::Male;
        }
        if let Some(n) = w.npcs.get_mut(&NpcId(1)) {
            n.attracted_to = vec![Sex::Male];
        }
        let out = romance_line(&w, NpcId(1));
        assert!(out.contains("compatible"));
        assert!(out.contains("attraction"));
    }

    #[test]
    fn romance_line_says_indifferent_when_preferences_unset() {
        let w = seeded_world();
        let out = romance_line(&w, NpcId(1));
        assert!(out.contains("indifferent"));
    }

    #[test]
    fn secrets_kept_lists_categories_not_summaries() {
        use crate::engine::Fact;
        use crate::world::Secret;
        let mut w = seeded_world();
        let f1 = w.intern_fact(Fact {
            kind: "adultery".into(),
            summary: "Aldwin meets a stranger past midnight at the chapel".into(),
        });
        let f2 = w.intern_fact(Fact {
            kind: "debt".into(),
            summary: "he owes the miller a year of wages".into(),
        });
        if let Some(n) = w.npcs.get_mut(&NpcId(1)) {
            n.secrets.push(Secret {
                fact: f1,
                category: "adultery".into(),
            });
            n.secrets.push(Secret {
                fact: f2,
                category: "debt".into(),
            });
        }
        let out = secrets_kept(&w, NpcId(1));
        assert!(out.contains("adultery"));
        assert!(out.contains("debt"));
        // Summaries must NOT leak into the prompt.
        assert!(!out.contains("chapel"));
        assert!(!out.contains("miller"));
    }

    #[test]
    fn secrets_kept_placeholder_when_empty() {
        let w = seeded_world();
        assert_eq!(secrets_kept(&w, NpcId(1)), "(none)");
    }

    #[test]
    fn goals_active_lists_kinds() {
        use crate::systems::ActiveGoal;
        let mut w = seeded_world();
        if let Some(n) = w.npcs.get_mut(&NpcId(1)) {
            n.goals.push(ActiveGoal {
                kind: "avenge".into(),
                summary: "even the score".into(),
                formed_at: crate::engine::Time { day: 0, minute: 0 },
            });
            n.goals.push(ActiveGoal {
                kind: "court".into(),
                summary: "win Edith".into(),
                formed_at: crate::engine::Time { day: 0, minute: 0 },
            });
        }
        let out = goals_active(&w, NpcId(1));
        assert!(out.contains("avenge"));
        assert!(out.contains("court"));
    }

    #[test]
    fn goals_active_placeholder_when_none() {
        let w = seeded_world();
        assert_eq!(goals_active(&w, NpcId(1)), "(none)");
    }

    #[test]
    fn npc_turn_surfaces_memories_secrets_romance_and_goals() {
        use crate::engine::{Fact, Time};
        use crate::systems::ActiveGoal;
        use crate::world::{MemorableEvent, Secret, Sex};
        let mut w = seeded_world();
        if let Some(p) = w.npcs.get_mut(&NpcId(0)) {
            p.sex = Sex::Male;
        }
        let f = w.intern_fact(Fact {
            kind: "adultery".into(),
            summary: "redacted".into(),
        });
        if let Some(n) = w.npcs.get_mut(&NpcId(1)) {
            n.attracted_to = vec![Sex::Male];
            n.memorable_events.push(MemorableEvent {
                when: Time { day: 1, minute: 0 },
                summary: "the player insulted me".into(),
                valence: -0.8,
                related_to: Some(NpcId(0)),
                fact: None,
            });
            n.secrets.push(Secret {
                fact: f,
                category: "adultery".into(),
            });
            n.goals.push(ActiveGoal {
                kind: "avenge".into(),
                summary: "even the score".into(),
                formed_at: Time { day: 0, minute: 0 },
            });
        }
        let reg = registry();
        let req = npc_turn_request(&w, &reg, NpcId(1), "anything", &[], TEST_MODEL);
        let sys = &req.messages[0].content;
        assert!(sys.contains("the player insulted me"));
        assert!(sys.contains("adultery"));
        assert!(sys.contains("avenge"));
        assert!(sys.contains("compatible"));
        // Secret summary must not appear in the prompt, only the category.
        assert!(!sys.contains("redacted"));
    }

    fn sample_incident() -> Incident {
        use crate::engine::{Fact, IncidentKind, Time};
        use crate::world::FactId;
        Incident {
            kind: IncidentKind::Death {
                victim: NpcId(2),
                perpetrator: NpcId(1),
                method: "poison".into(),
            },
            when: Time { day: 10, minute: 0 },
            location: LocationId(1),
            principals: vec![NpcId(1), NpcId(2)],
            witnesses: vec![NpcId(3)],
            evidence: vec![FactId(1)],
            public_facts: vec![FactId(2)],
            summary: "a death at the tavern".into(),
        }
    }

    #[test]
    fn epilogue_substitutes_outcome_and_culprit_for_solved_case() {
        let w = seeded_world();
        let reg = registry();
        let incident = sample_incident();
        let outcome = CaseOutcome::Solved { target: NpcId(1) };
        let req = epilogue_request(&w, &reg, &incident, &outcome, TEST_MODEL);
        let sys = &req.messages[0].content;
        assert_eq!(req.model, TEST_MODEL);
        assert!(sys.contains("Aldwin"), "culprit name should appear");
        assert!(sys.contains("correctly"), "solved outcome should be narrated");
        assert!(!sys.contains("{{outcome}}"));
        assert!(!sys.contains("{{culprit}}"));
    }

    #[test]
    fn epilogue_substitutes_actual_culprit_when_misaccused() {
        let w = seeded_world();
        let reg = registry();
        let incident = sample_incident();
        let outcome = CaseOutcome::MisAccused {
            target: NpcId(0),
            actual: NpcId(1),
        };
        let req = epilogue_request(&w, &reg, &incident, &outcome, TEST_MODEL);
        let sys = &req.messages[0].content;
        assert!(sys.contains("but the simulation records Aldwin"));
    }

    #[test]
    fn fallback_epilogue_reflects_outcome_and_grounds_in_world() {
        let w = seeded_world();
        let incident = sample_incident();
        let body = fallback_epilogue(&w, &incident, &CaseOutcome::Unsolved);
        assert!(body.contains("deadline"));
        assert!(body.contains("a death at the tavern"));
        // Names the NPCs that exist in the world (not the player).
        assert!(body.contains("Aldwin"));
    }
}
