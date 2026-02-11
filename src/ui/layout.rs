use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use crate::ui::app::{App, FocusedPanel, InputMode, SettingsField};

pub fn render_ui(f: &mut Frame, app: &App) {
    let mut constraints = vec![
        Constraint::Length(3),  // Search bar (with border)
        Constraint::Min(0),     // Main content (results + queue)
    ];

    // Add status bar if playing
    if app.player_manager.is_some() {
        constraints.push(Constraint::Length(3)); // Status bar + controls
    } else {
        constraints.push(Constraint::Length(2)); // Just controls
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.size());

    let mut chunk_idx = 0;
    render_search_bar(f, app, chunks[chunk_idx]);
    chunk_idx += 1;

    render_main_content(f, app, chunks[chunk_idx]);
    chunk_idx += 1;

    render_footer(f, app, chunks[chunk_idx]);

    // Overlay help if in help mode
    if app.input_mode == InputMode::Help {
        render_help_overlay(f, app);
    }

    // Overlay settings if in settings mode
    if app.settings_open {
        render_settings_modal(f, app);
    }
}

fn render_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::SearchBar;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let display_text = if is_focused {
        if app.search_input.is_empty() {
            "█".to_string()
        } else {
            format!("{}█", app.search_input)
        }
    } else {
        app.query.clone()
    };

    let search_bar = Paragraph::new(display_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Search ")
                .border_style(border_style)
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(search_bar, area);
}

fn render_main_content(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(70),
            Constraint::Percentage(30),
        ])
        .split(area);

    render_results(f, app, chunks[0]);
    render_queue_panel(f, app, chunks[1]);
}

fn render_results(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::Results;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Show loading indicator if searching
    if app.loading {
        let loading_items = vec![
            ListItem::new(""),
            ListItem::new(""),
            ListItem::new(Line::from(vec![
                Span::styled("  Searching for: ", Style::default().fg(Color::Cyan)),
                Span::styled(&app.query, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ])),
            ListItem::new(""),
            ListItem::new(Line::from(
                Span::styled("  Loading results...", Style::default().fg(Color::DarkGray))
            )),
        ];

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Search Results ")
            .border_style(border_style);

        let list = List::new(loading_items).block(block);
        f.render_widget(list, area);
        return;
    }

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
                "     {} | {} | {}",
                result.duration, result.channel, result.views
            ));

            let style = if i == app.selected_index && is_focused {
                Style::default().bg(Color::Yellow).fg(Color::Black)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(vec![title_line, meta_line]).style(style)
        })
        .collect();

    let page_info = if app.exhausted {
        let total_pages = (app.total_results + app.page_size - 1) / app.page_size;
        format!("Page {}/{} | {} total", app.page + 1, total_pages, app.total_results)
    } else {
        format!("Page {} | {}+ loaded", app.page + 1, app.total_results)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Results ({}) ", page_info))
        .border_style(border_style);

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

    let mut state = ListState::default();
    state.select(Some(app.selected_index));

    f.render_stateful_widget(list, area, &mut state);
}

fn render_queue_panel(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::Queue;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let items: Vec<ListItem> = if app.queue.is_empty() {
        vec![
            ListItem::new("Queue is empty").style(Style::default().fg(Color::DarkGray)),
            ListItem::new(""),
            ListItem::new("Press Enter on results").style(Style::default().fg(Color::DarkGray)),
            ListItem::new("to add tracks").style(Style::default().fg(Color::DarkGray)),
        ]
    } else {
        app.queue.iter().enumerate().map(|(i, track)| {
            let prefix = if i == 0 && app.player_manager.is_some() {
                "▶ "
            } else {
                "  "
            };
            let line = Line::from(format!("{}{}", prefix, track.title));

            let style = if i == app.queue_selected_index && is_focused {
                Style::default().bg(Color::Yellow).fg(Color::Black)
            } else if i == 0 && app.player_manager.is_some() {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(line).style(style)
        }).collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Queue ")
                .border_style(border_style)
        );

    f.render_widget(list, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let has_playback = app.player_manager.is_some();

    if has_playback {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),  // Status bar
                Constraint::Length(2),  // Controls
            ])
            .split(area);

        render_status_line(f, app, chunks[0]);
        render_controls_line(f, app, chunks[1]);
    } else {
        render_controls_line(f, app, area);
    }
}

