use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::palette;
use crate::app::{ActionEntry, App, SceneMode, SceneState};
use crate::engine::Action;
use crate::world::{LocationId, NpcId};

/// Minimum terminal width at which we render the two-column layout. Below
/// this we collapse to the original vertical stack so narrow terminals still
/// work.
const WIDE_THRESHOLD: u16 = 100;
const SIDEBAR_WIDTH: u16 = 26;

pub fn render(frame: &mut Frame, app: &App, state: &SceneState) {
    let area = frame.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // header strip
            Constraint::Min(8),    // body
            Constraint::Length(1), // controls
        ])
        .split(area);

    render_header(frame, outer[0], app, state);

    if area.width >= WIDE_THRESHOLD {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(40),
                Constraint::Length(SIDEBAR_WIDTH),
            ])
            .split(outer[1]);
        render_main_column(frame, body[0], app, state);
        render_sidebar(frame, body[1], app, state);
    } else {
        render_main_column_narrow(frame, outer[1], app, state);
    }

    render_controls(frame, outer[2], state);
}

// ---- header -----------------------------------------------------------------

fn render_header(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    let (location, day, clock, deadline_day) = match app.session.as_ref() {
        Some(s) => (
            s.world
                .location(state.here)
                .map(|l| l.name.clone())
                .unwrap_or_default(),
            s.world.day,
            s.world.clock_minutes,
            s.world.deadline_day,
        ),
        None => (String::new(), 0, 0, 0),
    };
    let time_word = bucket_time(clock);
    let banner = state.day_banner.clone().unwrap_or_default();
    let days_left = deadline_day.saturating_sub(day);

    let mut spans: Vec<Span> = vec![
        Span::styled(location, palette::location()),
        Span::raw("   "),
        Span::styled(format!("day {}", day), palette::time()),
        Span::raw(" · "),
        Span::styled(time_word.to_string(), palette::time()),
        Span::raw("   "),
        Span::styled(
            format!("{} of 4 left", state.events_remaining_today),
            palette::dim(),
        ),
        Span::raw("   "),
    ];
    if days_left == 0 {
        spans.push(Span::styled("DEADLINE", palette::deadline(0)));
    } else {
        spans.push(Span::styled(
            format!("deadline in {} d", days_left),
            palette::deadline(days_left),
        ));
    }

    let line1 = Line::from(spans);
    let line2 = Line::from(Span::styled(banner, palette::banner()));
    let p = Paragraph::new(vec![line1, line2]);
    frame.render_widget(p, area);
}

// ---- main column (wide) -----------------------------------------------------

fn render_main_column(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),  // narration
            Constraint::Min(8),  // lower (actions/dialogue)
        ])
        .split(area);
    render_narration(frame, chunks[0], state);
    render_lower(frame, chunks[1], app, state);
}

// ---- main column (narrow fallback) ------------------------------------------

fn render_main_column_narrow(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // narration
            Constraint::Length(3), // presence row
            Constraint::Min(6),    // lower
        ])
        .split(area);
    render_narration(frame, chunks[0], state);
    render_presence_inline(frame, chunks[1], app, state);
    render_lower(frame, chunks[2], app, state);
}

fn render_narration(frame: &mut Frame, area: Rect, state: &SceneState) {
    let p = Paragraph::new(state.narration.clone())
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" the scene ", palette::dim()))
                .border_style(palette::border_soft()),
        );
    frame.render_widget(p, area);
}

fn render_presence_inline(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    let session = match app.session.as_ref() {
        Some(s) => s,
        None => return,
    };
    let mut spans: Vec<Span> = vec![Span::styled("present  ", palette::dim())];
    if state.present.is_empty() {
        spans.push(Span::styled(
            "(no one else is here)",
            palette::dim(),
        ));
    } else {
        for (i, id) in state.present.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(", ", palette::dim()));
            }
            let name = session
                .world
                .npc(*id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| format!("#{}", id.0));
            spans.push(Span::styled(name, palette::npc_style(*id)));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true }), area);
}

// ---- right sidebar ----------------------------------------------------------

