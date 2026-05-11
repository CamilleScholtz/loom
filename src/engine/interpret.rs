//! Engine rules for turning a structured player-line interpretation into
//! bounded simulation deltas.
//!
//! Per the architectural rule from GAME.md/TECH.md, the LLM provides a typed
//! classification only — variants, not magnitudes — and this module converts
//! it into concrete `Event`s that the simulation already understands. The
//! deltas here are constants weighted by listener traits; the LLM has no say
//! in their size.

use crate::engine::{Cause, Distortion, EdgeDelta, Event, Fact, Time};
use crate::llm::interpret::{
    AssertedFact, Intent, PlayerLineInterpretation, RomanticSignal, Tone, Truthfulness,
};
use crate::world::{MemorableEvent, NpcId, World};

/// Compute the events produced by applying `interp` as a player utterance
/// directed at `listener`. Returned events have NOT been written to state yet;
/// the caller is expected to feed each through `crate::systems::apply_event`
/// and append it to the event log.
///
/// The function is pure with respect to `world`: it reads listener traits to
/// modulate magnitudes, but writes nothing.
pub fn events_for(
    world: &World,
    listener: NpcId,
    line: &str,
    interp: &PlayerLineInterpretation,
    when: Time,
) -> Vec<Event> {
    let player = NpcId(0);
    let mut out: Vec<Event> = Vec::new();

    let (mut delta, summary, valence) = base_delta(interp, line);
    apply_trait_weighting(world, listener, interp, &mut delta);
    apply_romance_compatibility(world, listener, interp, &mut delta);

    if !delta_is_zero(&delta) {
        out.push(Event::RelationshipShift {
            subject: listener,
            object: player,
            delta,
            cause: Cause { summary: summary.clone() },
            when,
        });
    }

    // Always record a memorable event when the line carries a clear signal,
    // even if the delta sums to zero — the NPC noticed, the LLM should know
    // about it next turn.
    if valence.abs() > f32::EPSILON || interp.intent != Intent::Other {
        out.push(Event::Remembered {
            holder: listener,
            event: MemorableEvent {
                when,
                summary,
                valence,
                related_to: Some(player),
                fact: None,
            },
            when,
        });
    }

    out
}

/// Materialized assertion ready for application: the `Fact` to intern, the
/// resolved `about` NpcId (already validated against the world roster — `None`
/// if the model omitted or named a stranger), and the `Distortion` to stamp
/// on the resulting `Event::Told`.
pub struct AssertionApplication {
    pub fact: Fact,
    pub about: Option<NpcId>,
    pub distortion: Distortion,
}

/// Filter and shape the player's `asserts` into facts ready for the caller to
/// intern + emit as `Event::Told`. Invalid `about` ids are dropped (silently
/// — the LLM cannot inject names that don't exist in the world). Empty
/// summaries are dropped. Truthfulness maps to `Distortion`:
/// Earnest → 0, Speculative → 1, Lying → 3.
pub fn assertion_emissions(
    world: &World,
    listener: NpcId,
    interp: &PlayerLineInterpretation,
) -> Vec<AssertionApplication> {
    interp
        .asserts
        .iter()
        .filter_map(|a| {
            let summary = a.summary.trim();
            if summary.is_empty() {
                return None;
            }
            let about = a
                .about
                .map(NpcId)
                .filter(|id| world.npc(*id).is_some());
            let distortion = Distortion {
                level: match a.truthfulness {
                    Truthfulness::Earnest => 0,
                    Truthfulness::Speculative => 1,
                    Truthfulness::Lying => 3,
                },
            };
            // Stamp the listener id into the fact kind so future queries can
            // filter "what the player told Aldwin" from "what Edith told
            // Aldwin"; keeps the kind machine-readable.
            let _ = listener;
            Some(AssertionApplication {
                fact: Fact {
                    kind: "told_by_player".into(),
                    summary: summary.to_string(),
                },
                about,
                distortion,
            })
        })
        .collect()
}

