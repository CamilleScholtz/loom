use std::collections::{BTreeSet, VecDeque};

use anyhow::Result;
use rand::Rng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;

use crate::content::ContentRegistry;
use crate::engine::{
    DisappearanceReason, Distortion, Event, Incident, IncidentKind, RipeMoment, Time,
};
use crate::systems::{KnowledgeEdge, KnowledgeSource, Value, ValueWeights};
use crate::world::{Location, LocationId, Npc, NpcId, PlayerCharacter, Sex, World};

/// Output of [`run`]: a complete starting state plus the buried mystery and
/// the events emitted along the way for the event log.
pub struct WorldgenResult {
    pub world: World,
    pub incident: Incident,
    pub player_hook: String,
    pub events: Vec<Event>,
}

/// Per-stage seed derivation. Reordering or modifying one stage's logic does
/// not shift the RNG stream for other stages — important when rerunning a
/// fixed seed after a non-population change.
struct WorldgenSeeds {
    geography: u64,
    population: u64,
    family: u64,
    employment: u64,
    secrets: u64,
    forward_sim: u64,
    incident: u64,
    player_placement: u64,
}

impl WorldgenSeeds {
    fn from_root(seed: u64) -> Self {
        Self {
            geography: seed.wrapping_add(0xA001),
            population: seed.wrapping_add(0xA002),
            family: seed.wrapping_add(0xA003),
            employment: seed.wrapping_add(0xA004),
            secrets: seed.wrapping_add(0xA008),
            forward_sim: seed.wrapping_add(0xA005),
            incident: seed.wrapping_add(0xA006),
            player_placement: seed.wrapping_add(0xA007),
        }
    }
}

/// Run the worldgen pipeline. Deterministic given `seed`, `content`, and the
/// player. Produces a populated village with a buried incident, ready for play.
/// The player is inserted as `NpcId(0)` at the entry location.
pub fn run(
    content: &ContentRegistry,
    seed: u64,
    player: &PlayerCharacter,
) -> Result<WorldgenResult> {
    use rand::SeedableRng;

    let seeds = WorldgenSeeds::from_root(seed);
    let mut world = World::new(seed);
    let mut events: Vec<Event> = Vec::new();

    // Stage 1: geography
    lay_geography(
        &mut world,
        content,
        &mut ChaCha8Rng::seed_from_u64(seeds.geography),
    );

    // Stage 2: population
    populate(
        &mut world,
        content,
        &mut ChaCha8Rng::seed_from_u64(seeds.population),
    );

    // Stage 3: families
    let mut family_events = weave_families(
        &mut world,
        &mut ChaCha8Rng::seed_from_u64(seeds.family),
    );
    events.append(&mut family_events);

    // Stage 4: employment
    let mut employment_events = assign_employment(
        &mut world,
        content,
        &mut ChaCha8Rng::seed_from_u64(seeds.employment),
    );
    events.append(&mut employment_events);

    // Stage 4.5: seed per-NPC secrets from the content pack. No events emitted
    // — secrets are NPC state, not log-worthy at worldgen time. Knowledge
    // propagation will leak some via the forward-sim rumor tick.
    seed_secrets(
        &mut world,
        content,
        &mut ChaCha8Rng::seed_from_u64(seeds.secrets),
    );

    // Stage 5: forward simulation
    let mut sim_events = forward_simulate(
        &mut world,
        content,
        &mut ChaCha8Rng::seed_from_u64(seeds.forward_sim),
        FORWARD_SIM_YEARS,
    );
    events.append(&mut sim_events);

    // Stages 6+7: detect ripe moment + resolve incident
    let (incident, mut incident_events) = build_incident(
        &mut world,
        &mut ChaCha8Rng::seed_from_u64(seeds.incident),
    );
    events.append(&mut incident_events);

    // Stage 8: place player
    let player_hook = place_player(
        &mut world,
        player,
        &incident,
        content,
        &mut ChaCha8Rng::seed_from_u64(seeds.player_placement),
    );

    Ok(WorldgenResult {
        world,
        incident,
        player_hook,
        events,
    })
}

const FORWARD_SIM_YEARS: u32 = 5;

// ---- Stage stubs — implemented in subsequent steps. ----

/// Stage 1 — Geography. Instantiates a `Location` per `LocationArchetype` in
/// the setting pack, picks a village name from the place-names pool, and lays
/// a connected adjacency graph (random spanning tree + ~N/3 shortcut edges).
fn lay_geography(world: &mut World, content: &ContentRegistry, rng: &mut ChaCha8Rng) {
    // Pick the village name.
    let villages = &content.setting.place_names.villages;
    world.village_name = villages
        .choose(rng)
        .cloned()
        .unwrap_or_else(|| "Unnamed".to_string());

    // Copy the setting's user-facing terms onto the world so engine helpers
    // and save-round-trip can reach them without holding a ContentRegistry.
    world.place_kind = content.setting.setting.place_kind.clone();
    world.inhabitant_singular = content.setting.setting.inhabitant_singular.clone();
    world.inhabitant_plural = content.setting.setting.inhabitant_plural.clone();

    // Instantiate locations. Reserve LocationId(0) for the off-map sentinel
    // ("elsewhere") used by disappearances; real locations start at id 1.
    let archetypes = &content.setting.locations.locations;
    let elsewhere = Location {
        id: LocationId(0),
        name: "elsewhere".into(),
        kind: "offmap".into(),
        description: "Beyond the village; the road out.".into(),
        adjacent: Vec::new(),
    };
    world.insert_location(elsewhere);

    let mut location_ids: Vec<LocationId> = Vec::new();
    for (i, arch) in archetypes.iter().enumerate() {
        let id = LocationId((i + 1) as u32);
        let loc = Location {
            id,
            name: arch.name.clone(),
            kind: arch.kind.clone(),
            description: arch.description.clone(),
            adjacent: Vec::new(),
        };
        world.insert_location(loc);
        location_ids.push(id);
    }

    // Build a connected graph over location_ids:
    //   1) Spanning tree: shuffle the list, connect each to a random earlier
    //      member to guarantee connectivity.
    //   2) Add ~N/3 random extra edges as shortcuts.
    let mut shuffled = location_ids.clone();
    shuffled.shuffle(rng);
    let mut edges: BTreeSet<(LocationId, LocationId)> = BTreeSet::new();
    for i in 1..shuffled.len() {
        let parent = shuffled[rng.gen_range(0..i)];
        let child = shuffled[i];
        let pair = sorted_pair(parent, child);
        edges.insert(pair);
    }
    let n = shuffled.len();
    let extras = (n / 3).max(1);
    let mut attempts = 0;
    while edges.len() < (n.saturating_sub(1) + extras) && attempts < extras * 10 {
        attempts += 1;
        if n < 2 {
            break;
        }
        let a = shuffled[rng.gen_range(0..n)];
        let b = shuffled[rng.gen_range(0..n)];
        if a == b {
            continue;
        }
        edges.insert(sorted_pair(a, b));
    }

    // Write symmetric adjacency lists onto each Location.
    for (a, b) in &edges {
        if let Some(loc) = world.locations.get_mut(a) {
            loc.adjacent.push(*b);
        }
        if let Some(loc) = world.locations.get_mut(b) {
            loc.adjacent.push(*a);
        }
    }
    for loc in world.locations.values_mut() {
        loc.adjacent.sort();
        loc.adjacent.dedup();
    }

    // Sanity: confirm connectivity over real (non-sentinel) locations.
    debug_assert!(is_connected(world, &location_ids));
}

