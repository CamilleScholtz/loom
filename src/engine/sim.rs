use std::collections::BTreeMap;

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use crate::content::ContentRegistry;
use crate::engine::{Cause, EdgeDelta, Event, Fact, Time};
use crate::systems::{apply_event, goals, knowledge, mood, needs, relationships, reputation, romance};
use crate::world::{LocationId, NpcId, World};

/// Advance the world by one in-world day. Runs every Phase 4 system tick in a
/// fixed order, plus the engine-internal `daily_activity_pass`. Returns every
/// event produced this tick so the caller can append to `events.log`.
///
/// This is the single canonical orchestration step for both forward-sim
/// (worldgen) and play-loop (Phase 6+). Callers should not invoke individual
/// system ticks directly.
pub fn step(
    world: &mut World,
    content: &ContentRegistry,
    rng: &mut ChaCha8Rng,
    now: Time,
) -> Vec<Event> {
    needs::tick(world, now, rng);
    mood::tick(world, now, rng);
    relationships::tick(world, now, rng);
    reputation::tick(world, now, rng);

    let mut events = Vec::new();
    events.extend(activity_pass(world, content, rng, now, 1.0));
    events.extend(knowledge::tick_propagation(world, now, rng));
    events.extend(goals::tick(world, &content.setting.goals.defs, now, rng));
    let romance_events = romance::tick(world, now, rng);
    for ev in &romance_events {
        crate::systems::apply_event(world, ev);
    }
    events.extend(romance_events);

    world.day = world.day.saturating_add(1);
    events
}

/// Advance the world by a single play-loop scene's worth of time (~3 in-world
/// hours). Returns `(events, day_rolled_over)`. The caller is responsible for
/// resetting `events_remaining_today` and any front-end day-rollover effects.
///
/// This runs a scaled-down activity pass (so a quarter-day of NPC behavior
/// produces a quarter-day of churn), then the cheap per-tick systems
/// (knowledge propagation, mood, needs). The slow daily-only systems
/// (relationships decay, reputation drift, goals) run only when the clock
/// wraps past midnight.
pub fn tick_scene(
    world: &mut World,
    content: &ContentRegistry,
    rng: &mut ChaCha8Rng,
    now: Time,
) -> (Vec<Event>, bool) {
    let mut events = Vec::new();
    events.extend(activity_pass(world, content, rng, now, 0.25));
    events.extend(knowledge::tick_propagation(world, now, rng));
    mood::tick(world, now, rng);
    needs::tick(world, now, rng);

    let new_minutes = world.clock_minutes.saturating_add(SCENE_MINUTES);
    let rolled = new_minutes >= MINUTES_PER_DAY;
    if rolled {
        world.clock_minutes = new_minutes - MINUTES_PER_DAY;
        // Day-only systems.
        relationships::tick(world, now, rng);
        reputation::tick(world, now, rng);
        events.extend(goals::tick(world, &content.setting.goals.defs, now, rng));
        let romance_events = romance::tick(world, now, rng);
        for ev in &romance_events {
            crate::systems::apply_event(world, ev);
        }
        events.extend(romance_events);
        world.day = world.day.saturating_add(1);
    } else {
        world.clock_minutes = new_minutes;
    }
    (events, rolled)
}

const RANDOM_MOVE_P: f32 = 0.08;
const GOAL_DRIVEN_MOVE_P: f32 = 0.35;
const PAIR_INTERACT_P: f32 = 0.05;
const AFFAIR_TRIGGER_P: f32 = 0.005;

const SCENE_MINUTES: u32 = 180;
const MINUTES_PER_DAY: u32 = 1440;

