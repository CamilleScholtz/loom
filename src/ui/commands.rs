//! Slash-commands available in the dialogue input.
//!
//! Output from a command is rendered inline as a `ChatLine::System` so it
//! reads as a quiet aside in the same transcript as the player's lines and
//! the NPC's replies — but `System` lines are filtered out before the
//! conversation is handed to the LLM, so commands never leak into the
//! model's context as if the player had said them.

use crate::app::ChatLine;
use crate::engine::Time;
use crate::systems::Edge;
use crate::ui::palette;
use crate::world::{NpcId, World};

/// One slash-command the player can type during dialogue.
pub struct ChatCommand {
    pub name: &'static str,
    pub blurb: &'static str,
}

/// The canonical list. Order is the order the autocomplete shows them in,
/// which doubles as the order `/help` prints.
pub const CHAT_COMMANDS: &[ChatCommand] = &[
    ChatCommand {
        name: "inspect",
        blurb: "look them over",
    },
    ChatCommand {
        name: "recall",
        blurb: "what you remember of them",
    },
    ChatCommand {
        name: "me",
        blurb: "your own state",
    },
    ChatCommand {
        name: "help",
        blurb: "list these commands",
    },
    ChatCommand {
        name: "leave",
        blurb: "step away",
    },
];

/// Parse a typed dialogue buffer and return the matching command name if
/// the buffer is well-formed and resolves to exactly one known command.
/// Returns `Err` with a one-line system-message when the buffer starts with
/// `/` but doesn't match any command — the caller surfaces this as a
/// `ChatLine::System` rather than sending it to the LLM.
pub fn parse(buffer: &str) -> ParsedCommand<'_> {
    let trimmed = buffer.trim_start();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return ParsedCommand::NotACommand;
    };
    // Allow `/cmd args` shape; only the verb is meaningful for now.
    let verb = rest.split_whitespace().next().unwrap_or("");
    if verb.is_empty() {
        return ParsedCommand::Unknown("");
    }
    match CHAT_COMMANDS.iter().find(|c| c.name.eq_ignore_ascii_case(verb)) {
        Some(cmd) => ParsedCommand::Match(cmd),
        None => ParsedCommand::Unknown(verb),
    }
}

pub enum ParsedCommand<'a> {
    NotACommand,
    Match(&'static ChatCommand),
    /// Buffer started with `/` but the verb didn't match. The caller renders
    /// a "Unknown command" system line.
    Unknown(&'a str),
}

/// Prefix-match for the autocomplete popup. Returns commands whose names
/// start with `prefix` (case-insensitive). Empty prefix returns all.
pub fn matches(prefix: &str) -> Vec<&'static ChatCommand> {
    let p = prefix.to_ascii_lowercase();
    CHAT_COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&p))
        .collect()
}