fn sorted_pair(a: LocationId, b: LocationId) -> (LocationId, LocationId) {
    if a.0 <= b.0 { (a, b) } else { (b, a) }
}

fn is_connected(world: &World, ids: &[LocationId]) -> bool {
    if ids.is_empty() {
        return true;
    }
    let mut seen: BTreeSet<LocationId> = BTreeSet::new();
    let mut q: VecDeque<LocationId> = VecDeque::new();
    let start = ids[0];
    seen.insert(start);
    q.push_back(start);
    while let Some(here) = q.pop_front() {
        let Some(loc) = world.location(here) else { continue };
        for nxt in &loc.adjacent {
            if seen.insert(*nxt) {
                q.push_back(*nxt);
            }
        }
    }
    ids.iter().all(|id| seen.contains(id))
}

const POP_MIN: u32 = 35;
const POP_MAX: u32 = 45;
const ALL_VALUES: [Value; 8] = [
    Value::Duty,
    Value::Freedom,
    Value::Pleasure,
    Value::Ambition,
    Value::Faith,
    Value::Family,
    Value::Reputation,
    Value::Curiosity,
];

/// Stage 2 — Population. Inserts 35–45 NPCs with names, ages, sex, traits,
/// and value weights. Each NPC is placed at a random real (non-sentinel)
/// location. NpcId(0) is reserved for the player and not used here.
fn populate(world: &mut World, content: &ContentRegistry, rng: &mut ChaCha8Rng) {
    let n = rng.gen_range(POP_MIN..=POP_MAX);
    let real_locations: Vec<LocationId> = world
        .locations
        .keys()
        .copied()
        .filter(|id| id.0 != 0)
        .collect();

    let names = &content.setting.names;
    let traits = &content.setting.traits;

    for i in 0..n {
        let id = NpcId(i + 1);
        let sex = if rng.r#gen::<bool>() { Sex::Male } else { Sex::Female };
        let given_pool = match sex {
            Sex::Male => &names.given_male,
            Sex::Female => &names.given_female,
        };
        let given = given_pool
            .choose(rng)
            .cloned()
            .unwrap_or_else(|| "Anon".into());
        let surname = names
            .surnames
            .choose(rng)
            .cloned()
            .unwrap_or_else(|| "Of-Nowhere".into());
        let name = format!("{} {}", given, surname);

        let mut npc = Npc::new(id, name);
        npc.sex = sex;
        npc.age = roll_age(rng);
        npc.attracted_to = roll_attraction(sex, rng);

        // One virtue + one vice + one inclination.
        if let Some(v) = traits.virtues.choose(rng) {
            npc.traits.virtues.push(v.name.clone());
        }
        if let Some(v) = traits.vices.choose(rng) {
            npc.traits.vices.push(v.name.clone());
        }
        if let Some(v) = traits.inclinations.choose(rng) {
            npc.traits.inclinations.push(v.name.clone());
        }

        // Value weights: uniform draw then normalize.
        let mut weights = ValueWeights::default();
        for v in ALL_VALUES {
            weights.set(v, rng.r#gen::<f32>());
        }
        npc.values = weights.normalized();

        // Place at a random real location.
        if !real_locations.is_empty() {
            let pick = real_locations[rng.gen_range(0..real_locations.len())];
            npc.location = Some(pick);
        }

        world.insert_npc(npc);
    }
}

/// Piecewise age distribution favoring adults 20–55; some children and elderly.
/// Approximates a Beta(2,5)-shaped distribution scaled to 1..=80 without
/// pulling in an extra distribution crate.
fn roll_age(rng: &mut ChaCha8Rng) -> u32 {
    let bucket = rng.r#gen::<f32>();
    if bucket < 0.15 {
        rng.gen_range(1..=17) // children + adolescents
    } else if bucket < 0.85 {
        rng.gen_range(20..=55) // working-age adults
    } else {
        rng.gen_range(56..=80) // elders
    }
}

/// Roll a sexuality vector. ~80% opposite-sex-only, ~10% same-sex-only,
/// ~10% bisexual. Asexual NPCs are out of scope for v1 (empty `attracted_to`
/// would model that — we can flip 1% of NPCs to empty later if useful).
fn roll_attraction(sex: Sex, rng: &mut ChaCha8Rng) -> Vec<Sex> {
    let bucket = rng.r#gen::<f32>();
    let opposite = match sex {
        Sex::Male => Sex::Female,
        Sex::Female => Sex::Male,
    };
    if bucket < 0.80 {
        vec![opposite]
    } else if bucket < 0.90 {
        vec![sex]
    } else {
        vec![Sex::Male, Sex::Female]
    }
}

const ADULT_AGE: u32 = 18;
const MAX_MARRIAGE_AGE: u32 = 65;
const MAX_AGE_GAP_FOR_SPOUSE: u32 = 15;
const TARGET_COUPLE_RATE: f32 = 0.60;
const MIN_CHILD_GAP: u32 = 18;
const MAX_CHILD_GAP: u32 = 25;

/// Stage 3 — Families. Picks mutually-attracted, age-compatible adult pairs as
/// spouses; emits a `RelationshipShift` for each spousal bond. From each pair,
/// adopts existing age-appropriate NPCs as children when available. Updates
/// `Npc.spouse`, `Npc.parents`, `Npc.children` fields directly.
fn weave_families(world: &mut World, rng: &mut ChaCha8Rng) -> Vec<Event> {
    use crate::engine::{Cause, EdgeDelta};
    use crate::systems::apply_event;

    let mut events: Vec<Event> = Vec::new();
    let when = Time { day: 0, minute: 0 };

    // -- Spousal pairing --
    let mut unpaired: Vec<NpcId> = world
        .npcs
        .values()
        .filter(|n| {
            n.age >= ADULT_AGE && n.age <= MAX_MARRIAGE_AGE && n.spouse.is_none() && !n.dead
        })
        .map(|n| n.id)
        .collect();
    unpaired.shuffle(rng);

    let target_pairs = (unpaired.len() as f32 * TARGET_COUPLE_RATE / 2.0).round() as usize;
    let mut paired: BTreeSet<NpcId> = BTreeSet::new();

    'outer: while paired.len() < target_pairs * 2 {
        // Pick a still-unpaired starting NPC.
        let mut start_idx = None;
        for (i, id) in unpaired.iter().enumerate() {
            if !paired.contains(id) {
                start_idx = Some(i);
                break;
            }
        }
        let Some(si) = start_idx else { break };
        let a = unpaired[si];

        // Find a compatible partner from the remaining unpaired.
        let mut chosen: Option<NpcId> = None;
        for j in 0..unpaired.len() {
            let b = unpaired[j];
            if b == a || paired.contains(&b) {
                continue;
            }
            if !compatible_for_marriage(world, a, b) {
                continue;
            }
            chosen = Some(b);
            break;
        }
        let Some(b) = chosen else {
            // No partner for `a`; mark them paired with no-one (skip) and
            // continue. Without this they'd loop forever.
            paired.insert(a);
            continue 'outer;
        };

        // Mark spouses both ways.
        if let Some(npc_a) = world.npcs.get_mut(&a) {
            npc_a.spouse = Some(b);
        }
        if let Some(npc_b) = world.npcs.get_mut(&b) {
            npc_b.spouse = Some(a);
        }
        paired.insert(a);
        paired.insert(b);

        // Emit RelationshipShifts in both directions.
        for (subj, obj) in [(a, b), (b, a)] {
            let ev = Event::RelationshipShift {
                subject: subj,
                object: obj,
                delta: EdgeDelta {
                    affection: 70,
                    trust: 60,
                    debt: 0,
                    grievance: 0,
                    attraction: 0,
                },
                cause: Cause {
                    summary: "spousal bond".into(),
                },
                when,
            };
            apply_event(world, &ev);
            events.push(ev);
        }
    }

    // -- Children adoption --
    let spouse_pairs: Vec<(NpcId, NpcId)> = world
        .npcs
        .values()
        .filter_map(|n| n.spouse.and_then(|s| if n.id.0 < s.0 { Some((n.id, s)) } else { None }))
        .collect();

    for (a, b) in spouse_pairs {
        let n_children: u32 = match rng.r#gen::<f32>() {
            x if x < 0.20 => 0,
            x if x < 0.55 => 1,
            x if x < 0.85 => 2,
            x if x < 0.95 => 3,
            _ => 4,
        };
        // Both parents must be at least MIN_CHILD_GAP older than the child,
        // so we gate candidate selection on the *younger* parent's age.
        let younger_parent_age = world
            .npc(a)
            .zip(world.npc(b))
            .map(|(na, nb)| na.age.min(nb.age))
            .unwrap_or(0);
        for _ in 0..n_children {
            let target_min_age_diff = MIN_CHILD_GAP;
            let target_max_age_diff = MAX_CHILD_GAP;
            let candidate: Option<NpcId> = {
                let mut pool: Vec<NpcId> = world
                    .npcs
                    .values()
                    .filter(|c| {
                        c.id != a
                            && c.id != b
                            && c.parents.is_empty()
                            && younger_parent_age.saturating_sub(c.age) >= target_min_age_diff
                            && younger_parent_age.saturating_sub(c.age) <= target_max_age_diff
                    })
                    .map(|c| c.id)
                    .collect();
                pool.shuffle(rng);
                pool.into_iter().next()
            };
            let Some(child) = candidate else { continue };
            if let Some(c) = world.npcs.get_mut(&child) {
                c.parents.push(a);
                c.parents.push(b);
            }
            if let Some(p) = world.npcs.get_mut(&a) {
                p.children.push(child);
            }
            if let Some(p) = world.npcs.get_mut(&b) {
                p.children.push(child);
            }
            // Bidirectional warm-relationship shift parent↔child.
            for (subj, obj) in [(a, child), (child, a), (b, child), (child, b)] {
                let ev = Event::RelationshipShift {
                    subject: subj,
                    object: obj,
                    delta: EdgeDelta {
                        affection: 40,
                        trust: 30,
                        debt: 0,
                        grievance: 0,
                        attraction: 0,
                    },
                    cause: Cause {
                        summary: "parent-child bond".into(),
                    },
                    when,
                };
                apply_event(world, &ev);
                events.push(ev);
            }
        }
    }

    events
}

