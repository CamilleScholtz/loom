use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::palette;
use crate::app::NotebookState;

pub fn render(frame: &mut Frame, state: &NotebookState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let heading = Paragraph::new(Line::from(Span::styled(
        "notebook",
        palette::location(),
    )))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(palette::border_soft()),
    );
    frame.render_widget(heading, chunks[0]);

    let lines = if state.body.trim().is_empty() {
        vec![Line::from(Span::styled(
            "(empty — observe and speak with people to fill these pages)",
            palette::dim(),
        ))]
    } else {
        render_markdown(&state.body)
    };

    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((state.scroll, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(palette::border_soft()),
        );
    frame.render_widget(p, chunks[1]);

    let pairs = [("j/k", "scroll"), ("PgUp/PgDn", "page"), ("esc/q", "back")];
    let mut spans: Vec<Span> = Vec::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", palette::dim()));
        }
        spans.push(Span::styled(format!("[{}]", k), palette::key()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*v, palette::dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
}

/// Lightweight markdown → ratatui lines, just enough for the notebook's
/// authored shape: `## Day N` headers, `- ` bullets, `**bold**` spans.
/// We don't pull in a full renderer because the notebook is appended in a
/// constrained format we control.
fn render_markdown(body: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = Vec::new();
    for raw in body.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.strip_prefix("## ") {
            out.push(Line::from(Span::styled(
                rest.to_string(),
                palette::location().add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            out.push(Line::from(Span::styled(
                rest.to_string(),
                palette::location().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            continue;
        }
        let (indent, body_part) = if let Some(rest) = line.strip_prefix("- ") {
            ("  • ", rest)
        } else if let Some(rest) = line.strip_prefix("  - ") {
            ("      ◦ ", rest)
        } else {
            ("", line)
        };
        let mut spans: Vec<Span> = Vec::new();
        if !indent.is_empty() {
            spans.push(Span::styled(indent.to_string(), palette::dim()));
        }
        spans.extend(render_inline(body_part));
        out.push(Line::from(spans));
    }
    out
}

fn render_inline(text: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("**") {
        if start > 0 {
            spans.push(Span::raw(rest[..start].to_string()));
        }
        let after = &rest[start + 2..];
        if let Some(end) = after.find("**") {
            spans.push(Span::styled(
                after[..end].to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            rest = &after[end + 2..];
        } else {
            // Unterminated — emit as plain.
            spans.push(Span::raw(format!("**{}", after)));
            rest = "";
            break;
        }
    }
    if !rest.is_empty() {
        spans.push(Span::raw(rest.to_string()));
    }
    spans
}