fn render_status_line(f: &mut Frame, app: &App, area: Rect) {
    if let Some(ref player) = app.player_manager {
        let status = &player.status;
        let progress_width = area.width.saturating_sub(60) as usize;
        let progress = if status.duration > 0.0 {
            (status.time_pos / status.duration * progress_width as f64) as usize
        } else {
            0
        };

        let filled = "━".repeat(progress.min(progress_width));
        let empty = "─".repeat(progress_width.saturating_sub(progress));

        let elapsed = format_duration(status.time_pos as u64);
        let duration = format_duration(status.duration as u64);

        let line = Line::from(vec![
            Span::styled(
                if status.paused { " ⏸ " } else { " ▶ " },
                Style::default().fg(Color::Green).bg(Color::Black).add_modifier(Modifier::BOLD)
            ),
            Span::styled(
                format!(" {} ", status.title),
                Style::default().fg(Color::White).bg(Color::Black).add_modifier(Modifier::BOLD)
            ),
            Span::styled(
                " │ ",
                Style::default().fg(Color::DarkGray).bg(Color::Black)
            ),
            Span::styled(
                filled,
                Style::default().fg(Color::Green).bg(Color::Black).add_modifier(Modifier::BOLD)
            ),
            Span::styled(
                empty,
                Style::default().fg(Color::DarkGray).bg(Color::Black)
            ),
            Span::styled(
                format!(" {} ", elapsed),
                Style::default().fg(Color::Cyan).bg(Color::Black).add_modifier(Modifier::BOLD)
            ),
            Span::styled(
                "/",
                Style::default().fg(Color::DarkGray).bg(Color::Black)
            ),
            Span::styled(
                format!(" {} ", duration),
                Style::default().fg(Color::Gray).bg(Color::Black)
            ),
            Span::styled(
                "│",
                Style::default().fg(Color::DarkGray).bg(Color::Black)
            ),
            Span::styled(
                format!(" 🔊 {}% ", status.volume),
                Style::default().fg(Color::Cyan).bg(Color::Black).add_modifier(Modifier::BOLD)
            ),
        ]);

        let status_bar = Paragraph::new(line)
            .style(Style::default().bg(Color::Black));

        f.render_widget(status_bar, area);
    }
}

fn render_controls_line(f: &mut Frame, app: &App, area: Rect) {
    let text = match app.focused_panel {
        FocusedPanel::SearchBar => {
            "Enter: Search | Esc: Cancel | Tab: Switch Panel | F2: Settings".to_string()
        }
        FocusedPanel::Results => {
            if !app.number_input.is_empty() {
                format!("Select: {}_ | Enter: Confirm | Bksp: Clear", app.number_input)
            } else {
                "Up/Dn: Navigate | Enter: Queue | Space: Pause | Tab: Switch | n/p: Page | s/F2: Settings | q: Quit".to_string()
            }
        }
        FocusedPanel::Queue => {
            "Up/Dn: Navigate | Enter: Jump | Del: Remove | c: Clear | Tab: Switch | F2: Settings".to_string()
        }
    };

    let footer = Paragraph::new(text)
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::TOP));

    f.render_widget(footer, area);
}