fn compatible_for_marriage(world: &World, a: NpcId, b: NpcId) -> bool {
    let Some(na) = world.npc(a) else { return false };
    let Some(nb) = world.npc(b) else { return false };
    if na.age < ADULT_AGE || nb.age < ADULT_AGE {
        return false;
    }
    if na.age > MAX_MARRIAGE_AGE || nb.age > MAX_MARRIAGE_AGE {
        return false;
    }
    let age_diff = na.age.abs_diff(nb.age);
    if age_diff > MAX_AGE_GAP_FOR_SPOUSE {
        return false;
    }
    // Mutual attraction.
    if !na.attracted_to.contains(&nb.sex) || !nb.attracted_to.contains(&na.sex) {
        return false;
    }
    // No incest (rare, but guard).
    if na.parents.contains(&b) || nb.parents.contains(&a) {
        return false;
    }
    if na.parents.iter().any(|p| nb.parents.contains(p)) {
        return false;
    }
    true
}

const WORK_MIN_AGE: u32 = 18;
const WORK_MAX_AGE: u32 = 70;
const MAX_SUBORDINATES_PER_OCCUPATION: u32 = 2;

/// Stage 4 — Employment. Assigns each `Occupation` archetype a head NPC,
/// optionally a couple of subordinates, scoring on `typical_traits` match.
/// Sets `Npc.employer` and `Npc.occupation`; emits employer-edge `RelationshipShift`s.
fn assign_employment(
    world: &mut World,
    content: &ContentRegistry,
    rng: &mut ChaCha8Rng,
) -> Vec<Event> {
    use crate::engine::{Cause, EdgeDelta};
    use crate::systems::apply_event;

    let mut events: Vec<Event> = Vec::new();
    let when = Time { day: 0, minute: 0 };

    let mut available: Vec<NpcId> = world
        .npcs
        .values()
        .filter(|n| {
            n.age >= WORK_MIN_AGE
                && n.age <= WORK_MAX_AGE
                && n.occupation.is_none()
                && !n.dead
        })
        .map(|n| n.id)
        .collect();
    available.shuffle(rng);

    for occupation in &content.setting.occupations.occupations {
        if available.is_empty() {
            break;
        }
        // Score each available NPC by trait-match to typical_traits, age fit.
        let mut scored: Vec<(NpcId, i32)> = available
            .iter()
            .map(|id| {
                let npc = world.npc(*id).expect("available list is fresh");
                let mut score = 0i32;
                for t in &occupation.typical_traits {
                    if npc.traits.has(t) || npc.traits.has(&capitalize(t)) {
                        score += 3;
                    }
                }
                // Slight preference for middle-aged heads.
                if (28..=55).contains(&npc.age) {
                    score += 1;
                }
                (*id, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));

        // Head: best scorer.
        let head_id = scored[0].0;
        if let Some(head) = world.npcs.get_mut(&head_id) {
            head.occupation = Some(occupation.name.clone());
            // Head is self-employed.
            head.employer = None;
        }
        available.retain(|id| *id != head_id);

        // Subordinates: 0..=MAX, biased by score.
        let n_sub = rng.gen_range(0..=MAX_SUBORDINATES_PER_OCCUPATION);
        for _ in 0..n_sub {
            if available.is_empty() {
                break;
            }
            // Pick a random remaining adult (slight preference for those who
            // scored well on traits, but kept simple here).
            let idx = rng.gen_range(0..available.len());
            let sub_id = available.remove(idx);
            if let Some(sub) = world.npcs.get_mut(&sub_id) {
                sub.occupation = Some(occupation.name.clone());
                sub.employer = Some(head_id);
            }
            // Subordinate's edge toward their employer.
            let ev = Event::RelationshipShift {
                subject: sub_id,
                object: head_id,
                delta: EdgeDelta {
                    affection: 5,
                    trust: 15,
                    debt: 0,
                    grievance: 0,
                    attraction: 0,
                },
                cause: Cause {
                    summary: format!("works for the {}", occupation.name),
                },
                when,
            };
            apply_event(world, &ev);
            events.push(ev);
            // Employer's edge toward subordinate.
            let ev = Event::RelationshipShift {
                subject: head_id,
                object: sub_id,
                delta: EdgeDelta {
                    affection: 5,
                    trust: 10,
                    debt: 0,
                    grievance: 0,
                    attraction: 0,
                },
                cause: Cause {
                    summary: format!("employs at the {}", occupation.name),
                },
                when,
            };
            apply_event(world, &ev);
            events.push(ev);
        }
    }

    // Remaining adults: tag as "household" if married or has children, else "idle".
    let still_unemployed: Vec<NpcId> = world
        .npcs
        .values()
        .filter(|n| {
            n.age >= WORK_MIN_AGE
                && n.age <= WORK_MAX_AGE
                && n.occupation.is_none()
                && !n.dead
        })
        .map(|n| n.id)
        .collect();
    for id in still_unemployed {
        if let Some(npc) = world.npcs.get_mut(&id) {
            let tag = if npc.spouse.is_some() || !npc.children.is_empty() {
                "household"
            } else {
                "idle"
            };
            npc.occupation = Some(tag.into());
        }
    }
    // Children below working age are tagged "child" for narrative clarity.
    let children: Vec<NpcId> = world
        .npcs
        .values()
        .filter(|n| n.age < WORK_MIN_AGE && n.occupation.is_none())
        .map(|n| n.id)
        .collect();
    for id in children {
        if let Some(npc) = world.npcs.get_mut(&id) {
            npc.occupation = Some("child".into());
        }
    }

    events
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().chain(chars).collect(),
    }
}