fn render_sidebar(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7), // you panel
            Constraint::Min(4),    // here now
            Constraint::Min(3),    // ways out
        ])
        .split(area);
    render_you_panel(frame, chunks[0], app, state);
    render_here_now(frame, chunks[1], app, state);
    render_ways_out(frame, chunks[2], app, state);
}

fn render_you_panel(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(session) = app.session.as_ref() {
        let player = session.world.npc(NpcId(0));
        let name = player
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "you".into());
        lines.push(Line::from(Span::styled(name, palette::player())));

        // Conditional mood line — only when valence crosses a threshold.
        if let Some(mood) = player.and_then(|p| palette::mood_label(p.mood.valence)) {
            lines.push(Line::from(vec![
                Span::styled("mood  ", palette::dim()),
                Span::styled(mood.0, mood.1),
            ]));
        }

        // Conditional needs — only show needs that are starting to bother.
        if let Some(p) = player {
            for label in needs_labels(p) {
                lines.push(label);
            }
        }
    } else {
        lines.push(Line::from(Span::styled("you", palette::player())));
    }

    lines.push(Line::from(vec![
        Span::styled("events    ", palette::dim()),
        Span::styled(
            format!("{} / 4", state.events_remaining_today),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));

    let days_left = app
        .session
        .as_ref()
        .map(|s| s.world.deadline_day.saturating_sub(s.world.day))
        .unwrap_or(0);
    lines.push(Line::from(vec![
        Span::styled("deadline  ", palette::dim()),
        Span::styled(format!("{} d", days_left), palette::deadline(days_left)),
    ]));

    let p = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" you ", palette::dim()))
            .border_style(palette::border_soft()),
    );
    frame.render_widget(p, area);
}

fn needs_labels(p: &crate::world::Npc) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = Vec::new();
    let entries: [(&'static str, f32); 4] = [
        ("hungry", p.needs.hunger),
        ("weary", p.needs.sleep),
        ("alone", p.needs.belonging),
        ("aimless", p.needs.purpose),
    ];
    for (name, level) in entries {
        if let Some((label, style)) = palette::need_label(name, level) {
            out.push(Line::from(vec![
                Span::styled("          ", palette::dim()),
                Span::styled(label, style),
            ]));
        }
    }
    out
}

fn render_here_now(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    let session = match app.session.as_ref() {
        Some(s) => s,
        None => return,
    };
    let mut lines: Vec<Line> = Vec::new();
    if state.present.is_empty() {
        lines.push(Line::from(Span::styled(
            "no one else",
            palette::dim(),
        )));
    } else {
        for id in &state.present {
            let npc = session.world.npc(*id);
            let name = npc
                .map(|n| n.name.clone())
                .unwrap_or_else(|| format!("#{}", id.0));
            let occ = npc
                .and_then(|n| n.occupation.clone())
                .map(|o| format!("  {}", o));
            let mut spans = vec![Span::styled(name, palette::npc_style_bold(*id))];
            if let Some(o) = occ {
                spans.push(Span::styled(o, palette::dim()));
            }
            lines.push(Line::from(spans));
        }
    }
    let p = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" here now ", palette::dim()))
            .border_style(palette::border_soft()),
    );
    frame.render_widget(p, area);
}

fn render_ways_out(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    let session = match app.session.as_ref() {
        Some(s) => s,
        None => return,
    };
    let adj: Vec<LocationId> = session
        .world
        .location(state.here)
        .map(|l| l.adjacent.clone())
        .unwrap_or_default();
    let mut lines: Vec<Line> = Vec::new();
    if adj.is_empty() {
        lines.push(Line::from(Span::styled("nowhere", palette::dim())));
    } else {
        for id in adj {
            let name = session
                .world
                .location(id)
                .map(|l| l.name.clone())
                .unwrap_or_else(|| format!("#{}", id.0));
            lines.push(Line::from(Span::styled(name, palette::location())));
        }
    }
    let p = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" ways out ", palette::dim()))
            .border_style(palette::border_soft()),
    );
    frame.render_widget(p, area);
}

// ---- lower body (actions / dialogue / accuse) ------------------------------

