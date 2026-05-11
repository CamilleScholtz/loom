use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::palette;
use crate::app::EpilogueState;

pub fn render(frame: &mut Frame, state: &EpilogueState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let heading = Paragraph::new(Line::from(Span::styled(
        "epilogue",
        palette::location(),
    )))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(palette::border_soft()),
    );
    frame.render_widget(heading, chunks[0]);

    let outcome_style = outcome_style_for(&state.outcome_label);
    let outcome = Paragraph::new(Line::from(vec![
        Span::styled("outcome   ", palette::dim()),
        Span::styled(state.outcome_label.clone(), outcome_style),
    ]));
    frame.render_widget(outcome, chunks[1]);

    let body = Paragraph::new(state.body.clone())
        .wrap(Wrap { trim: false })
        .scroll((state.scroll, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(palette::border_soft()),
        );
    frame.render_widget(body, chunks[2]);

    let pairs = [
        ("j/k", "scroll"),
        ("PgUp/PgDn", "page"),
        ("n", "new game"),
        ("q", "quit"),
    ];
    let mut spans: Vec<Span> = Vec::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", palette::dim()));
        }
        spans.push(Span::styled(format!("[{}]", k), palette::key()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*v, palette::dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[3]);
}

fn outcome_style_for(label: &str) -> Style {
    let lower = label.to_ascii_lowercase();
    if lower.contains("solved") && !lower.contains("mis") {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else if lower.contains("misaccused") || lower.contains("died") {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    }
}
