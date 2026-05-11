use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::palette;
use crate::app::{ConfigField, ConfigState};

pub fn render(frame: &mut Frame, state: &ConfigState) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(2), // heading
            Constraint::Length(1), // config path
            Constraint::Length(1), // spacer
            Constraint::Min(3),    // fields
            Constraint::Length(1), // status line
            Constraint::Length(1), // footer
        ])
        .split(area);

    let heading = Paragraph::new(Line::from(Span::styled(
        "edit config",
        palette::location().add_modifier(Modifier::BOLD),
    )))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(palette::border_soft()),
    );
    frame.render_widget(heading, chunks[0]);

    let path_line = Paragraph::new(Line::from(vec![
        Span::styled("file  ", palette::dim()),
        Span::styled(state.config_path.display().to_string(), palette::dim()),
    ]));
    frame.render_widget(path_line, chunks[1]);

    render_fields(frame, chunks[3], state);

    let status_line = match &state.status {
        Some(msg) => Line::from(Span::styled(msg.clone(), palette::banner())),
        None => Line::from(""),
    };
    frame.render_widget(Paragraph::new(status_line), chunks[4]);

    let hint_pairs = [
        ("↑↓ / tab", "field"),
        ("type", "edit"),
        ("⌫", "delete"),
        ("esc", "save & back"),
    ];
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
        chunks[5],
    );
}

fn render_fields(frame: &mut Frame, area: Rect, state: &ConfigState) {
    // Two-column layout: a fixed-width label column on the left, value on the
    // right. Width 22 covers the longest label ("models.scene_narration") with
    // one space of padding.
    const LABEL_WIDTH: usize = 24;

    let mut lines: Vec<Line> = Vec::with_capacity(state.fields.len());
    for (i, field) in state.fields.iter().enumerate() {
        let selected = i == state.selected;
        let cursor = if selected { "> " } else { "  " };
        let cursor_style = if selected {
            palette::selected()
        } else {
            palette::dim()
        };
        let label_style = if selected {
            palette::key()
        } else {
            palette::dim()
        };
        let value_style = if selected {
            palette::selected()
        } else {
            palette::key()
        };

        let label = pad_label(field.label(), LABEL_WIDTH);
        let raw = state.drafts.get(i).cloned().unwrap_or_default();
        let masked = match field {
            ConfigField::ApiKey => mask_secret(&raw),
            _ => raw.clone(),
        };
        let display: String = if masked.is_empty() && !selected {
            "(default)".to_string()
        } else if selected {
            // Render a trailing block as a poor-man's caret so the user knows
            // which row the keystrokes are going into.
            format!("{}█", masked)
        } else {
            masked
        };

        lines.push(Line::from(vec![
            Span::styled(cursor, cursor_style),
            Span::styled(label, label_style),
            Span::styled(display, value_style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn pad_label(label: &str, width: usize) -> String {
    if label.len() >= width {
        format!("{} ", label)
    } else {
        format!("{:<width$}", label, width = width)
    }
}

/// Show the last 4 chars of an api key and mask the rest, so the user can
/// confirm they typed the right one without leaking the full token to
/// shoulder-surfers.
fn mask_secret(raw: &str) -> String {
    let n = raw.chars().count();
    if n == 0 {
        return String::new();
    }
    if n <= 4 {
        return "*".repeat(n);
    }
    let tail: String = raw.chars().skip(n - 4).collect();
    format!("{}{}", "*".repeat(n - 4), tail)
}