/// Engine-internal NPC behavior pass. Two phases:
/// 1. Movement — each live NPC may hop to an adjacent location (goal-driven or
///    random walk).
/// 2. Co-located interactions — pairs of NPCs at the same location may have
///    small affection/trust drifts shaped by their value compatibility.
///
/// `scale` is multiplied into every per-NPC probability; 1.0 is one full day,
/// 0.25 is the quarter-day play-loop scene tick.
///
/// All state changes flow through `systems::apply_event` so dispatcher
/// invariants hold.
fn activity_pass(
    world: &mut World,
    content: &ContentRegistry,
    rng: &mut ChaCha8Rng,
    now: Time,
    scale: f32,
) -> Vec<Event> {
    let scale = scale.clamp(0.0, 1.0);
    let random_move_p = RANDOM_MOVE_P * scale;
    let goal_driven_move_p = GOAL_DRIVEN_MOVE_P * scale;
    let pair_interact_p = PAIR_INTERACT_P * scale;
    let affair_trigger_p = AFFAIR_TRIGGER_P * scale;
    let mut events = Vec::new();

    // ---- Phase A: movement ----
    let movers: Vec<NpcId> = world
        .npcs
        .values()
        .filter(|n| !n.dead && n.location.is_some())
        .map(|n| n.id)
        .collect();
    for id in movers {
        let Some(npc) = world.npc(id) else { continue };
        let Some(here) = npc.location else { continue };
        let active_goal_kind = npc.goals.first().map(|g| g.kind.clone());
        let Some(loc) = world.location(here) else { continue };
        if loc.adjacent.is_empty() {
            continue;
        }
        let scheduled_kind = scheduled_location_kind(npc, content, now);
        let has_schedule_pull = scheduled_kind
            .as_deref()
            .map(|k| world.location(here).map(|l| l.kind != k).unwrap_or(true))
            .unwrap_or(false);
        let p = if active_goal_kind.is_some() {
            goal_driven_move_p
        } else if has_schedule_pull {
            goal_driven_move_p
        } else {
            random_move_p
        };
        if rng.r#gen::<f32>() >= p {
            continue;
        }
        let target = pick_destination(
            world,
            id,
            &active_goal_kind,
            scheduled_kind.as_deref(),
            here,
            rng,
        );
        if target == here {
            continue;
        }
        let ev = Event::Moved {
            who: id,
            to: target,
            when: now,
        };
        apply_event(world, &ev);
        events.push(ev);
    }

    // ---- Phase B: co-located interactions ----
    let mut by_loc: BTreeMap<LocationId, Vec<NpcId>> = BTreeMap::new();
    for npc in world.npcs.values() {
        if npc.dead {
            continue;
        }
        if let Some(loc) = npc.location {
            if loc.0 != 0 {
                by_loc.entry(loc).or_default().push(npc.id);
            }
        }
    }
    for (_loc, ids) in &by_loc {
        if ids.len() < 2 {
            continue;
        }
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                if rng.r#gen::<f32>() >= pair_interact_p {
                    continue;
                }
                let a = ids[i];
                let b = ids[j];
                let compat = value_compatibility(world, a, b);
                let (affection_delta, trust_delta) = if compat > 0.6 {
                    (1, 1)
                } else if compat < 0.3 {
                    (-1, 0)
                } else {
                    continue;
                };
                let cause = Cause {
                    summary: if affection_delta > 0 {
                        "found common ground".into()
                    } else {
                        "rubbed each other wrong".into()
                    },
                };
                for (s, o) in [(a, b), (b, a)] {
                    let ev = Event::RelationshipShift {
                        subject: s,
                        object: o,
                        delta: EdgeDelta {
                            affection: affection_delta,
                            trust: trust_delta,
                            debt: 0,
                            grievance: 0,
                            attraction: 0,
                        },
                        cause: cause.clone(),
                        when: now,
                    };
                    apply_event(world, &ev);
                    events.push(ev);
                }
            }
        }
    }

    // ---- Phase C: rare taboo-triggering act (an affair). ----
    // Married NPC + other-than-spouse with high mutual affection, co-located.
    let mut affair_candidates: Vec<(NpcId, NpcId, LocationId)> = Vec::new();
    for npc in world.npcs.values() {
        if npc.dead {
            continue;
        }
        let Some(spouse) = npc.spouse else { continue };
        let Some(here) = npc.location else { continue };
        for (target, edge) in world.relationships_from(npc.id) {
            if target == spouse {
                continue;
            }
            if edge.affection < 40 {
                continue;
            }
            let Some(other) = world.npc(target) else { continue };
            if other.dead {
                continue;
            }
            if other.location != Some(here) {
                continue;
            }
            // Mutual edge required.
            let back = world.relationship(target, npc.id);
            if back.affection < 40 {
                continue;
            }
            affair_candidates.push((npc.id, target, here));
        }
    }
    for (a, b, loc) in affair_candidates {
        if rng.r#gen::<f32>() >= affair_trigger_p {
            continue;
        }
        // Create a fact and emit Witnessed events for any *other* NPC at the
        // same location (the indirect witness — the gossip seed).
        let fact = world.intern_fact(Fact {
            kind: "affair".into(),
            summary: format!(
                "{} and {} together at {}",
                world.npc(a).map(|n| n.name.clone()).unwrap_or_default(),
                world.npc(b).map(|n| n.name.clone()).unwrap_or_default(),
                world.location(loc).map(|l| l.name.clone()).unwrap_or_default(),
            ),
        });
        // The participants witness it themselves.
        for who in [a, b] {
            let ev = Event::Witnessed {
                observer: who,
                fact,
                when: now,
            };
            apply_event(world, &ev);
            events.push(ev);
        }
        // Other NPCs in this location also witness (gossip seed).
        let here_npcs: Vec<NpcId> = world
            .npcs
            .values()
            .filter(|n| !n.dead && n.location == Some(loc) && n.id != a && n.id != b)
            .map(|n| n.id)
            .collect();
        for who in here_npcs {
            let ev = Event::Witnessed {
                observer: who,
                fact,
                when: now,
            };
            apply_event(world, &ev);
            events.push(ev);
        }
    }

    events
}