/// Intent label suitable for stamping into a witnessable `Fact.kind`. None
/// means the line carried no signal worth a bystander noticing.
pub fn bystander_fact_kind(interp: &PlayerLineInterpretation) -> Option<&'static str> {
    match interp.intent {
        Intent::Flirt => Some("flirt"),
        Intent::Insult => Some("insult"),
        Intent::Threat => Some("threat"),
        Intent::Accuse => Some("accusation"),
        Intent::Comfort => Some("comfort"),
        Intent::Confront => Some("confrontation"),
        // Ask/Inform/SmallTalk/Other don't read as a witnessable event to a
        // bystander unless tone makes it loud; keep it conservative for v1.
        _ => None,
    }
}

/// Plain-prose summary of the encounter, written from the perspective of an
/// outside observer (no hidden state surfaces here). Returns `None` if no
/// signal is loud enough to register on bystanders.
pub fn bystander_fact_summary(
    world: &World,
    listener: NpcId,
    interp: &PlayerLineInterpretation,
) -> Option<Fact> {
    let kind = bystander_fact_kind(interp)?;
    let player_name = world
        .npc(NpcId(0))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "the stranger".into());
    let listener_name = world
        .npc(listener)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| "another".into());
    let verb_phrase = match interp.intent {
        Intent::Flirt => "flirted with",
        Intent::Insult => "spoke harshly to",
        Intent::Threat => "threatened",
        Intent::Accuse => "accused",
        Intent::Comfort => "showed kindness to",
        Intent::Confront => "confronted",
        _ => return None,
    };
    Some(Fact {
        kind: kind.to_string(),
        summary: format!("{} {} {}", player_name, verb_phrase, listener_name),
    })
}

/// Threshold above which a bystander is "invested" enough in the listener for
/// a witnessed flirt/insult to land as grievance toward the speaker (the
/// player, in this codepath). Tuned low enough that spouses and close friends
/// react, but not the whole tavern.
pub const BYSTANDER_INVESTMENT_THRESHOLD: i32 = 40;

/// Witness-induced grievance events: each bystander invested in `listener`
/// (affection ≥ threshold) gets a `RelationshipShift` against the player with
/// a Cause describing what they saw. Flirts hit hardest; insults/threats
/// against someone they care for raise grievance more sharply.
pub fn bystander_grievance_events(
    world: &World,
    listener: NpcId,
    bystanders: &[NpcId],
    interp: &PlayerLineInterpretation,
    when: Time,
) -> Vec<Event> {
    let player = NpcId(0);
    let kind = match bystander_fact_kind(interp) {
        Some(k) => k,
        None => return Vec::new(),
    };
    let (grievance_bump, affection_bump): (i32, i32) = match interp.intent {
        Intent::Flirt => (3, -1),
        Intent::Insult => (2, -1),
        Intent::Threat => (3, -1),
        Intent::Accuse => (1, 0),
        Intent::Confront => (1, 0),
        // Comfort doesn't punish onlookers.
        Intent::Comfort => (0, 0),
        _ => (0, 0),
    };
    if grievance_bump == 0 && affection_bump == 0 {
        return Vec::new();
    }
    let listener_name = world
        .npc(listener)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| "another".into());
    bystanders
        .iter()
        .copied()
        .filter(|b| *b != player && *b != listener)
        .filter(|b| world.relationship(*b, listener).affection >= BYSTANDER_INVESTMENT_THRESHOLD)
        .map(|b| Event::RelationshipShift {
            subject: b,
            object: player,
            delta: EdgeDelta {
                affection: affection_bump,
                grievance: grievance_bump,
                ..EdgeDelta::default()
            },
            cause: Cause {
                summary: format!("witnessed the player's {} of {}", kind, listener_name),
            },
            when,
        })
        .collect()
}

