use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::palette;
use crate::app::{
    App, TitleState, TITLE_ITEM_COUNT, TITLE_ROW_CONFIG, TITLE_ROW_CONTINUE, TITLE_ROW_NEW_GAME,
    TITLE_ROW_QUIT,
};

pub fn render(frame: &mut Frame, state: &TitleState, _app: &App) {
    let area = frame.area();

    // Vertically center a fixed-height menu inside the terminal.
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(16),
            Constraint::Min(0),
        ])
        .split(area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(palette::border_soft());
    let inner = block.inner(layout[1]);
    frame.render_widget(block, layout[1]);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // tagline
            Constraint::Length(1), // spacer
            Constraint::Length(TITLE_ITEM_COUNT as u16),
            Constraint::Min(0), // spacer
            Constraint::Length(1), // footer
        ])
        .split(inner);

    let title = Paragraph::new(Line::from(Span::styled(
        "B O O K",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(title, rows[0]);

    let tagline = Paragraph::new(Line::from(Span::styled(
        "a system-driven narrative",
        palette::banner(),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(tagline, rows[1]);

    render_menu(frame, rows[3], state);

    let hint_pairs = [("↑↓", "select"), ("↵", "activate"), ("q", "quit")];
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
        rows[5],
    );
}

fn render_menu(frame: &mut Frame, area: Rect, state: &TitleState) {
    let mut lines: Vec<Line> = Vec::with_capacity(TITLE_ITEM_COUNT);
    for row in 0..TITLE_ITEM_COUNT {
        lines.push(menu_line(row, state));
    }
    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

fn menu_line(row: usize, state: &TitleState) -> Line<'static> {
    let selected = row == state.selected;
    let cursor = if selected { ">  " } else { "   " };

    let (label, suffix, enabled) = match row {
        TITLE_ROW_NEW_GAME => ("new game".to_string(), String::new(), true),
        TITLE_ROW_CONTINUE => {
            let saves = state.saves_count;
            let enabled = saves > 0;
            let suffix = if saves == 0 {
                "  (no saves)".to_string()
            } else if saves == 1 {
                "  (1 save)".to_string()
            } else {
                format!("  ({} saves)", saves)
            };
            ("continue".to_string(), suffix, enabled)
        }
        TITLE_ROW_CONFIG => ("edit config".to_string(), String::new(), true),
        TITLE_ROW_QUIT => ("quit".to_string(), String::new(), true),
        _ => ("?".to_string(), String::new(), false),
    };

    let label_style = if !enabled {
        palette::dim()
    } else if selected {
        palette::selected()
    } else {
        palette::key()
    };
    let cursor_style = if selected {
        palette::selected()
    } else {
        palette::dim()
    };
    let suffix_style = palette::dim();

    Line::from(vec![
        Span::styled(cursor, cursor_style),
        Span::styled(label, label_style),
        Span::styled(suffix, suffix_style),
    ])
}