fn pick_destination(
    world: &World,
    holder: NpcId,
    goal_kind: &Option<String>,
    scheduled_kind: Option<&str>,
    here: LocationId,
    rng: &mut ChaCha8Rng,
) -> LocationId {
    let Some(loc) = world.location(here) else { return here };
    // Targeted goals route toward a specific NPC's location; if that NPC is
    // adjacent (one step away), step toward them.
    let target_id: Option<NpcId> = match goal_kind.as_deref() {
        Some("vendetta") => grievance_target(world, holder),
        Some("support") => affection_target(world, holder),
        _ => None,
    };
    if let Some(target) = target_id {
        if let Some(target_loc) = world.npc(target).and_then(|n| n.location) {
            if loc.adjacent.contains(&target_loc) {
                return target_loc;
            }
        }
    }

    let goal_preferred = match goal_kind.as_deref() {
        Some("eat") | Some("belong") => Some("social"),
        Some("confess") => Some("sacred"),
        Some("earn") => Some("trade"),
        Some("flee") => Some("offmap"),
        _ => None,
    };
    // Goal pull beats schedule pull — actively-pursued goals win the moment.
    let preferred_kind = goal_preferred.or(scheduled_kind);
    if let Some(kind) = preferred_kind {
        let matches: Vec<LocationId> = loc
            .adjacent
            .iter()
            .copied()
            .filter(|id| world.location(*id).map(|l| l.kind == kind).unwrap_or(false))
            .collect();
        if let Some(pick) = matches.first() {
            return *pick;
        }
    }
    // Fallback: random adjacent.
    let idx = rng.gen_range(0..loc.adjacent.len());
    loc.adjacent[idx]
}