/// Baseline `(delta, summary, valence)` for an (intent, tone) pair before
/// trait or compatibility weighting. Magnitudes are intentionally small;
/// repetition is what builds lasting grievance, not a single line.
fn base_delta(
    interp: &PlayerLineInterpretation,
    line: &str,
) -> (EdgeDelta, String, f32) {
    let snippet = short_quote(line);
    let mut d = EdgeDelta::default();
    let (summary, valence) = match (interp.intent, interp.tone) {
        (Intent::Insult, _) => {
            d.affection = -2;
            d.trust = -1;
            d.grievance = 3;
            (format!("the player insulted me: {}", snippet), -0.8)
        }
        (Intent::Threat, _) => {
            d.trust = -2;
            d.grievance = 3;
            (format!("the player threatened me: {}", snippet), -0.7)
        }
        (Intent::Accuse, _) => {
            d.trust = -1;
            d.grievance = 2;
            (format!("the player accused me of something: {}", snippet), -0.6)
        }
        (Intent::Confront, Tone::Hostile) => {
            d.trust = -1;
            d.grievance = 2;
            (format!("the player confronted me, hot: {}", snippet), -0.6)
        }
        (Intent::Confront, _) => {
            d.trust = -1;
            d.grievance = 1;
            (format!("the player confronted me: {}", snippet), -0.4)
        }
        (Intent::Comfort, _) => {
            d.affection = 1;
            d.trust = 1;
            (format!("the player was kind to me: {}", snippet), 0.5)
        }
        (Intent::Flirt, _) => {
            // Romance handled fully in `apply_romance_compatibility`.
            (format!("the player flirted: {}", snippet), 0.3)
        }
        (Intent::Inform, _) => (format!("the player told me: {}", snippet), 0.0),
        (Intent::Ask, Tone::Hostile) | (Intent::Ask, Tone::Cold) => {
            d.grievance = 1;
            (format!("the player questioned me pointedly: {}", snippet), -0.2)
        }
        (Intent::Ask, _) => (format!("the player asked me: {}", snippet), 0.0),
        (Intent::SmallTalk, Tone::Warm) | (Intent::SmallTalk, Tone::Playful) => {
            d.affection = 1;
            (format!("the player passed pleasant words with me: {}", snippet), 0.2)
        }
        (Intent::SmallTalk, _) => (format!("the player made small talk: {}", snippet), 0.0),
        (Intent::Other, Tone::Hostile) => {
            d.grievance = 1;
            (format!("the player came at me coldly: {}", snippet), -0.2)
        }
        (Intent::Other, _) => (format!("the player spoke with me: {}", snippet), 0.0),
    };
    (d, summary, valence)
}

/// Listener traits modulate magnitudes. Wrathful: hostile deltas land harder.
/// Kind: hostile deltas soften, warm deltas land a little stronger. Vain: any
/// flirt/insult hits more, since it touches the ego. Keep it cheap and
/// readable — no full matrix.
fn apply_trait_weighting(
    world: &World,
    listener: NpcId,
    interp: &PlayerLineInterpretation,
    delta: &mut EdgeDelta,
) {
    let Some(npc) = world.npc(listener) else { return };
    let has_vice = |name: &str| npc.traits.vices.iter().any(|v| v.eq_ignore_ascii_case(name));
    let has_virtue = |name: &str| {
        npc.traits
            .virtues
            .iter()
            .any(|v| v.eq_ignore_ascii_case(name))
    };

    let hostile = matches!(
        interp.intent,
        Intent::Insult | Intent::Threat | Intent::Accuse | Intent::Confront
    );

    if hostile {
        if has_vice("Wrathful") {
            delta.grievance = (delta.grievance as f32 * 1.5).round() as i32;
            delta.affection = (delta.affection as f32 * 1.3).round() as i32;
        }
        if has_virtue("Kind") {
            delta.grievance = (delta.grievance as f32 * 0.6).round() as i32;
            delta.affection = (delta.affection as f32 * 0.7).round() as i32;
            delta.trust = (delta.trust as f32 * 0.7).round() as i32;
        }
    }

    if matches!(interp.intent, Intent::Comfort) {
        if has_virtue("Kind") {
            delta.affection = (delta.affection as f32 * 1.2).round() as i32;
        }
    }

    if matches!(interp.intent, Intent::Insult | Intent::Flirt) && has_vice("Vain") {
        // Pride is more easily wounded or stoked.
        delta.grievance = (delta.grievance as f32 * 1.3).round() as i32;
        delta.affection = (delta.affection as f32 * 1.3).round() as i32;
    }
}