fn render_help_overlay(f: &mut Frame, _app: &App) {
    let area = centered_rect(60, 70, f.size());

    // Clear the background area first to hide content behind the modal
    f.render_widget(Clear, area);

    let help_text = vec![
        Line::from(Span::styled("Keyboard Controls", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("Focus Navigation:"),
        Line::from("  Tab         - Cycle focus (Search > Results > Queue)"),
        Line::from("  Shift+Tab   - Reverse cycle"),
        Line::from(""),
        Line::from("Search Bar (when focused):"),
        Line::from("  Type        - Enter query"),
        Line::from("  Enter       - Submit search"),
        Line::from("  Esc         - Clear and return to Results"),
        Line::from(""),
        Line::from("Results (when focused):"),
        Line::from("  Up/Dn       - Move selection"),
        Line::from("  Enter       - Add to queue and play"),
        Line::from("  1-9         - Quick-pick by number"),
        Line::from("  n/p         - Next/Previous page"),
        Line::from("  s           - Focus search bar"),
        Line::from(""),
        Line::from("Queue (when focused):"),
        Line::from("  Up/Dn       - Navigate queue"),
        Line::from("  Enter       - Jump to track"),
        Line::from("  Del/Bksp    - Remove track"),
        Line::from("  c           - Clear queue"),
        Line::from(""),
        Line::from("Playback (global):"),
        Line::from("  Space       - Play/Pause"),
        Line::from("  n           - Next track"),
        Line::from("  </>         - Seek -/+ 10 seconds"),
        Line::from("  +/-         - Volume up/down"),
        Line::from("  m           - Mute toggle"),
        Line::from(""),
        Line::from("Settings:"),
        Line::from("  s/F2        - Open settings"),
        Line::from(""),
        Line::from("Other:"),
        Line::from("  h           - Toggle this help"),
        Line::from("  q/Esc       - Quit"),
        Line::from("  Ctrl+C      - Force quit"),
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

fn render_settings_modal(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 70, f.size());

    // Clear the background area first to hide content behind the modal
    f.render_widget(Clear, area);

    let items = settings_items(app);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Settings ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    let list = List::new(items).block(block);

    f.render_widget(list, area);
}

fn settings_items(app: &App) -> Vec<ListItem<'static>> {
    let selected = app.settings_selected_index;
    let editing = &app.settings_editing;

    vec![
        // Playback section header
        ListItem::new(Line::from(
            Span::styled("Playback", Style::default().add_modifier(Modifier::BOLD))
        )),
        ListItem::new("────────"),
        checkbox_item(2, "Audio Only", app.config.audio_only, selected),
        checkbox_item(3, "Bandwidth Limit (360p video, 128k audio)", app.config.bandwidth_limit, selected),
        checkbox_item(4, "Keep Temporary Files", app.config.keep_temp, selected),
        checkbox_item(5, "Include YouTube Shorts", app.config.include_shorts, selected),
        ListItem::new(""),

        // Downloads section header
        ListItem::new(Line::from(
            Span::styled("Downloads", Style::default().add_modifier(Modifier::BOLD))
        )),
        ListItem::new("─────────"),
        checkbox_item(9, "Download Mode (save permanently)", app.config.download_mode, selected),
        text_field_item(10, "Download Directory", &app.config.download_dir, selected, editing, SettingsField::DownloadDir),
        ListItem::new(""),

        // Display section header
        ListItem::new(Line::from(
            Span::styled("Display", Style::default().add_modifier(Modifier::BOLD))
        )),
        ListItem::new("───────"),
        text_field_item(14, "Results Per Page",
            &app.results_per_page_input.as_ref()
                .unwrap_or(&app.config.results_per_page.to_string()),
            selected, editing, SettingsField::ResultsPerPage),
        ListItem::new(""),

        // Advanced section header
        ListItem::new(Line::from(
            Span::styled("Advanced", Style::default().add_modifier(Modifier::BOLD))
        )),
        ListItem::new("────────"),
        text_field_item(18, "Custom Format", &app.config.custom_format, selected, editing, SettingsField::CustomFormat),
        ListItem::new(Line::from(
            Span::styled("(leave empty for auto)", Style::default().fg(Color::DarkGray))
        )),
        ListItem::new(""),
        ListItem::new(""),
        ListItem::new(Line::from(
            Span::styled("Changes are saved automatically", Style::default().fg(Color::Green))
        )),
        ListItem::new(Line::from(
            Span::styled("Press [Esc] to close", Style::default().fg(Color::Cyan))
        )),
    ]
}

fn checkbox_item(idx: usize, label: &str, checked: bool, selected: usize) -> ListItem<'static> {
    let checkbox = if checked { "[✓]" } else { "[ ]" };
    let text = format!("  {} {}", checkbox, label);
    let style = if idx == selected {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default().fg(Color::White)
    };
    ListItem::new(text).style(style)
}

fn text_field_item(idx: usize, label: &str, value: &str, selected: usize,
                   editing: &Option<SettingsField>, field: SettingsField) -> ListItem<'static> {
    let is_editing = if let Some(edit_field) = editing {
        *edit_field == field && idx == selected
    } else {
        false
    };
    let cursor = if is_editing { "█" } else { "" };
    let text = format!("  {}: [{}{}]", label, value, cursor);
    let style = if idx == selected {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default().fg(Color::White)
    };
    ListItem::new(text).style(style)
}