const SECRETS_BASE_CHANCE: f32 = 0.30;
const SECRETS_AFFINITY_BONUS: f32 = 0.45;
const SECRETS_SECOND_CHANCE: f32 = 0.18;
const SECRETS_MAX_PER_NPC: usize = 2;

/// Worldgen stage 4.5 — Seed per-NPC secrets from the setting pack.
///
/// For each eligible adult NPC: roll for a primary secret (base chance, lifted
/// when the NPC's traits match the category's `affinity_traits`); if one
/// lands, roll a second at a smaller chance from a different category. Each
/// secret interns a `Fact` in the world registry whose summary substitutes the
/// holder's name (and, for pair-keyed categories like adultery/parentage, the
/// name of a randomly picked partner). The fact lives only on the holder's
/// `secrets` list at this point; the forward-sim rumor tick decides which
/// ones leak over the 5-year prelude.
fn seed_secrets(world: &mut World, content: &ContentRegistry, rng: &mut ChaCha8Rng) {
    let categories = &content.setting.secrets.categories;
    if categories.is_empty() {
        return;
    }

    // Collect ids up-front so we can mutate world.facts and world.npcs
    // independently without re-borrowing the iterator each pass.
    let candidates: Vec<(NpcId, u32, Vec<String>)> = world
        .npcs
        .values()
        .filter(|n| !n.dead)
        .map(|n| {
            let mut tags: Vec<String> = Vec::new();
            tags.extend(n.traits.virtues.iter().cloned());
            tags.extend(n.traits.vices.iter().cloned());
            tags.extend(n.traits.inclinations.iter().cloned());
            (n.id, n.age, tags)
        })
        .collect();

    // Pool of other adult ids for the `{{other}}` substitution.
    let adults: Vec<NpcId> = candidates
        .iter()
        .filter(|(_, age, _)| *age >= 18)
        .map(|(id, _, _)| *id)
        .collect();

    for (id, age, traits) in candidates {
        // Pick the eligible category list for this NPC (gated by min_age).
        let eligible: Vec<&crate::content::secrets::SecretCategory> = categories
            .iter()
            .filter(|c| age >= c.min_age)
            .collect();
        if eligible.is_empty() {
            continue;
        }

        let weighted: Vec<(usize, f32)> = eligible
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let affinity = c
                    .affinity_traits
                    .iter()
                    .any(|t| traits.iter().any(|nt| nt.eq_ignore_ascii_case(t)));
                let w = if affinity {
                    SECRETS_BASE_CHANCE + SECRETS_AFFINITY_BONUS
                } else {
                    SECRETS_BASE_CHANCE
                };
                (i, w)
            })
            .collect();

        // Primary secret: weighted pick over eligible categories; only land
        // it if the rolled weight survives a uniform-[0,1] gate. (Keeps the
        // overall fraction of secret-holders bounded.)
        let primary_idx = pick_weighted(rng, &weighted);
        let Some(idx) = primary_idx else { continue };
        let cat = eligible[idx];
        if rng.r#gen::<f32>() > weighted.iter().find(|(i, _)| *i == idx).map(|(_, w)| *w).unwrap_or(0.0) {
            continue;
        }
        place_secret(world, rng, id, cat, &adults);

        // Roll for an optional second secret at a tighter rate, from a
        // different category.
        if rng.r#gen::<f32>() < SECRETS_SECOND_CHANCE
            && world.npc(id).map(|n| n.secrets.len()).unwrap_or(0) < SECRETS_MAX_PER_NPC
        {
            let remaining: Vec<&crate::content::secrets::SecretCategory> = eligible
                .iter()
                .copied()
                .filter(|c| c.name != cat.name)
                .collect();
            if let Some(second) = remaining.choose(rng) {
                place_secret(world, rng, id, second, &adults);
            }
        }
    }
}