/// Flirt and explicit romantic signals route entirely through compatibility.
/// A flirt the listener welcomes lifts attraction + affection; an unwelcome
/// flirt drops attraction and adds a small grievance.
fn apply_romance_compatibility(
    world: &World,
    listener: NpcId,
    interp: &PlayerLineInterpretation,
    delta: &mut EdgeDelta,
) {
    let is_romantic = matches!(interp.intent, Intent::Flirt)
        || matches!(
            interp.romantic_signal,
            Some(RomanticSignal::Flirt)
                | Some(RomanticSignal::Confess)
                | Some(RomanticSignal::Reciprocate)
        );
    if !is_romantic {
        if matches!(interp.romantic_signal, Some(RomanticSignal::Rebuff)) {
            delta.attraction = -1;
            delta.affection -= 1;
        }
        return;
    }
    let Some(npc) = world.npc(listener) else { return };
    let player_sex = world.npc(NpcId(0)).map(|p| p.sex);
    let compatible = match player_sex {
        Some(sex) => npc.attracted_to.contains(&sex),
        None => false,
    };
    if compatible {
        delta.attraction += 1;
        delta.affection += 1;
        if matches!(interp.romantic_signal, Some(RomanticSignal::Confess)) {
            delta.attraction += 1; // stronger signal
        }
    } else {
        delta.attraction -= 1;
        delta.grievance += 1;
    }
}

fn delta_is_zero(d: &EdgeDelta) -> bool {
    d.affection == 0 && d.trust == 0 && d.debt == 0 && d.grievance == 0 && d.attraction == 0
}

