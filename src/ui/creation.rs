use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::palette;
use crate::app::{CreationState, CreationStep};
use crate::content::{ContentRegistry, TraitOption};

pub fn render(frame: &mut Frame, state: &CreationState, registry: &ContentRegistry) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(frame, chunks[0], state);
    render_body(frame, chunks[1], state, registry);
    render_footer(frame, chunks[2], state);
}

fn render_header(frame: &mut Frame, area: Rect, state: &CreationState) {
    let title = match state.step {
        CreationStep::Setting => "Choose your setting.",
        CreationStep::Virtue => "Choose your virtue.",
        CreationStep::Vice => "Choose your vice.",
        CreationStep::Inclination => "Choose your inclination.",
        CreationStep::Background => "Choose your background.",
        CreationStep::Name => "A name finds you.",
        CreationStep::Confirm => "Look at yourself.",
    };
    let p = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(title, palette::location())).alignment(Alignment::Center),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(palette::border_soft()),
    );
    frame.render_widget(p, area);
}

fn render_body(frame: &mut Frame, area: Rect, state: &CreationState, registry: &ContentRegistry) {
    let lines: Vec<Line> = match state.step {
        CreationStep::Setting => setting_lines(state),
        CreationStep::Virtue => trait_lines(&registry.setting.traits.virtues, state.selected_index, AccentKind::Virtue),
        CreationStep::Vice => trait_lines(&registry.setting.traits.vices, state.selected_index, AccentKind::Vice),
        CreationStep::Inclination => {
            trait_lines(&registry.setting.traits.inclinations, state.selected_index, AccentKind::Inclination)
        }
        CreationStep::Background => background_lines(state, registry),
        CreationStep::Name => name_lines(state, registry),
        CreationStep::Confirm => confirm_lines(state, registry),
    };

    let body = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(palette::border_soft()),
    );
    frame.render_widget(body, area);
}

#[derive(Clone, Copy)]
enum AccentKind {
    Virtue,
    Vice,
    Inclination,
}

fn accent_style(kind: AccentKind) -> Style {
    match kind {
        // Virtue green, vice red, inclination blue — a quick visual cue while
        // walking through creation. Defaults stay readable on any theme.
        AccentKind::Virtue => Style::default().fg(ratatui::style::Color::Green),
        AccentKind::Vice => Style::default().fg(ratatui::style::Color::Red),
        AccentKind::Inclination => Style::default().fg(ratatui::style::Color::Blue),
    }
}

fn setting_lines(state: &CreationState) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("")];
    if state.available_settings.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no setting packs found under content/settings/",
            palette::dim(),
        )));
        return lines;
    }
    let cyan = Style::default().fg(ratatui::style::Color::Cyan);
    for (i, name) in state.available_settings.iter().enumerate() {
        let is_sel = i == state.selected_index;
        let marker = if is_sel { "▶ " } else { "  " };
        let name_style = if is_sel { palette::selected() } else { cyan };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                marker,
                if is_sel {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    palette::dim()
                },
            ),
            Span::styled(name.clone(), name_style),
        ]));
        lines.push(Line::from(""));
    }
    lines
}

fn trait_lines(options: &[TraitOption], selected: usize, kind: AccentKind) -> Vec<Line<'_>> {
    let mut lines = vec![Line::from("")];
    for (i, opt) in options.iter().enumerate() {
        let is_sel = i == selected;
        let marker = if is_sel { "▶ " } else { "  " };
        let name_style = if is_sel {
            palette::selected()
        } else {
            accent_style(kind)
        };
        let desc_style = if is_sel { Style::default() } else { palette::dim() };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(marker, if is_sel { Style::default().add_modifier(Modifier::BOLD) } else { palette::dim() }),
            Span::styled(opt.name.clone(), name_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw("      "),
            Span::styled(opt.description.clone(), desc_style),
        ]));
        lines.push(Line::from(""));
    }
    lines
}

fn background_lines(state: &CreationState, registry: &ContentRegistry) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("")];
    for (i, bg) in registry.vocation.backgrounds.backgrounds.iter().enumerate() {
        let is_sel = i == state.selected_index;
        let marker = if is_sel { "▶ " } else { "  " };
        let name_style = if is_sel {
            palette::selected()
        } else {
            Style::default().fg(ratatui::style::Color::Cyan)
        };
        let desc_style = if is_sel { Style::default() } else { palette::dim() };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(marker, if is_sel { Style::default().add_modifier(Modifier::BOLD) } else { palette::dim() }),
            Span::styled(bg.name.clone(), name_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw("      "),
            Span::styled(bg.description.clone(), desc_style),
        ]));
        lines.push(Line::from(""));
    }
    lines
}

fn name_lines<'a>(state: &CreationState, registry: &'a ContentRegistry) -> Vec<Line<'a>> {
    let inhabitants = registry.setting.setting.inhabitant_plural.as_str();
    vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("The {} will call you:", inhabitants),
            palette::dim(),
        ))
        .alignment(Alignment::Center),
        Line::from(""),
        Line::from(Span::styled(state.rolled_name.clone(), palette::player())).alignment(Alignment::Center),
        Line::from(""),
        Line::from(vec![
            Span::styled("[ r ] ", palette::key()),
            Span::styled("reroll   ", palette::dim()),
            Span::styled("[ ↵ ] ", palette::key()),
            Span::styled("accept", palette::dim()),
        ])
        .alignment(Alignment::Center),
    ]
}

fn confirm_lines(state: &CreationState, registry: &ContentRegistry) -> Vec<Line<'static>> {
    let none = "—".to_string();
    let vocation = &registry.vocation.vocation.name;
    let row = |label: &str, value: &str, value_style: Style| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{:>14}  ", label), palette::dim()),
            Span::styled(value.to_string(), value_style),
        ])
    };
    let green = Style::default().fg(ratatui::style::Color::Green);
    let red = Style::default().fg(ratatui::style::Color::Red);
    let blue = Style::default().fg(ratatui::style::Color::Blue);
    let cyan = Style::default().fg(ratatui::style::Color::Cyan);
    vec![
        Line::from(""),
        row("Setting", &state.staged_setting, cyan),
        row("Name", &state.rolled_name, palette::player()),
        row("Vocation", vocation, cyan),
        row("Virtue", state.virtue.as_ref().unwrap_or(&none), green),
        row("Vice", state.vice.as_ref().unwrap_or(&none), red),
        row("Inclination", state.inclination.as_ref().unwrap_or(&none), blue),
        row("Background", state.background.as_ref().unwrap_or(&none), cyan),
        Line::from(""),
    ]
}

fn render_footer(frame: &mut Frame, area: Rect, state: &CreationState) {
    let pairs: &[(&str, &str)] = match state.step {
        CreationStep::Name => &[
            ("↑↓", "select"),
            ("↵", "accept"),
            ("r", "reroll"),
            ("esc", "back"),
        ],
        CreationStep::Confirm => &[("↵", "save & return to title"), ("esc", "back")],
        _ => &[("↑↓", "select"), ("↵", "confirm"), ("esc", "back")],
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
    let p = Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(palette::border_soft()),
        );
    frame.render_widget(p, area);
}