fn place_secret(
    world: &mut World,
    rng: &mut ChaCha8Rng,
    holder: NpcId,
    cat: &crate::content::secrets::SecretCategory,
    adults: &[NpcId],
) {
    let template = match cat.summary_templates.choose(rng) {
        Some(t) => t,
        None => return,
    };
    let holder_name = world
        .npc(holder)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| "someone".into());
    let other_name = adults
        .iter()
        .filter(|id| **id != holder)
        .copied()
        .collect::<Vec<_>>()
        .choose(rng)
        .and_then(|id| world.npc(*id).map(|n| n.name.clone()))
        .unwrap_or_else(|| "another".into());
    let summary = template
        .replace("{{subject}}", &holder_name)
        .replace("{{other}}", &other_name);
    let fact = world.intern_fact(crate::engine::Fact {
        kind: cat.name.clone(),
        summary,
    });
    if let Some(npc) = world.npcs.get_mut(&holder) {
        // Holder gains direct knowledge of their own secret.
        npc.knowledge.insert(
            fact,
            crate::systems::KnowledgeEdge {
                source: crate::systems::KnowledgeSource::Witnessed,
                confidence: 1.0,
                distortion: crate::engine::Distortion { level: 0 },
                acquired_at: Time { day: 0, minute: 0 },
            },
        );
        npc.secrets.push(crate::world::Secret {
            fact,
            category: cat.name.clone(),
        });
    }
}

fn pick_weighted(rng: &mut ChaCha8Rng, items: &[(usize, f32)]) -> Option<usize> {
    if items.is_empty() {
        return None;
    }
    let total: f32 = items.iter().map(|(_, w)| *w).sum();
    if total <= 0.0 {
        return None;
    }
    let mut r = rng.r#gen::<f32>() * total;
    for (idx, w) in items {
        if r < *w {
            return Some(*idx);
        }
        r -= w;
    }
    items.last().map(|(idx, _)| *idx)
}

/// Stage 5 — Forward simulation. Run `engine::sim::step` for `years × 365`
/// ticks. Every system tick runs each day; pressures accumulate naturally.
/// Returns every event produced during the sim.
fn forward_simulate(
    world: &mut World,
    content: &ContentRegistry,
    rng: &mut ChaCha8Rng,
    years: u32,
) -> Vec<Event> {
    use crate::engine::sim;
    let total_days = years.saturating_mul(365);
    let mut all_events = Vec::new();
    for _ in 0..total_days {
        let now = Time {
            day: world.day,
            minute: 0,
        };
        let evs = sim::step(world, content, rng, now);
        all_events.extend(evs);
    }
    all_events
}