fn render_lower(frame: &mut Frame, area: Rect, app: &App, state: &SceneState) {
    match &state.mode {
        SceneMode::Browsing => render_actions(frame, area, state),
        SceneMode::DialogueLine { npc, buffer } => {
            render_dialogue_chatbox(frame, area, app, *npc, ChatTail::Input(buffer))
        }
        SceneMode::DialogueStreaming {
            npc, npc_name, buffer, revealed, ..
        } => {
            let cap = (*revealed).min(buffer.len());
            let visible = &buffer[..cap];
            render_dialogue_chatbox(
                frame,
                area,
                app,
                *npc,
                ChatTail::Streaming { npc_name, buffer: visible },
            )
        }
        SceneMode::DialogueReply { npc, .. } => {
            render_dialogue_chatbox(frame, area, app, *npc, ChatTail::AwaitContinue)
        }
        SceneMode::Accuse { targets, selected } => {
            render_accuse(frame, area, app, targets, *selected)
        }
    }
}

fn render_actions(frame: &mut Frame, area: Rect, state: &SceneState) {
    let mut lines: Vec<Line> = Vec::new();
    if state.actions.is_empty() {
        lines.push(Line::from(Span::styled(
            "nothing here calls to you",
            palette::dim(),
        )));
    } else {
        for (i, entry) in state.actions.iter().enumerate() {
            lines.push(action_line(entry, i == state.selected));
        }
    }
    let p = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" what you do ", palette::dim()))
            .border_style(palette::border_focus()),
    );
    frame.render_widget(p, area);
}

fn action_line(entry: &ActionEntry, selected: bool) -> Line<'_> {
    let marker = if selected { "▶ " } else { "  " };
    let text = entry
        .flavored
        .clone()
        .unwrap_or_else(|| entry.engine_label.clone());

    // Per-action accent: interview takes the target NPC's colour; move uses
    // location cyan. Keeps the list scannable.
    let accent = match &entry.action {
        Action::Interview(id) => Some(palette::npc_style(*id)),
        Action::Move(_) => Some(palette::location()),
        Action::Accuse => Some(palette::deadline(2)),
        Action::LeaveTown => Some(palette::deadline(2)),
        _ => None,
    };

    let text_style = if selected {
        palette::selected()
    } else if let Some(s) = accent {
        s
    } else {
        Style::default()
    };
    let marker_style = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        palette::dim()
    };

    Line::from(vec![
        Span::styled(marker, marker_style),
        Span::styled(text, text_style),
        Span::raw("  "),
        Span::styled(format!("[{}]", entry.engine_label), palette::dim()),
    ])
}

enum ChatTail<'a> {
    /// The player is typing — show "> buffer▌" beneath the transcript.
    Input(&'a str),
    /// The model is streaming the NPC's reply — show it live with a cursor,
    /// or a "(thinking…)" placeholder before the first visible char arrives.
    Streaming { npc_name: &'a str, buffer: &'a str },
    /// The reply is finalized and already in history; the player can press ↵
    /// to continue or esc to step away. No tail content needed beyond a hint.
    AwaitContinue,
}

fn render_dialogue_chatbox(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    npc: NpcId,
    tail: ChatTail<'_>,
) {
    let npc_name = app
        .session
        .as_ref()
        .and_then(|s| s.world.npc(npc).map(|n| n.name.clone()))
        .unwrap_or_else(|| "them".into());

    let history: Vec<(bool, String)> = app
        .session
        .as_ref()
        .and_then(|s| s.dialogue_history.get(&npc).cloned())
        .unwrap_or_default();

    let title = format!(" talk to {} ", npc_name.trim());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title, palette::dim()))
        .border_style(palette::border_focus());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::with_capacity(history.len() * 2 + 4);
    for (is_player, text) in &history {
        if *is_player {
            lines.push(Line::from(vec![
                Span::styled("you  ", palette::player()),
                Span::styled(text.clone(), palette::player()),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(format!("{}  ", npc_name.trim()), palette::npc_style_bold(npc)),
                Span::styled(text.clone(), palette::npc_style(npc)),
            ]));
        }
    }

    match tail {
        ChatTail::Input(buffer) => {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(vec![
                Span::styled("> ", palette::dim()),
                Span::styled(buffer.to_string(), palette::player()),
                Span::styled(
                    "▌",
                    palette::player().add_modifier(Modifier::SLOW_BLINK),
                ),
            ]));
        }
        ChatTail::Streaming { npc_name: nm, buffer } => {
            if buffer.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}  ", nm.trim()), palette::npc_style_bold(npc)),
                    Span::styled(
                        "(thinking…)",
                        palette::npc_style(npc).add_modifier(Modifier::SLOW_BLINK),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}  ", nm.trim()), palette::npc_style_bold(npc)),
                    Span::styled(buffer.to_string(), palette::npc_style(npc)),
                    Span::styled(
                        "▌",
                        palette::npc_style(npc).add_modifier(Modifier::SLOW_BLINK),
                    ),
                ]));
            }
        }
        ChatTail::AwaitContinue => { /* hint shown in the controls strip */ }
    }

    // Stick to the bottom: estimate how many wrapped terminal rows the lines
    // occupy and scroll past the overflow so the most recent content is
    // always visible.
    let width = inner.width.max(1);
    let total_rows: usize = lines
        .iter()
        .map(|l| line_rows(l, width as usize))
        .sum();
    let height = inner.height as usize;
    let scroll: u16 = total_rows.saturating_sub(height).min(u16::MAX as usize) as u16;

    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(p, inner);
}

