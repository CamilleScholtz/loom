use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::palette;
use crate::app::ReadyState;

pub fn render(frame: &mut Frame, state: &ReadyState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(2), // heading
            Constraint::Length(3), // village + deadline
            Constraint::Min(5),    // opening hook
            Constraint::Length(3), // incident summary
            Constraint::Length(1), // controls
        ])
        .split(area);

    let heading = Paragraph::new(Line::from(Span::styled(
        "the world is ready",
        palette::location().add_modifier(Modifier::BOLD),
    )))
    .block(Block::default().borders(Borders::BOTTOM).border_style(palette::border_soft()));
    frame.render_widget(heading, chunks[0]);

    let village = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                format!("{:<10}", state.place_kind),
                palette::dim(),
            ),
            Span::styled(state.village_name.clone(), palette::location()),
        ]),
        Line::from(vec![
            Span::styled("deadline  ", palette::dim()),
            Span::styled(
                format!("{} days", state.deadline_days),
                palette::deadline(state.deadline_days),
            ),
        ]),
    ]);
    frame.render_widget(village, chunks[1]);

    let hook = Paragraph::new(state.player_hook.clone())
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" opening ", palette::dim()))
                .border_style(palette::border_focus()),
        );
    frame.render_widget(hook, chunks[2]);

    let case = Paragraph::new(Line::from(vec![
        Span::styled("case      ", palette::dim()),
        Span::styled(state.incident_summary.clone(), palette::banner()),
    ]))
    .wrap(Wrap { trim: true });
    frame.render_widget(case, chunks[3]);

    let ctrl_pairs = [("↵", "begin"), ("n", "new game"), ("q", "quit")];
    let mut spans: Vec<Span> = Vec::new();
    for (i, (k, v)) in ctrl_pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", palette::dim()));
        }
        spans.push(Span::styled(format!("[{}]", k), palette::key()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*v, palette::dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[4]);
}
