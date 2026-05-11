use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::engine::{Event, Goal, Outcome, Time};
use crate::systems::needs::NeedKind;
use crate::systems::traits::Value;
use crate::world::{Npc, NpcId, World};

/// An NPC's currently-pursued goal. Stable strings (`kind`) survive save/load
/// and reconnect to a `GoalDef` in the content pack at runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveGoal {
    pub kind: String,
    pub summary: String,
    pub formed_at: Time,
}

/// A content-pack goal archetype. Loaded from TOML; pure data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalDef {
    pub kind: String,
    pub summary_template: String,
    pub urgency: Score,
    #[serde(default)]
    pub trait_affinity: Vec<(String, f32)>,
    #[serde(default)]
    pub value_affinity: Vec<(Value, f32)>,
    pub precondition: Condition,
    pub resolve: Resolution,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Condition {
    Always,
    Never,
    NeedAtLeast { need: NeedKind, threshold: f32 },
    NeedAtMost { need: NeedKind, threshold: f32 },
    HasTrait { name: String },
    LacksTrait { name: String },
    GrievanceAgainstAnyAtLeast { threshold: i32 },
    AffectionForAnyAtLeast { threshold: i32 },
    ValueAtLeast { value: Value, threshold: f32 },
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Score {
    Const { value: f32 },
    Need { need: NeedKind },
    OneMinusNeed { need: NeedKind },
    GrievanceMax,
    AffectionMax,
    MoodValence,
    Sum { parts: Vec<Score> },
    Scale { by: f32, of: Box<Score> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Resolution {
    NeedBelow { need: NeedKind, threshold: f32 },
    GrievanceMaxBelow { threshold: i32 },
    AffectionMaxAtLeast { threshold: i32 },
    PreconditionLost,
}

const HYSTERESIS: f32 = 0.1;

impl Condition {
    pub fn evaluate(&self, npc: &Npc, world: &World) -> bool {
        match self {
            Condition::Always => true,
            Condition::Never => false,
            Condition::NeedAtLeast { need, threshold } => npc.needs.get(*need) >= *threshold,
            Condition::NeedAtMost { need, threshold } => npc.needs.get(*need) <= *threshold,
            Condition::HasTrait { name } => npc.traits.has(name),
            Condition::LacksTrait { name } => !npc.traits.has(name),
            Condition::GrievanceAgainstAnyAtLeast { threshold } => {
                grievance_max(world, npc.id) >= *threshold
            }
            Condition::AffectionForAnyAtLeast { threshold } => {
                affection_max(world, npc.id) >= *threshold
            }
            Condition::ValueAtLeast { value, threshold } => npc.values.get(*value) >= *threshold,
            Condition::All(parts) => parts.iter().all(|c| c.evaluate(npc, world)),
            Condition::Any(parts) => parts.iter().any(|c| c.evaluate(npc, world)),
            Condition::Not(inner) => !inner.evaluate(npc, world),
        }
    }
}

impl Score {
    pub fn evaluate(&self, npc: &Npc, world: &World) -> f32 {
        match self {
            Score::Const { value } => *value,
            Score::Need { need } => npc.needs.get(*need),
            Score::OneMinusNeed { need } => 1.0 - npc.needs.get(*need),
            Score::GrievanceMax => grievance_max(world, npc.id) as f32 / 100.0,
            Score::AffectionMax => affection_max(world, npc.id) as f32 / 100.0,
            Score::MoodValence => npc.mood.valence,
            Score::Sum { parts } => parts.iter().map(|s| s.evaluate(npc, world)).sum(),
            Score::Scale { by, of } => by * of.evaluate(npc, world),
        }
    }
}

fn grievance_max(world: &World, who: NpcId) -> i32 {
    world
        .relationships_from(who)
        .map(|(_, e)| e.grievance)
        .max()
        .unwrap_or(0)
}

fn affection_max(world: &World, who: NpcId) -> i32 {
    world
        .relationships_from(who)
        .map(|(_, e)| e.affection)
        .max()
        .unwrap_or(0)
}

/// Score a `GoalDef` for an NPC. Combines urgency, trait affinity, and value
/// affinity. Returns a finite f32; nan-safe.
pub fn score_goal(def: &GoalDef, npc: &Npc, world: &World) -> f32 {
    let mut s = def.urgency.evaluate(npc, world);
    for (name, w) in &def.trait_affinity {
        if npc.traits.has(name) {
            s += w;
        }
    }
    for (value, w) in &def.value_affinity {
        s += w * npc.values.get(*value);
    }
    s
}

fn resolves(def: &GoalDef, npc: &Npc, world: &World) -> Option<Outcome> {
    match &def.resolve {
        Resolution::NeedBelow { need, threshold } => (npc.needs.get(*need) < *threshold)
            .then_some(Outcome::Achieved),
        Resolution::GrievanceMaxBelow { threshold } => {
            (grievance_max(world, npc.id) < *threshold).then_some(Outcome::Achieved)
        }
        Resolution::AffectionMaxAtLeast { threshold } => {
            (affection_max(world, npc.id) >= *threshold).then_some(Outcome::Achieved)
        }
        Resolution::PreconditionLost => (!def.precondition.evaluate(npc, world))
            .then_some(Outcome::Abandoned),
    }
}

/// Goal selection and resolution tick. Returns events produced (GoalFormed /
/// GoalResolved) so the caller can append them to the event log.
pub fn tick(
    world: &mut World,
    defs: &[GoalDef],
    now: Time,
    _rng: &mut ChaCha8Rng,
) -> Vec<Event> {
    let mut events = Vec::new();
    let ids: Vec<NpcId> = world.npcs.keys().copied().collect();
    for id in ids {
        // Take a snapshot for read-only scoring without holding &mut on the npc.
        let (active_kind, active_score) = {
            let npc = world.npc(id).expect("npc exists");
            let active = npc.goals.first().cloned();
            let score = active
                .as_ref()
                .and_then(|a| defs.iter().find(|d| d.kind == a.kind))
                .map(|d| score_goal(d, npc, world))
                .unwrap_or(0.0);
            (active.map(|a| a.kind), score)
        };

        // Check resolution on the active goal.
        if let Some(kind) = &active_kind {
            if let Some(def) = defs.iter().find(|d| d.kind == *kind) {
                let npc_ref = world.npc(id).expect("npc exists");
                if let Some(outcome) = resolves(def, npc_ref, world) {
                    let goal = Goal {
                        kind: def.kind.clone(),
                        summary: render_summary(def, npc_ref),
                    };
                    events.push(Event::GoalResolved {
                        holder: id,
                        goal,
                        outcome,
                        when: now,
                    });
                    if let Some(npc) = world.npcs.get_mut(&id) {
                        npc.goals.clear();
                    }
                    continue;
                }
            }
        }

        // Score all eligible defs.
        let npc_ref = world.npc(id).expect("npc exists");
        let mut best: Option<(&GoalDef, f32)> = None;
        for def in defs {
            if !def.precondition.evaluate(npc_ref, world) {
                continue;
            }
            let s = score_goal(def, npc_ref, world);
            if best.map(|(_, bs)| s > bs).unwrap_or(true) {
                best = Some((def, s));
            }
        }

        let Some((best_def, best_score)) = best else {
            continue;
        };

        let should_switch = match &active_kind {
            None => true,
            Some(k) if k != &best_def.kind => best_score > active_score + HYSTERESIS,
            Some(_) => false,
        };

        if should_switch {
            let summary = render_summary(best_def, npc_ref);
            let goal = Goal {
                kind: best_def.kind.clone(),
                summary: summary.clone(),
            };
            events.push(Event::GoalFormed {
                holder: id,
                goal,
                when: now,
            });
            if let Some(npc) = world.npcs.get_mut(&id) {
                npc.goals.clear();
                npc.goals.push(ActiveGoal {
                    kind: best_def.kind.clone(),
                    summary,
                    formed_at: now,
                });
            }
        }
    }
    events
}

fn render_summary(def: &GoalDef, _npc: &Npc) -> String {
    // No template variables yet — the template is static prose. When we add
    // contact names or targets, this becomes the natural place to render them.
    def.summary_template.clone()
}

pub(crate) fn apply_goal_formed(world: &mut World, holder: NpcId, goal: &Goal, when: Time) {
    if let Some(npc) = world.npcs.get_mut(&holder) {
        npc.goals.clear();
        npc.goals.push(ActiveGoal {
            kind: goal.kind.clone(),
            summary: goal.summary.clone(),
            formed_at: when,
        });
    }
}

pub(crate) fn apply_goal_resolved(world: &mut World, holder: NpcId, goal: &Goal, _outcome: &Outcome) {
    if let Some(npc) = world.npcs.get_mut(&holder) {
        npc.goals.retain(|g| g.kind != goal.kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EdgeDelta;
    use crate::systems::relationships::apply_relationship_shift;
    use rand::SeedableRng;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0)
    }

    fn t() -> Time {
        Time { day: 0, minute: 0 }
    }

    fn eat_def() -> GoalDef {
        GoalDef {
            kind: "eat".into(),
            summary_template: "to eat".into(),
            urgency: Score::Need { need: NeedKind::Hunger },
            trait_affinity: vec![],
            value_affinity: vec![(Value::Pleasure, 0.3)],
            precondition: Condition::NeedAtLeast {
                need: NeedKind::Hunger,
                threshold: 0.4,
            },
            resolve: Resolution::NeedBelow {
                need: NeedKind::Hunger,
                threshold: 0.2,
            },
        }
    }

    fn sleep_def() -> GoalDef {
        GoalDef {
            kind: "sleep".into(),
            summary_template: "to sleep".into(),
            urgency: Score::Need { need: NeedKind::Sleep },
            trait_affinity: vec![],
            value_affinity: vec![],
            precondition: Condition::NeedAtLeast {
                need: NeedKind::Sleep,
                threshold: 0.4,
            },
            resolve: Resolution::NeedBelow {
                need: NeedKind::Sleep,
                threshold: 0.2,
            },
        }
    }

    #[test]
    fn condition_dsl_evaluates_basics() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs.hunger = 0.7;
        npc.traits.virtues.push("Kind".into());
        w.insert_npc(npc);
        let npc = w.npc(NpcId(1)).unwrap();

        assert!(Condition::Always.evaluate(npc, &w));
        assert!(!Condition::Never.evaluate(npc, &w));
        assert!(Condition::NeedAtLeast {
            need: NeedKind::Hunger,
            threshold: 0.5,
        }
        .evaluate(npc, &w));
        assert!(Condition::HasTrait {
            name: "Kind".into()
        }
        .evaluate(npc, &w));
        assert!(Condition::LacksTrait {
            name: "Vain".into()
        }
        .evaluate(npc, &w));
        assert!(Condition::Not(Box::new(Condition::HasTrait {
            name: "Kind".into()
        }))
        .evaluate(npc, &w)
            == false);
        assert!(Condition::All(vec![
            Condition::Always,
            Condition::HasTrait { name: "Kind".into() }
        ])
        .evaluate(npc, &w));
        assert!(Condition::Any(vec![Condition::Never, Condition::Always]).evaluate(npc, &w));
    }

    #[test]
    fn score_dsl_evaluates_basics() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs.hunger = 0.8;
        npc.values.set(Value::Faith, 0.5);
        w.insert_npc(npc);
        let npc = w.npc(NpcId(1)).unwrap();

        assert_eq!(Score::Const { value: 0.5 }.evaluate(npc, &w), 0.5);
        assert!((Score::Need { need: NeedKind::Hunger }.evaluate(npc, &w) - 0.8).abs() < 1e-6);
        assert!(
            (Score::OneMinusNeed {
                need: NeedKind::Hunger
            }
            .evaluate(npc, &w)
                - 0.2)
                .abs()
                < 1e-6
        );
        assert_eq!(
            Score::Sum {
                parts: vec![Score::Const { value: 1.0 }, Score::Const { value: 2.0 }]
            }
            .evaluate(npc, &w),
            3.0
        );
        assert_eq!(
            Score::Scale {
                by: 3.0,
                of: Box::new(Score::Const { value: 2.0 }),
            }
            .evaluate(npc, &w),
            6.0
        );
    }

    #[test]
    fn hungry_npc_forms_eat_goal_and_emits_event() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs.hunger = 0.9;
        w.insert_npc(npc);
        let mut r = rng();
        let events = tick(&mut w, &[eat_def()], t(), &mut r);
        assert_eq!(events.len(), 1);
        let goals = &w.npc(NpcId(1)).unwrap().goals;
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].kind, "eat");
        assert!(matches!(&events[0], Event::GoalFormed { holder, goal, .. }
            if *holder == NpcId(1) && goal.kind == "eat"));
    }

    #[test]
    fn goal_resolved_when_need_satisfied() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs.hunger = 0.9;
        npc.goals.push(ActiveGoal {
            kind: "eat".into(),
            summary: "eat".into(),
            formed_at: t(),
        });
        w.insert_npc(npc);
        // Satisfy the hunger entirely.
        w.npcs.get_mut(&NpcId(1)).unwrap().needs.hunger = 0.1;
        let mut r = rng();
        let events = tick(&mut w, &[eat_def()], t(), &mut r);
        assert!(events.iter().any(|e| matches!(e,
            Event::GoalResolved { holder, outcome: Outcome::Achieved, .. }
                if *holder == NpcId(1))));
        assert!(w.npc(NpcId(1)).unwrap().goals.is_empty());
    }

    #[test]
    fn hysteresis_prevents_thrash() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs.hunger = 0.5;
        npc.needs.sleep = 0.5;
        w.insert_npc(npc);
        let mut r = rng();
        // Eat wins narrowly the first tick.
        let _ = tick(&mut w, &[eat_def(), sleep_def()], t(), &mut r);
        let initial_goal = w.npc(NpcId(1)).unwrap().goals[0].kind.clone();
        // Make sleep marginally higher — within hysteresis band — and tick.
        w.npcs.get_mut(&NpcId(1)).unwrap().needs.sleep = 0.55;
        let events = tick(&mut w, &[eat_def(), sleep_def()], t(), &mut r);
        // No GoalFormed event because the swing is inside HYSTERESIS.
        assert!(events.iter().all(|e| !matches!(e, Event::GoalFormed { .. })));
        assert_eq!(w.npc(NpcId(1)).unwrap().goals[0].kind, initial_goal);
    }

    #[test]
    fn precondition_blocks_goal_formation() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.needs.hunger = 0.1; // below precondition threshold
        w.insert_npc(npc);
        let mut r = rng();
        let events = tick(&mut w, &[eat_def()], t(), &mut r);
        assert!(events.is_empty());
        assert!(w.npc(NpcId(1)).unwrap().goals.is_empty());
    }

    #[test]
    fn grievance_score_uses_relationship_max() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        w.insert_npc(Npc::new(NpcId(2), "b"));
        apply_relationship_shift(
            &mut w,
            NpcId(1),
            NpcId(2),
            &EdgeDelta {
                affection: 0,
                trust: 0,
                debt: 0,
                grievance: 60,
                attraction: 0,
            },
        );
        let npc = w.npc(NpcId(1)).unwrap();
        let s = Score::GrievanceMax.evaluate(npc, &w);
        assert!((s - 0.60).abs() < 1e-5);
    }

    #[test]
    fn apply_goal_formed_records_active() {
        let mut w = World::new(0);
        w.insert_npc(Npc::new(NpcId(1), "a"));
        apply_goal_formed(
            &mut w,
            NpcId(1),
            &Goal {
                kind: "eat".into(),
                summary: "to eat".into(),
            },
            t(),
        );
        assert_eq!(w.npc(NpcId(1)).unwrap().goals.len(), 1);
        assert_eq!(w.npc(NpcId(1)).unwrap().goals[0].kind, "eat");
    }

    #[test]
    fn apply_goal_resolved_clears_active() {
        let mut w = World::new(0);
        let mut npc = Npc::new(NpcId(1), "a");
        npc.goals.push(ActiveGoal {
            kind: "eat".into(),
            summary: "eat".into(),
            formed_at: t(),
        });
        w.insert_npc(npc);
        apply_goal_resolved(
            &mut w,
            NpcId(1),
            &Goal {
                kind: "eat".into(),
                summary: "eat".into(),
            },
            &Outcome::Achieved,
        );
        assert!(w.npc(NpcId(1)).unwrap().goals.is_empty());
    }
}