fn short_quote(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() <= 60 {
        format!("\"{}\"", trimmed)
    } else {
        let cut: String = trimmed.chars().take(57).collect();
        format!("\"{}…\"", cut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Npc, NpcId, Sex};

    fn t() -> Time {
        Time { day: 3, minute: 600 }
    }

    fn world_with(player_sex: Sex, listener_traits: &[(&str, &str)], listener_attracted: &[Sex]) -> World {
        let mut w = World::new(0);
        let mut player = Npc::new(NpcId(0), "Wren");
        player.sex = player_sex;
        w.insert_npc(player);
        let mut listener = Npc::new(NpcId(1), "Aldwin");
        listener.attracted_to = listener_attracted.to_vec();
        for (kind, name) in listener_traits {
            match *kind {
                "virtue" => listener.traits.virtues.push(name.to_string()),
                "vice" => listener.traits.vices.push(name.to_string()),
                _ => {}
            }
        }
        w.insert_npc(listener);
        w
    }

    fn insult() -> PlayerLineInterpretation {
        PlayerLineInterpretation {
            intent: Intent::Insult,
            tone: Tone::Hostile,
            romantic_signal: None,
            references: Vec::new(),
            asserts: Vec::new(),
        }
    }

    #[test]
    fn insult_produces_relationship_shift_and_memory() {
        let w = world_with(Sex::Male, &[], &[]);
        let events = events_for(&w, NpcId(1), "you fool", &insult(), t());
        assert_eq!(events.len(), 2);
        match &events[0] {
            Event::RelationshipShift { subject, object, delta, .. } => {
                assert_eq!(*subject, NpcId(1));
                assert_eq!(*object, NpcId(0));
                assert!(delta.grievance > 0);
                assert!(delta.affection < 0);
            }
            _ => panic!("expected RelationshipShift first"),
        }
        match &events[1] {
            Event::Remembered { holder, event, .. } => {
                assert_eq!(*holder, NpcId(1));
                assert!(event.summary.contains("insulted"));
                assert!(event.valence < 0.0);
            }
            _ => panic!("expected Remembered second"),
        }
    }

    #[test]
    fn wrathful_listener_takes_insult_harder_than_kind_listener() {
        let wr = world_with(Sex::Male, &[("vice", "Wrathful")], &[]);
        let kn = world_with(Sex::Male, &[("virtue", "Kind")], &[]);
        let g_wr = match &events_for(&wr, NpcId(1), "fool", &insult(), t())[0] {
            Event::RelationshipShift { delta, .. } => delta.grievance,
            _ => unreachable!(),
        };
        let g_kn = match &events_for(&kn, NpcId(1), "fool", &insult(), t())[0] {
            Event::RelationshipShift { delta, .. } => delta.grievance,
            _ => unreachable!(),
        };
        assert!(g_wr > g_kn, "wrathful {} should exceed kind {}", g_wr, g_kn);
    }

    #[test]
    fn comfort_lifts_affection_and_trust() {
        let w = world_with(Sex::Male, &[], &[]);
        let interp = PlayerLineInterpretation {
            intent: Intent::Comfort,
            tone: Tone::Warm,
            romantic_signal: None,
            references: Vec::new(),
            asserts: Vec::new(),
        };
        let events = events_for(&w, NpcId(1), "I'm here.", &interp, t());
        let delta = match &events[0] {
            Event::RelationshipShift { delta, .. } => delta.clone(),
            _ => panic!(),
        };
        assert!(delta.affection > 0);
        assert!(delta.trust > 0);
        assert_eq!(delta.grievance, 0);
    }

    #[test]
    fn flirt_to_compatible_listener_raises_attraction() {
        let w = world_with(Sex::Male, &[], &[Sex::Male]);
        let interp = PlayerLineInterpretation {
            intent: Intent::Flirt,
            tone: Tone::Playful,
            romantic_signal: Some(RomanticSignal::Flirt),
            references: Vec::new(),
            asserts: Vec::new(),
        };
        let events = events_for(&w, NpcId(1), "your eyes…", &interp, t());
        let delta = match &events[0] {
            Event::RelationshipShift { delta, .. } => delta.clone(),
            _ => panic!(),
        };
        assert!(delta.attraction > 0);
        assert!(delta.affection > 0);
        assert_eq!(delta.grievance, 0);
    }

    #[test]
    fn flirt_to_incompatible_listener_costs_attraction_and_adds_grievance() {
        let w = world_with(Sex::Male, &[], &[Sex::Female]);
        let interp = PlayerLineInterpretation {
            intent: Intent::Flirt,
            tone: Tone::Playful,
            romantic_signal: Some(RomanticSignal::Flirt),
            references: Vec::new(),
            asserts: Vec::new(),
        };
        let events = events_for(&w, NpcId(1), "your eyes…", &interp, t());
        let delta = match &events[0] {
            Event::RelationshipShift { delta, .. } => delta.clone(),
            _ => panic!(),
        };
        assert!(delta.attraction < 0);
        assert!(delta.grievance > 0);
    }

    #[test]
    fn other_with_neutral_tone_emits_no_relationship_shift() {
        let w = world_with(Sex::Male, &[], &[]);
        let interp = PlayerLineInterpretation::default();
        let events = events_for(&w, NpcId(1), "hm.", &interp, t());
        // No RelationshipShift, no Remembered (default Intent::Other + no
        // valence). The NPC simply doesn't register it.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::RelationshipShift { .. })),
            "no shift for Other/Neutral"
        );
    }

    #[test]
    fn short_quote_truncates_long_lines() {
        let long = "a".repeat(200);
        let q = short_quote(&long);
        assert!(q.ends_with("…\""));
        assert!(q.chars().count() <= 62);
    }

    #[test]
    fn bystander_fact_summary_is_observer_perspective_for_flirt() {
        let w = world_with(Sex::Male, &[], &[]);
        let interp = PlayerLineInterpretation {
            intent: Intent::Flirt,
            tone: Tone::Playful,
            romantic_signal: Some(RomanticSignal::Flirt),
            references: Vec::new(),
            asserts: Vec::new(),
        };
        let fact = bystander_fact_summary(&w, NpcId(1), &interp).unwrap();
        assert_eq!(fact.kind, "flirt");
        assert!(fact.summary.contains("Wren"));
        assert!(fact.summary.contains("Aldwin"));
        assert!(fact.summary.contains("flirted"));
    }

    #[test]
    fn bystander_fact_summary_returns_none_for_quiet_intents() {
        let w = world_with(Sex::Male, &[], &[]);
        for intent in [Intent::Ask, Intent::Inform, Intent::SmallTalk, Intent::Other] {
            let interp = PlayerLineInterpretation {
                intent,
                ..PlayerLineInterpretation::default()
            };
            assert!(bystander_fact_summary(&w, NpcId(1), &interp).is_none());
        }
    }

    fn world_with_three(player_sex: Sex) -> World {
        let mut w = World::new(0);
        let mut player = Npc::new(NpcId(0), "Wren");
        player.sex = player_sex;
        w.insert_npc(player);
        w.insert_npc(Npc::new(NpcId(1), "Aldwin"));
        w.insert_npc(Npc::new(NpcId(2), "Edith"));
        w
    }

    #[test]
    fn invested_bystander_gets_grievance_against_player_on_flirt() {
        use crate::engine::EdgeDelta;
        let mut w = world_with_three(Sex::Male);
        // Edith holds Aldwin dear.
        crate::systems::relationships::apply_relationship_shift(
            &mut w,
            NpcId(2),
            NpcId(1),
            &EdgeDelta {
                affection: 60,
                ..EdgeDelta::default()
            },
        );
        let interp = PlayerLineInterpretation {
            intent: Intent::Flirt,
            tone: Tone::Playful,
            romantic_signal: Some(RomanticSignal::Flirt),
            references: Vec::new(),
            asserts: Vec::new(),
        };
        let events = bystander_grievance_events(&w, NpcId(1), &[NpcId(2)], &interp, t());
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::RelationshipShift { subject, object, delta, cause, .. } => {
                assert_eq!(*subject, NpcId(2));
                assert_eq!(*object, NpcId(0));
                assert!(delta.grievance > 0);
                assert!(cause.summary.contains("flirt"));
                assert!(cause.summary.contains("Aldwin"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn uninvested_bystander_does_not_react() {
        let w = world_with_three(Sex::Male);
        // No relationship set — Edith doesn't care about Aldwin.
        let interp = PlayerLineInterpretation {
            intent: Intent::Flirt,
            tone: Tone::Playful,
            romantic_signal: Some(RomanticSignal::Flirt),
            references: Vec::new(),
            asserts: Vec::new(),
        };
        let events = bystander_grievance_events(&w, NpcId(1), &[NpcId(2)], &interp, t());
        assert!(events.is_empty());
    }

    #[test]
    fn assertion_emissions_drops_invalid_ids_and_maps_truthfulness() {
        use crate::llm::interpret::{AssertedFact, Truthfulness};
        let w = world_with_three(Sex::Male);
        // Ids: 0 (player), 1 (Aldwin), 2 (Edith). 99 is invalid.
        let interp = PlayerLineInterpretation {
            asserts: vec![
                AssertedFact {
                    summary: "Aldwin was at the well at midnight".into(),
                    about: Some(1),
                    truthfulness: Truthfulness::Earnest,
                },
                AssertedFact {
                    summary: "Brother Hew did the deed".into(),
                    about: Some(99),
                    truthfulness: Truthfulness::Lying,
                },
                AssertedFact {
                    summary: "  ".into(), // empty after trim
                    about: Some(2),
                    truthfulness: Truthfulness::Speculative,
                },
            ],
            ..PlayerLineInterpretation::default()
        };
        let apps = assertion_emissions(&w, NpcId(1), &interp);
        // The empty-summary assert was dropped; the two non-empty ones survived.
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].about, Some(NpcId(1)));
        assert_eq!(apps[0].distortion.level, 0);
        // Invalid id silently dropped to None, but the assert itself stays.
        assert_eq!(apps[1].about, None);
        assert_eq!(apps[1].distortion.level, 3, "lying maps to high distortion");
    }

    #[test]
    fn quiet_intents_produce_no_bystander_grievance() {
        let mut w = world_with_three(Sex::Male);
        crate::systems::relationships::apply_relationship_shift(
            &mut w,
            NpcId(2),
            NpcId(1),
            &EdgeDelta {
                affection: 100,
                ..EdgeDelta::default()
            },
        );
        for intent in [Intent::Ask, Intent::Inform, Intent::SmallTalk] {
            let interp = PlayerLineInterpretation {
                intent,
                ..PlayerLineInterpretation::default()
            };
            let events = bystander_grievance_events(&w, NpcId(1), &[NpcId(2)], &interp, t());
            assert!(events.is_empty(), "intent {:?} should not anger bystanders", intent);
        }
    }
}