fn grievance_target(world: &World, holder: NpcId) -> Option<NpcId> {
    world
        .relationships_from(holder)
        .filter(|(_, e)| e.grievance > 0)
        .max_by_key(|(_, e)| e.grievance)
        .map(|(o, _)| o)
}

fn affection_target(world: &World, holder: NpcId) -> Option<NpcId> {
    world
        .relationships_from(holder)
        .filter(|(_, e)| e.affection > 0)
        .max_by_key(|(_, e)| e.affection)
        .map(|(o, _)| o)
}

/// Resolve the NPC's preferred location `kind` for `now`, applying any
/// per-NPC override first and otherwise consulting their occupation's
/// schedule. Returns `None` if no schedule rule covers the current hour.
fn scheduled_location_kind(
    npc: &crate::world::Npc,
    content: &ContentRegistry,
    now: Time,
) -> Option<String> {
    let hour = (now.minute / 60) as u8;
    if let Some(sched) = npc.daily_schedule.as_ref() {
        // Prefer the block with the highest weight that covers the hour.
        let best = sched
            .blocks
            .iter()
            .filter(|b| hour_in_window(hour, b.start_hour, b.end_hour))
            .max_by(|a, b| a.weight.partial_cmp(&b.weight).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(b) = best {
            return Some(b.location_kind.clone());
        }
    }
    // Fall back to the occupation default.
    let occupation_name = npc.occupation.as_deref()?;
    let occupation = content
        .setting
        .occupations
        .occupations
        .iter()
        .find(|o| o.name == occupation_name)?;
    if !occupation.work_location_kind.is_empty()
        && hour_in_window(hour, occupation.work_hours.0, occupation.work_hours.1)
    {
        Some(occupation.work_location_kind.clone())
    } else if !occupation.home_location_kind.is_empty() {
        Some(occupation.home_location_kind.clone())
    } else {
        None
    }
}

/// `[start, end)` window with optional midnight wrap. When `end <= start` the
/// window crosses midnight (e.g. watchman 20–06). When both are 0, never
/// match — convention for "no fixed hours".
fn hour_in_window(hour: u8, start: u8, end: u8) -> bool {
    if start == 0 && end == 0 {
        return false;
    }
    if end > start {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

/// Cosine-like compatibility over normalized ValueWeights. Returns roughly
/// `[0, 1]` where 1.0 means identical value profiles.
fn value_compatibility(world: &World, a: NpcId, b: NpcId) -> f32 {
    let (Some(na), Some(nb)) = (world.npc(a), world.npc(b)) else {
        return 0.0;
    };
    use crate::systems::Value;
    const ALL: [Value; 8] = [
        Value::Duty,
        Value::Freedom,
        Value::Pleasure,
        Value::Ambition,
        Value::Faith,
        Value::Family,
        Value::Reputation,
        Value::Curiosity,
    ];
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for v in ALL {
        let av = na.values.get(v);
        let bv = nb.values.get(v);
        dot += av * bv;
        norm_a += av * av;
        norm_b += bv * bv;
    }
    let denom = (norm_a.sqrt() * norm_b.sqrt()).max(1e-6);
    (dot / denom).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::ContentRegistry;
    use rand::SeedableRng;
    use std::path::PathBuf;

    fn registry() -> ContentRegistry {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("content");
        ContentRegistry::load(&root, "medieval", "investigator").unwrap()
    }

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0)
    }

    fn t() -> Time {
        Time { day: 0, minute: 0 }
    }

    #[test]
    fn hour_in_window_daytime_inclusive_exclusive() {
        // miller 06..18
        assert!(!hour_in_window(5, 6, 18));
        assert!(hour_in_window(6, 6, 18));
        assert!(hour_in_window(17, 6, 18));
        assert!(!hour_in_window(18, 6, 18));
    }

    #[test]
    fn hour_in_window_wraps_past_midnight() {
        // watchman 20..06
        assert!(hour_in_window(22, 20, 6));
        assert!(hour_in_window(0, 20, 6));
        assert!(hour_in_window(5, 20, 6));
        assert!(!hour_in_window(6, 20, 6));
        assert!(!hour_in_window(12, 20, 6));
    }

    #[test]
    fn hour_in_window_zero_pair_means_no_fixed_hours() {
        for h in 0..24 {
            assert!(!hour_in_window(h, 0, 0));
        }
    }

    #[test]
    fn step_returns_no_events_on_empty_world() {
        let reg = registry();
        let mut w = World::new(0);
        let mut r = rng();
        let events = step(&mut w, &reg, &mut r, t());
        assert!(events.is_empty());
    }

    #[test]
    fn step_advances_day_counter() {
        let reg = registry();
        let mut w = World::new(0);
        let mut r = rng();
        assert_eq!(w.day, 0);
        step(&mut w, &reg, &mut r, t());
        assert_eq!(w.day, 1);
        step(&mut w, &reg, &mut r, t());
        assert_eq!(w.day, 2);
    }

    #[test]
    fn movement_only_lands_on_adjacent_locations() {
        use crate::engine::worldgen;
        use crate::world::{NpcId, PlayerCharacter};
        let reg = registry();
        let player = PlayerCharacter {
            id: NpcId(0),
            name: "P".into(),
            vocation: "investigator".into(),
            virtue: "Kind".into(),
            vice: "Vain".into(),
            inclination: "Scholarly".into(),
            background: "Itinerant".into(),
            sex: crate::world::Sex::Female,
        };
        let r = worldgen::run(&reg, 7, &player).unwrap();
        let mut w = r.world;
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let now = Time {
            day: w.day,
            minute: 0,
        };
        let before: BTreeMap<NpcId, Option<LocationId>> =
            w.npcs.values().map(|n| (n.id, n.location)).collect();
        let _ = activity_pass(&mut w, &reg, &mut rng, now, 1.0);
        for (id, before_loc) in before {
            let after = w.npc(id).unwrap().location;
            if before_loc != after {
                let from = before_loc.unwrap();
                let to = after.unwrap();
                let loc = w.location(from).unwrap();
                assert!(
                    loc.adjacent.contains(&to),
                    "NPC {:?} hopped from {:?} to non-adjacent {:?}",
                    id,
                    from,
                    to
                );
            }
        }
    }

    #[test]
    fn tick_scene_advances_clock_by_three_hours() {
        let reg = registry();
        let mut w = World::new(0);
        w.clock_minutes = 480;
        let mut r = rng();
        let (_, rolled) = tick_scene(&mut w, &reg, &mut r, t());
        assert_eq!(w.clock_minutes, 480 + 180);
        assert!(!rolled);
        assert_eq!(w.day, 0);
    }

    #[test]
    fn tick_scene_wraps_past_midnight_exactly_once() {
        let reg = registry();
        let mut w = World::new(0);
        w.clock_minutes = 1380; // 23:00
        let mut r = rng();
        let (_, rolled) = tick_scene(&mut w, &reg, &mut r, t());
        assert!(rolled);
        assert_eq!(w.day, 1);
        // 1380 + 180 = 1560 → 1560 - 1440 = 120 (02:00).
        assert_eq!(w.clock_minutes, 120);
    }

    #[test]
    fn eight_scene_ticks_roll_day_exactly_once() {
        let reg = registry();
        let mut w = World::new(0);
        w.clock_minutes = 480; // 08:00
        let mut r = rng();
        let mut rolls = 0;
        // 8 ticks = 24 in-world hours = exactly one full day cycle from 08:00.
        for _ in 0..8 {
            let (_, rolled) = tick_scene(&mut w, &reg, &mut r, t());
            if rolled {
                rolls += 1;
            }
        }
        assert_eq!(rolls, 1, "exactly one midnight crossing in 24 hours");
        assert_eq!(w.day, 1);
        assert_eq!(w.clock_minutes, 480);
    }
}
