use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use crate::ui::app::{App, InputMode, PlaybackState};

pub fn render_ui(f: &mut Frame, app: &App) {
    let mut constraints = vec![
        Constraint::Length(1),  // Header
        Constraint::Min(0),     // Results
    ];

    // Add status bar if playing
    if !matches!(app.playback_state, PlaybackState::Idle) {
        constraints.push(Constraint::Length(1));
    }

    constraints.push(Constraint::Length(2)); // Footer

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.size());

    let mut chunk_idx = 0;
    render_header(f, app, chunks[chunk_idx]);
    chunk_idx += 1;

    render_results(f, app, chunks[chunk_idx]);
    chunk_idx += 1;

    if !matches!(app.playback_state, PlaybackState::Idle) {
        render_status_bar(f, app, chunks[chunk_idx]);
        chunk_idx += 1;
    }

    render_footer(f, app, chunks[chunk_idx]);

    // Overlay help if in help mode
    if app.input_mode == InputMode::Help {
        render_help_overlay(f, app);
    }
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let header_text = format!("yt-search-play │ Query: \"{}\"", app.query);
    let header = Paragraph::new(header_text)
        .style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(header, area);
}

fn render_results(f: &mut Frame, app: &App, area: Rect) {
    let results = app.current_page_results();
    let start_idx = app.page * app.page_size;

    let items: Vec<ListItem> = results
        .iter()
        .enumerate()
        .map(|(i, result)| {
            let num = start_idx + i + 1;
            let title_line = Line::from(vec![
                Span::raw(format!("{:>3}. ", num)),
                Span::styled(&result.title, Style::default().add_modifier(Modifier::BOLD)),
            ]);

            let meta_line = Line::from(format!(
                "     ⏱ {} │ 📺 {} │ 👁 {}",
                result.duration, result.channel, result.views
            ));

            let style = if i == app.selected_index {
                Style::default().bg(Color::Yellow).fg(Color::Black)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(vec![title_line, meta_line]).style(style)
        })
        .collect();

    let page_info = if app.exhausted {
        let total_pages = (app.total_results + app.page_size - 1) / app.page_size;
        format!("Page {}/{} • {} total", app.page + 1, total_pages, app.total_results)
    } else {
        format!("Page {} • {}+ loaded", app.page + 1, app.total_results)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Search Results ({}) ", page_info))
        .border_style(Style::default().fg(Color::DarkGray));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

    let mut state = ListState::default();
    state.select(Some(app.selected_index));

    f.render_stateful_widget(list, area, &mut state);
}

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let text = match &app.playback_state {
        PlaybackState::Idle => String::new(),
        PlaybackState::Playing { title, elapsed, duration } => {
            let progress = if *duration > 0 {
                (*elapsed as f64 / *duration as f64 * 100.0) as u64
            } else {
                0
            };
            let elapsed_str = format_duration(*elapsed);
            let duration_str = format_duration(*duration);
            format!(
                " ▶ Playing: \"{}\" [{}/{}] {}%",
                title, elapsed_str, duration_str, progress
            )
        }
    };

    let status = Paragraph::new(text).style(Style::default().bg(Color::Green).fg(Color::Black));
    f.render_widget(status, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let text = match app.input_mode {
        InputMode::Browse => {
            if !app.number_input.is_empty() {
                format!("Select video: {}_", app.number_input)
            } else {
                "↑/↓: Navigate • Enter: Play • 1-9: Quick pick • s: Search • n/p: Next/Prev • h: Help • q: Quit".to_string()
            }
        }
        InputMode::Search => {
            format!("Search: {}_ (Enter: Submit • Esc: Cancel)", app.search_input)
        }
        InputMode::Help => "Press h or Esc to close help".to_string(),
    };

    let footer = Paragraph::new(text).style(Style::default().fg(Color::Cyan));
    f.render_widget(footer, area);
}

fn render_help_overlay(f: &mut Frame, _app: &App) {
    let area = centered_rect(60, 50, f.size());

    let help_text = vec![
        Line::from(Span::styled("Keyboard Controls", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("Navigation:"),
        Line::from("  ↑/↓         - Move selection up/down"),
        Line::from("  Enter       - Play selected video"),
        Line::from("  n/p         - Next/Previous page"),
        Line::from(""),
        Line::from("Commands:"),
        Line::from("  s           - New search"),
        Line::from("  h           - Toggle this help"),
        Line::from("  q/Esc       - Quit"),
        Line::from("  Ctrl+C      - Force quit"),
        Line::from(""),
        Line::from("During Playback:"),
        Line::from("  Space       - Play/Pause (in player)"),
        Line::from("  q           - Quit playback (in player)"),
        Line::from("  m           - Mute (in player)"),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    let paragraph = Paragraph::new(help_text).block(block);

    f.render_widget(paragraph, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn format_duration(seconds: u64) -> String {
    let hrs = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hrs > 0 {
        format!("{}:{:02}:{:02}", hrs, mins, secs)
    } else {
        format!("{}:{:02}", mins, secs)
    }
}