fn line_rows(line: &Line<'_>, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let text_len: usize = line
        .spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum();
    if text_len == 0 {
        return 1;
    }
    text_len.div_ceil(width)
}

fn render_accuse(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    targets: &[NpcId],
    selected: usize,
) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "name the culprit — this closes the case",
        palette::border_warn().add_modifier(Modifier::ITALIC),
    )));
    lines.push(Line::from(""));
    if targets.is_empty() {
        lines.push(Line::from(Span::styled(
            "no one alive to accuse",
            palette::dim(),
        )));
    } else {
        for (i, id) in targets.iter().enumerate() {
            let name = app
                .session
                .as_ref()
                .and_then(|s| s.world.npc(*id).map(|n| n.name.clone()))
                .unwrap_or_else(|| format!("#{}", id.0));
            let occ = app
                .session
                .as_ref()
                .and_then(|s| s.world.npc(*id).and_then(|n| n.occupation.clone()));
            let marker = if i == selected { "▶ " } else { "  " };
            let name_style = if i == selected {
                palette::selected()
            } else {
                palette::npc_style(*id)
            };
            let mut spans = vec![
                Span::styled(marker, palette::dim()),
                Span::styled(name, name_style),
            ];
            if let Some(o) = occ {
                spans.push(Span::styled(format!("  {}", o), palette::dim()));
            }
            lines.push(Line::from(spans));
        }
    }
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" accuse ", palette::border_warn()))
            .border_style(palette::border_warn()),
    );
    frame.render_widget(p, area);
}

// ---- controls strip ---------------------------------------------------------

fn render_controls(frame: &mut Frame, area: Rect, state: &SceneState) {
    let pairs: &[(&str, &str)] = match state.mode {
        SceneMode::Browsing => &[
            ("j/k", "move"),
            ("↵", "choose"),
            ("q", "quit"),
        ],
        SceneMode::DialogueLine { .. } => &[
            ("type", "your line"),
            ("↵", "speak"),
            ("⌫", "erase"),
            ("esc", "back"),
        ],
        SceneMode::DialogueStreaming { .. } => &[("…", "listening")],
        SceneMode::DialogueReply { .. } => &[
            ("↵", "say more"),
            ("esc", "back"),
        ],
        SceneMode::Accuse { .. } => &[
            ("j/k", "move"),
            ("↵", "accuse"),
            ("esc", "back"),
        ],
    };
    let mut spans: Vec<Span> = Vec::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", palette::dim()));
        }
        spans.push(Span::styled(format!("[{}]", k), palette::key()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*v, palette::dim()));
    }
    let p = Paragraph::new(Line::from(spans));
    frame.render_widget(p, area);
}

// ---- helpers ----------------------------------------------------------------

fn bucket_time(minutes: u32) -> &'static str {
    let h = minutes / 60;
    match h {
        4..=10 => "morning",
        11..=13 => "noon",
        14..=17 => "afternoon",
        18..=21 => "evening",
        _ => "night",
    }
}