/// Extract the `/prefix` from a buffer for autocomplete purposes. Returns
/// `Some(prefix_without_slash)` when the buffer is in command-typing form
/// (starts with `/`, no whitespace yet — we don't autocomplete arguments).
pub fn autocomplete_prefix(buffer: &str) -> Option<&str> {
    let rest = buffer.strip_prefix('/')?;
    if rest.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

// ---- handlers ---------------------------------------------------------------
//
// Each handler returns the system-line bodies to append to the transcript.
// `Vec<String>` rather than one big string so the renderer can wrap each
// bullet on its own line without dealing with embedded newlines.

pub fn run_help() -> Vec<String> {
    let mut out = vec!["commands you can use here:".to_string()];
    for cmd in CHAT_COMMANDS {
        out.push(format!("  /{:<8} {}", cmd.name, cmd.blurb));
    }
    out
}

pub fn run_inspect(world: &World, npc: NpcId) -> Vec<String> {
    let Some(n) = world.npc(npc) else {
        return vec!["no one's here to look at.".into()];
    };
    let age = age_label(n.age);
    let occ = n.occupation.clone().unwrap_or_else(|| "no trade you can name".into());
    let mood = palette::mood_label(n.mood.valence)
        .map(|(label, _)| label.to_string())
        .unwrap_or_else(|| "unreadable".into());

    // Player → NPC edge. Phrased as your impression of them, not numbers.
    let edge = world.relationship(NpcId(0), npc);
    let feel = feeling_label(&edge);

    vec![
        format!("you look at {}.", n.name),
        format!("  {} · {} · seems {}", age, occ, mood),
        format!("  you feel: {}", feel),
    ]
}

pub fn run_me(world: &World) -> Vec<String> {
    let Some(p) = world.npc(NpcId(0)) else {
        return vec!["you are nowhere.".into()];
    };
    let mood = palette::mood_label(p.mood.valence)
        .map(|(label, _)| label.to_string())
        .unwrap_or_else(|| "even".into());
    let mut needs: Vec<&'static str> = Vec::new();
    if p.needs.hunger >= 0.55 {
        needs.push("hungry");
    }
    if p.needs.sleep >= 0.55 {
        needs.push("weary");
    }
    if p.needs.belonging >= 0.55 {
        needs.push("alone");
    }
    if p.needs.purpose >= 0.55 {
        needs.push("aimless");
    }
    let needs_line = if needs.is_empty() {
        "none gnawing".to_string()
    } else {
        needs.join(", ")
    };
    vec![
        format!("you, {}.", p.name),
        format!("  mood: {}", mood),
        format!("  needs: {}", needs_line),
    ]
}

pub fn run_recall(world: &World, npc: NpcId) -> Vec<String> {
    let Some(n) = world.npc(npc) else {
        return vec!["no one to recall.".into()];
    };
    // What the player has heard about them, filtered to the player's own
    // knowledge graph. We surface fact summaries the player holds that
    // mention the NPC's name — a coarse but diegetic filter that doesn't
    // require new indexing.
    let Some(player) = world.npc(NpcId(0)) else {
        return vec![format!("you've heard nothing of {}.", n.name)];
    };
    let needle = n.name.to_ascii_lowercase();
    let mut hits: Vec<String> = Vec::new();
    for (fact_id, _edge) in &player.knowledge {
        if let Some(fact) = world.fact(*fact_id) {
            if fact.summary.to_ascii_lowercase().contains(&needle) {
                hits.push(format!("  · {}", fact.summary));
            }
        }
        if hits.len() >= 5 {
            break;
        }
    }
    if hits.is_empty() {
        vec![format!("you've heard nothing of {} yet.", n.name)]
    } else {
        let mut out = vec![format!("what you remember of {}:", n.name)];
        out.extend(hits);
        out
    }
}

pub fn run_unknown(verb: &str) -> Vec<String> {
    if verb.is_empty() {
        vec!["unknown command. try /help.".into()]
    } else {
        vec![format!("unknown command: /{}. try /help.", verb)]
    }
}

/// Run a command by name. The dispatcher is kept here so adding a new entry
/// to `CHAT_COMMANDS` is the only spot that needs editing.
pub fn run(name: &str, world: &World, npc: NpcId, _now: Time) -> Vec<ChatLine> {
    let bodies = match name {
        "help" => run_help(),
        "inspect" => run_inspect(world, npc),
        "me" => run_me(world),
        "recall" => run_recall(world, npc),
        "leave" => Vec::new(), // handled by caller; produces no system lines
        other => run_unknown(other),
    };
    bodies.into_iter().map(ChatLine::System).collect()
}

// ---- helpers ----------------------------------------------------------------

fn age_label(age: u32) -> &'static str {
    match age {
        0..=12 => "a child",
        13..=18 => "still young",
        19..=29 => "young",
        30..=44 => "middle-aged",
        45..=59 => "past their prime",
        _ => "old",
    }
}

/// Coarse "you feel: …" word for the player's edge toward an NPC. Avoids
/// numbers so the system reads as impression rather than scoreboard.
fn feeling_label(edge: &Edge) -> &'static str {
    if edge.grievance >= 40 {
        return "wronged";
    }
    if edge.affection >= 40 && edge.trust >= 20 {
        return "fond of them";
    }
    if edge.affection >= 15 {
        return "warm to them";
    }
    if edge.affection <= -25 || edge.trust <= -25 {
        return "wary";
    }
    if edge.attraction >= 30 {
        return "drawn to them";
    }
    "nothing strong"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_not_a_command() {
        assert!(matches!(parse("hello there"), ParsedCommand::NotACommand));
    }

    #[test]
    fn parse_known() {
        let p = parse("/inspect");
        assert!(matches!(p, ParsedCommand::Match(c) if c.name == "inspect"));
    }

    #[test]
    fn parse_unknown() {
        let p = parse("/banana");
        assert!(matches!(p, ParsedCommand::Unknown(v) if v == "banana"));
    }

    #[test]
    fn parse_case_insensitive() {
        let p = parse("/HELP");
        assert!(matches!(p, ParsedCommand::Match(c) if c.name == "help"));
    }

    #[test]
    fn matches_prefix() {
        let m = matches("i");
        assert!(m.iter().any(|c| c.name == "inspect"));
        let m = matches("");
        assert_eq!(m.len(), CHAT_COMMANDS.len());
        let m = matches("xyz");
        assert!(m.is_empty());
    }

    #[test]
    fn autocomplete_prefix_basic() {
        assert_eq!(autocomplete_prefix("/in"), Some("in"));
        assert_eq!(autocomplete_prefix("/"), Some(""));
        // Stops being autocompletable once an argument starts.
        assert_eq!(autocomplete_prefix("/inspect foo"), None);
        assert_eq!(autocomplete_prefix("plain text"), None);
    }
}