const MAX_NUDGE_RETRIES: u32 = 3;

/// Stages 6+7 — Detect a ripe moment from accumulated pressures; if none
/// fires above threshold, nudge a grievance edge over the line and retry.
/// Always returns an `Incident`.
fn build_incident(world: &mut World, rng: &mut ChaCha8Rng) -> (Incident, Vec<Event>) {
    use crate::engine::incident::{detect_ripe, nudge_pressure, resolve};
    for _ in 0..MAX_NUDGE_RETRIES {
        if let Some(ripe) = detect_ripe(world) {
            return resolve(world, ripe, rng);
        }
        if !nudge_pressure(world, rng) {
            break;
        }
    }
    // Fallback: a quiet disappearance of the oldest unattached NPC.
    let who = world
        .npcs
        .values()
        .filter(|n| !n.dead && n.spouse.is_none())
        .max_by_key(|n| n.age)
        .map(|n| n.id)
        .unwrap_or(NpcId(1));
    let ripe = RipeMoment {
        kind: IncidentKind::Disappearance {
            who,
            reason: DisappearanceReason::Fled,
        },
        score: 0.0,
        when: Time {
            day: world.day,
            minute: 720,
        },
        location: world.npc(who).and_then(|n| n.location).unwrap_or(LocationId(0)),
    };
    resolve(world, ripe, rng)
}

const DEADLINE_MIN_DAYS: u32 = 4;
const DEADLINE_MAX_DAYS: u32 = 14;
const PLAYER_INITIAL_CONFIDENCE: f32 = 0.5;

/// Stage 8 — Place the player. Inserts the player as `NpcId(0)`, seeds their
/// knowledge with the incident's public facts, rolls the deadline, and renders
/// the vocation pack's opening hook with substitution. Returns the rendered
/// hook text for the caller to display.
fn place_player(
    world: &mut World,
    player: &PlayerCharacter,
    incident: &Incident,
    content: &ContentRegistry,
    rng: &mut ChaCha8Rng,
) -> String {
    // Set start_day and deadline_day before inserting the player so they read
    // the same `now`.
    world.start_day = world.day;
    world.deadline_day = world
        .day
        .saturating_add(rng.gen_range(DEADLINE_MIN_DAYS..=DEADLINE_MAX_DAYS));

    // Entry location: prefer "social" (tavern), fall back to "sacred"
    // (chapel), then anything except the offmap sentinel.
    let entry = choose_entry_location(world);
    let now = Time {
        day: world.day,
        minute: 720,
    };

    let mut npc = Npc::new(NpcId(0), player.name.clone());
    if !player.virtue.is_empty() {
        npc.traits.virtues.push(player.virtue.clone());
    }
    if !player.vice.is_empty() {
        npc.traits.vices.push(player.vice.clone());
    }
    if !player.inclination.is_empty() {
        npc.traits.inclinations.push(player.inclination.clone());
    }
    npc.age = 30;
    npc.sex = Sex::default();
    npc.attracted_to = vec![Sex::Male, Sex::Female]; // player orientation is its own decision; default open
    npc.location = Some(entry);
    npc.occupation = Some(player.vocation.clone());

    // Seed initial knowledge from incident.public_facts.
    for fact in &incident.public_facts {
        npc.knowledge.insert(
            *fact,
            KnowledgeEdge {
                source: KnowledgeSource::Witnessed,
                confidence: PLAYER_INITIAL_CONFIDENCE,
                distortion: Distortion { level: 0 },
                acquired_at: now,
            },
        );
    }

    world.insert_npc(npc);

    render_opening_hook(world, player, incident, content, entry)
}

fn choose_entry_location(world: &World) -> LocationId {
    if let Some(loc) = world
        .locations
        .values()
        .find(|l| l.id.0 != 0 && l.kind == "social")
    {
        return loc.id;
    }
    if let Some(loc) = world
        .locations
        .values()
        .find(|l| l.id.0 != 0 && l.kind == "sacred")
    {
        return loc.id;
    }
    world
        .locations
        .values()
        .find(|l| l.id.0 != 0)
        .map(|l| l.id)
        .unwrap_or(LocationId(1))
}

