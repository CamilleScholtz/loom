use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::palette;
use crate::app::SaveBrowserState;

pub fn render(frame: &mut Frame, state: &SaveBrowserState) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(2), // heading
            Constraint::Min(3),    // slot list
            Constraint::Length(1), // footer hint
        ])
        .split(area);

    let heading = Paragraph::new(Line::from(Span::styled(
        "continue a playthrough",
        palette::location().add_modifier(Modifier::BOLD),
    )))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(palette::border_soft()),
    );
    frame.render_widget(heading, chunks[0]);

    let body: Vec<Line> = if state.slots.is_empty() {
        vec![Line::from(Span::styled("no saves found", palette::dim()))]
    } else {
        state
            .slots
            .iter()
            .enumerate()
            .flat_map(|(i, slot)| {
                let selected = i == state.selected;
                let cursor = if selected { "> " } else { "  " };
                let cursor_style = if selected {
                    palette::selected()
                } else {
                    palette::dim()
                };
                let name_style = if selected {
                    palette::selected()
                } else {
                    palette::key()
                };
                let header = Line::from(vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(slot.slot_name.clone(), name_style),
                    Span::styled("  —  ", palette::dim()),
                    Span::styled(slot.player_name.clone(), palette::player()),
                ]);
                let deadline = if slot.deadline_day > 0 {
                    let left = slot.deadline_day.saturating_sub(slot.day);
                    format!("day {} of {} ({} left)", slot.day, slot.deadline_day, left)
                } else {
                    format!("day {}", slot.day)
                };
                let setting_label = slot
                    .setting_name
                    .clone()
                    .unwrap_or_else(|| "(unknown setting)".into());
                let meta = Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        if slot.village_name.is_empty() {
                            "—".into()
                        } else {
                            slot.village_name.clone()
                        },
                        palette::location(),
                    ),
                    Span::styled("   ", palette::dim()),
                    Span::styled(setting_label, palette::banner()),
                    Span::styled("   ", palette::dim()),
                    Span::styled(deadline, palette::dim()),
                ]);
                vec![header, meta, Line::from("")]
            })
            .collect()
    };

    frame.render_widget(Paragraph::new(body), chunks[1]);

    let hint_pairs = [("↑↓", "select"), ("↵", "load"), ("esc", "back")];
    let mut spans: Vec<Span> = Vec::new();
    for (i, (k, v)) in hint_pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", palette::dim()));
        }
        spans.push(Span::styled(format!("[{}]", k), palette::key()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*v, palette::dim()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        chunks[2],
    );
}
