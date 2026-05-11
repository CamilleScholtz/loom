//! Centralized colour and style helpers for the TUI.
//!
//! The rule for this module: only use the 16 ANSI palette colours and
//! `Color::Reset` — never `Rgb`, `Indexed`, or terminal-specific values like
//! `DarkGray`. The terminal owns the actual hues; we own the semantic mapping.
//!
//! Emphasis is carried by `Modifier` (BOLD, DIM, ITALIC, REVERSED), so a
//! selected item, a faded hint, or a heading all read correctly under any
//! user theme — light or dark, solarized or default.

use ratatui::style::{Color, Modifier, Style};

use crate::world::NpcId;

/// Six-colour palette used for per-NPC identity, hashed from `NpcId`. The
/// player (NpcId(0)) is handled separately by `player()` and never appears
/// here, so we keep the palette to colours that read well against any theme
/// and don't collide with semantic uses below (deadline red, warning yellow).
const NPC_PALETTE: [Color; 6] = [
    Color::Magenta,
    Color::Cyan,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Red,
];

/// Stable per-NPC colour. Hashing `NpcId.0` directly keeps the assignment
/// reproducible across renders without needing a side table on the World.
pub fn npc_color(id: NpcId) -> Color {
    // Cheap mixer — id is u32 and we just need a deterministic spread.
    let mut x = id.0.wrapping_mul(2654435761);
    x ^= x >> 16;
    NPC_PALETTE[(x as usize) % NPC_PALETTE.len()]
}

pub fn npc_style(id: NpcId) -> Style {
    Style::default().fg(npc_color(id))
}

pub fn npc_style_bold(id: NpcId) -> Style {
    Style::default()
        .fg(npc_color(id))
        .add_modifier(Modifier::BOLD)
}

/// The player's voice / "you". Distinct from any NPC colour so the player
/// never collides with an NPC in transcripts or presence rows.
pub fn player() -> Style {
    Style::default()
        .fg(Color::LightGreen)
        .add_modifier(Modifier::BOLD)
}

/// Location names, scene headers, place-of-the-world metadata.
pub fn location() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// Day / time-of-day strip — present but quiet.
pub fn time() -> Style {
    Style::default().fg(Color::Blue)
}

/// Italic banner used for day-rollover and mode prompts ("how do you mean to speak?").
pub fn banner() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::ITALIC)
}

/// Deadline style: scales from quiet → yellow → red as the clock runs out.
pub fn deadline(days_left: u32) -> Style {
    if days_left == 0 {
        Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::BOLD)
    } else if days_left <= 2 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        dim()
    }
}

/// Quiet text — hints, engine labels, secondary metadata. Stays in the user's
/// default foreground colour and just dims; this is what makes the UI feel
/// "themed" rather than painted-on.
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Bold default-colour text — used for keycaps in hint strips, etc.
pub fn key() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// The selected entry in a menu. REVERSED inverts whatever the terminal's
/// default fg/bg are, so the highlight reads correctly on any theme without
/// us picking a colour at all.
pub fn selected() -> Style {
    Style::default()
        .add_modifier(Modifier::REVERSED)
        .add_modifier(Modifier::BOLD)
}

/// Border style for soft / passive frames (narration, presence). DIM on
/// default fg keeps the frame visible without dominating.
pub fn border_soft() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Border style for the currently-focused / active frame (action menu,
/// dialogue input, accusation picker). Uses a quiet accent so it stands out
/// from the soft borders without screaming.
pub fn border_focus() -> Style {
    Style::default().fg(Color::Cyan)
}

/// Border style for danger / consequence frames (accusation confirmation).
pub fn border_warn() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

/// Mood valence → human label + style. Used for the conditional sidebar mood
/// line: only the extremes are shown.
pub fn mood_label(valence: f32) -> Option<(&'static str, Style)> {
    if valence >= 0.5 {
        Some((
            "elated",
            Style::default().fg(Color::Green),
        ))
    } else if valence >= 0.2 {
        Some((
            "warm",
            Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
        ))
    } else if valence <= -0.5 {
        Some((
            "wretched",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
    } else if valence <= -0.2 {
        Some((
            "low",
            Style::default().fg(Color::Red),
        ))
    } else {
        None
    }
}

/// A need's discomfort label + style if it has crossed a threshold. Needs
/// values rise from 0 → 1 as the discomfort grows; `Needs::default` sits at
/// 0.2, so anything ≥ 0.55 is "starting to bother you" and ≥ 0.8 is acute.
pub fn need_label(name: &'static str, level: f32) -> Option<(String, Style)> {
    if level >= 0.8 {
        Some((
            format!("very {}", name),
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ))
    } else if level >= 0.55 {
        Some((
            name.to_string(),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npc_color_is_stable_per_id() {
        let a1 = npc_color(NpcId(7));
        let a2 = npc_color(NpcId(7));
        assert_eq!(a1, a2);
    }

    #[test]
    fn npc_color_distributes_over_palette() {
        use std::collections::HashSet;
        let seen: HashSet<Color> = (1..200).map(|i| npc_color(NpcId(i))).collect();
        // All six palette slots should appear in 200 ids.
        assert_eq!(seen.len(), NPC_PALETTE.len());
    }

    #[test]
    fn mood_label_thresholds() {
        assert!(mood_label(0.0).is_none());
        assert!(mood_label(0.1).is_none());
        assert!(mood_label(0.3).is_some());
        assert!(mood_label(-0.3).is_some());
        assert!(mood_label(0.9).is_some());
    }

    #[test]
    fn need_label_thresholds() {
        assert!(need_label("hungry", 0.2).is_none());
        assert!(need_label("hungry", 0.55).is_some());
        assert!(need_label("hungry", 0.9).unwrap().0.starts_with("very"));
    }
}