fn render_opening_hook(
    world: &World,
    _player: &PlayerCharacter,
    incident: &Incident,
    content: &ContentRegistry,
    entry: LocationId,
) -> String {
    let template = &content.vocation.opening_hook.template;
    let incident_label = match &incident.kind {
        IncidentKind::Death { .. } => "a death",
        IncidentKind::Disappearance { .. } => "a disappearance",
        IncidentKind::Scandal { .. } => "a scandal",
    };
    let deadline_days = world.deadline_day.saturating_sub(world.start_day).to_string();
    let entry_name = world
        .location(entry)
        .map(|l| l.name.clone())
        .unwrap_or_default();
    template
        .replace("{{village.name}}", &world.village_name)
        .replace("{{arrival.day}}", "the eve")
        .replace("{{arrival.season}}", "autumn")
        .replace("{{summoner.role}}", "the magistrate")
        .replace("{{summoner.name}}", "Magistrate")
        .replace("{{incident.kind}}", incident_label)
        .replace("{{deadline.days}}", &deadline_days)
        .replace("{{first.location}}", &entry_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
    fn run_with_a_seed_produces_a_result() {
        let reg = registry();
        let r = run(&reg, 42, &fake_player()).expect("worldgen runs");
        assert_eq!(r.world.seed, 42);
        assert!(!r.world.village_name.is_empty());
        // Player inserted as NpcId(0).
        let player_npc = r.world.npc(NpcId(0)).expect("player exists");
        assert_eq!(player_npc.name, "Quintus Wren");
        assert!(player_npc.location.is_some());
        // Deadline rolled.
        assert!(r.world.deadline_day > r.world.start_day);
        assert!(r.world.deadline_day - r.world.start_day <= DEADLINE_MAX_DAYS);
        // Player has seeded knowledge from public_facts.
        for f in &r.incident.public_facts {
            assert!(player_npc.knowledge.contains_key(f));
        }
        // Hook substitution worked.
        assert!(!r.player_hook.contains("{{"));
        assert!(r.player_hook.contains(&r.world.village_name));
    }

    #[test]
    fn seeds_are_deterministic_from_root() {
        let a = WorldgenSeeds::from_root(123);
        let b = WorldgenSeeds::from_root(123);
        assert_eq!(a.geography, b.geography);
        assert_eq!(a.population, b.population);
        assert_eq!(a.family, b.family);
        assert_ne!(a.geography, a.population);
    }

    use rand::SeedableRng;

    #[test]
    fn lay_geography_picks_a_village_name() {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(7));
        assert!(!w.village_name.is_empty());
        assert!(reg.setting.place_names.villages.contains(&w.village_name));
    }

    #[test]
    fn lay_geography_instantiates_one_per_archetype_plus_sentinel() {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(7));
        // +1 for the LocationId(0) "elsewhere" sentinel.
        assert_eq!(
            w.locations.len(),
            reg.setting.locations.locations.len() + 1
        );
        // Sentinel is present and unconnected.
        let sentinel = w.location(LocationId(0)).unwrap();
        assert_eq!(sentinel.name, "elsewhere");
        assert!(sentinel.adjacent.is_empty());
    }

    #[test]
    fn lay_geography_produces_connected_graph() {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(7));
        let ids: Vec<LocationId> = w
            .locations
            .keys()
            .copied()
            .filter(|id| id.0 != 0)
            .collect();
        assert!(is_connected(&w, &ids));
    }

    #[test]
    fn lay_geography_adjacency_is_symmetric() {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(7));
        for (id, loc) in &w.locations {
            for nbr in &loc.adjacent {
                let nb = w.location(*nbr).unwrap();
                assert!(
                    nb.adjacent.contains(id),
                    "adjacency not symmetric: {} -> {} but not back",
                    id.0,
                    nbr.0
                );
            }
        }
    }

    #[test]
    fn lay_geography_is_deterministic() {
        let reg = registry();
        let mut a = World::new(0);
        let mut b = World::new(0);
        lay_geography(&mut a, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        lay_geography(&mut b, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        assert_eq!(a.village_name, b.village_name);
        assert_eq!(a.locations, b.locations);
    }

    fn populated_world() -> World {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        populate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        w
    }

    #[test]
    fn populate_size_in_range() {
        let w = populated_world();
        let n = w.npcs.len() as u32;
        assert!(
            (POP_MIN..=POP_MAX).contains(&n),
            "population {} out of range [{}, {}]",
            n,
            POP_MIN,
            POP_MAX
        );
    }

    #[test]
    fn every_npc_has_one_virtue_one_vice_one_inclination() {
        let w = populated_world();
        for npc in w.npcs.values() {
            assert_eq!(npc.traits.virtues.len(), 1, "{} virtues", npc.name);
            assert_eq!(npc.traits.vices.len(), 1, "{} vices", npc.name);
            assert_eq!(npc.traits.inclinations.len(), 1, "{} inclinations", npc.name);
        }
    }

    #[test]
    fn every_npc_has_a_location_in_the_real_set() {
        let w = populated_world();
        let real_locs: std::collections::BTreeSet<LocationId> =
            w.locations.keys().copied().filter(|id| id.0 != 0).collect();
        for npc in w.npcs.values() {
            let loc = npc.location.expect("npc placed");
            assert!(real_locs.contains(&loc));
        }
    }

    #[test]
    fn npc_ids_skip_zero_so_player_can_take_it() {
        let w = populated_world();
        assert!(w.npc(NpcId(0)).is_none());
        for id in w.npcs.keys() {
            assert!(id.0 >= 1);
        }
    }

    #[test]
    fn names_come_from_pack() {
        let reg = registry();
        let w = populated_world();
        let male: std::collections::BTreeSet<&str> =
            reg.setting.names.given_male.iter().map(|s| s.as_str()).collect();
        let female: std::collections::BTreeSet<&str> =
            reg.setting.names.given_female.iter().map(|s| s.as_str()).collect();
        for npc in w.npcs.values() {
            let given = npc.name.split_whitespace().next().unwrap();
            assert!(
                male.contains(given) || female.contains(given),
                "given name {:?} not from pack",
                given
            );
        }
    }

    #[test]
    fn value_weights_normalize_close_to_one() {
        let w = populated_world();
        for npc in w.npcs.values() {
            let total: f32 = npc.values.0.values().sum();
            assert!((total - 1.0).abs() < 1e-4, "values sum to {}", total);
        }
    }

    #[test]
    fn populate_is_deterministic() {
        let reg = registry();
        let mut a = World::new(0);
        let mut b = World::new(0);
        lay_geography(&mut a, &reg, &mut ChaCha8Rng::seed_from_u64(99));
        lay_geography(&mut b, &reg, &mut ChaCha8Rng::seed_from_u64(99));
        populate(&mut a, &reg, &mut ChaCha8Rng::seed_from_u64(99));
        populate(&mut b, &reg, &mut ChaCha8Rng::seed_from_u64(99));
        let an: Vec<_> = a.npcs.values().map(|n| (n.id, n.name.clone())).collect();
        let bn: Vec<_> = b.npcs.values().map(|n| (n.id, n.name.clone())).collect();
        assert_eq!(an, bn);
    }

    fn world_with_families() -> World {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        populate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        let _ = weave_families(&mut w, &mut ChaCha8Rng::seed_from_u64(42));
        w
    }

    #[test]
    fn no_self_spouse() {
        let w = world_with_families();
        for npc in w.npcs.values() {
            if let Some(s) = npc.spouse {
                assert_ne!(s, npc.id, "{} married themselves", npc.name);
            }
        }
    }

    #[test]
    fn spouse_links_are_symmetric() {
        let w = world_with_families();
        for npc in w.npcs.values() {
            if let Some(s) = npc.spouse {
                let other = w.npc(s).expect("spouse exists");
                assert_eq!(other.spouse, Some(npc.id), "{} <-> {}", npc.name, other.name);
            }
        }
    }

    #[test]
    fn parents_older_than_children() {
        let w = world_with_families();
        for npc in w.npcs.values() {
            for parent_id in &npc.parents {
                let parent = w.npc(*parent_id).expect("parent exists");
                assert!(
                    parent.age >= npc.age + MIN_CHILD_GAP,
                    "{} (age {}) parent of {} (age {})",
                    parent.name,
                    parent.age,
                    npc.name,
                    npc.age
                );
            }
        }
    }

    #[test]
    fn children_have_either_zero_or_two_parents_in_v1() {
        let w = world_with_families();
        for npc in w.npcs.values() {
            assert!(
                npc.parents.is_empty() || npc.parents.len() == 2,
                "{} has {} parents",
                npc.name,
                npc.parents.len()
            );
        }
    }

    #[test]
    fn parent_child_links_are_symmetric() {
        let w = world_with_families();
        for npc in w.npcs.values() {
            for child_id in &npc.children {
                let child = w.npc(*child_id).expect("child exists");
                assert!(
                    child.parents.contains(&npc.id),
                    "{}'s child {} doesn't list them as parent",
                    npc.name,
                    child.name
                );
            }
            for parent_id in &npc.parents {
                let parent = w.npc(*parent_id).expect("parent exists");
                assert!(
                    parent.children.contains(&npc.id),
                    "{} not in {}'s children",
                    npc.name,
                    parent.name
                );
            }
        }
    }

    #[test]
    fn at_least_some_couples_form() {
        let w = world_with_families();
        let n_paired = w.npcs.values().filter(|n| n.spouse.is_some()).count();
        // We target ~60% of adults paired; require at least a few to confirm
        // the matcher fires.
        assert!(n_paired >= 4, "only {} paired npcs", n_paired);
    }

    fn world_with_employment() -> (World, ContentRegistry) {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        populate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        let _ = weave_families(&mut w, &mut ChaCha8Rng::seed_from_u64(42));
        let _ = assign_employment(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        (w, reg)
    }

    #[test]
    fn every_occupation_archetype_has_a_head() {
        let (w, reg) = world_with_employment();
        for occ in &reg.setting.occupations.occupations {
            let head = w
                .npcs
                .values()
                .find(|n| n.occupation.as_deref() == Some(occ.name.as_str()) && n.employer.is_none());
            assert!(head.is_some(), "no head for occupation {:?}", occ.name);
        }
    }

    #[test]
    fn employer_resolves_to_an_existing_head() {
        let (w, _) = world_with_employment();
        for npc in w.npcs.values() {
            if let Some(emp_id) = npc.employer {
                let head = w.npc(emp_id).expect("employer exists");
                assert!(head.occupation.is_some());
            }
        }
    }

    #[test]
    fn occupation_name_matches_pack_or_known_tag() {
        let (w, reg) = world_with_employment();
        let valid: std::collections::BTreeSet<String> = reg
            .setting
            .occupations
            .occupations
            .iter()
            .map(|o| o.name.clone())
            .chain(["household".into(), "idle".into(), "child".into()])
            .collect();
        for npc in w.npcs.values() {
            if let Some(occ) = &npc.occupation {
                assert!(valid.contains(occ), "unknown occupation: {:?}", occ);
            }
        }
    }

    #[test]
    fn employment_is_deterministic() {
        let reg = registry();
        let mut a = World::new(0);
        let mut b = World::new(0);
        for w in [&mut a, &mut b] {
            lay_geography(w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
            populate(w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
            let _ = weave_families(w, &mut ChaCha8Rng::seed_from_u64(42));
            let _ = assign_employment(w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        }
        let aocc: Vec<_> = a.npcs.values().map(|n| n.occupation.clone()).collect();
        let bocc: Vec<_> = b.npcs.values().map(|n| n.occupation.clone()).collect();
        assert_eq!(aocc, bocc);
    }

    #[test]
    fn forward_simulate_advances_day_count() {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        populate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        let _ = weave_families(&mut w, &mut ChaCha8Rng::seed_from_u64(42));
        let _ = assign_employment(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42));
        let start = w.day;
        // Short forward sim — 30 days — to keep the test fast.
        let _ = forward_simulate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42), 0);
        assert_eq!(w.day, start);
        // One year:
        let _ = forward_simulate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(42), 1);
        assert_eq!(w.day, start + 365);
    }

    #[test]
    fn forward_simulate_produces_some_state_changes() {
        let reg = registry();
        let mut w = World::new(0);
        lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(99));
        populate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(99));
        let _ = weave_families(&mut w, &mut ChaCha8Rng::seed_from_u64(99));
        let _ = assign_employment(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(99));
        let initial_relationships = w.relationships.len();
        // Two years.
        let events = forward_simulate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(99), 2);
        assert!(!events.is_empty(), "expected forward sim to produce events");
        // Relationships should have grown (co-located pair interactions).
        assert!(
            w.relationships.len() >= initial_relationships,
            "relationships shrank: {} → {}",
            initial_relationships,
            w.relationships.len()
        );
    }

    #[test]
    fn forward_simulate_is_deterministic() {
        let reg = registry();
        let build = |seed: u64| -> World {
            let mut w = World::new(0);
            lay_geography(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(seed));
            populate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(seed));
            let _ = weave_families(&mut w, &mut ChaCha8Rng::seed_from_u64(seed));
            let _ = assign_employment(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(seed));
            let _ = forward_simulate(&mut w, &reg, &mut ChaCha8Rng::seed_from_u64(seed), 2);
            w
        };
        let a = build(11);
        let b = build(11);
        let a_locs: Vec<_> = a.npcs.values().map(|n| (n.id, n.location)).collect();
        let b_locs: Vec<_> = b.npcs.values().map(|n| (n.id, n.location)).collect();
        assert_eq!(a_locs, b_locs);
    }
}
