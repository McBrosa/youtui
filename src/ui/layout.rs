use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph,
        Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
    Frame,
};
use crate::ui::app::{App, FocusedPanel, InputMode, SettingsField};

pub fn render_ui(f: &mut Frame, app: &App) {
    let footer_height: u16 = if app.player_manager.is_some() { 4 } else { 2 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .split(f.area());

    render_search_bar(f, app, chunks[0]);
    render_main_content(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);

    if app.input_mode == InputMode::Help {
        render_help_overlay(f, app);
    }

    if app.settings_open {
        render_settings_modal(f, app);
    }
}

fn render_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::SearchBar;

    let (border_style, title_style) = if is_focused {
        (
            Style::default().fg(Color::Cyan),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::DarkGray),
        )
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
                .border_type(BorderType::Rounded)
                .title(Span::styled(" 🔍 Search ", title_style))
                .border_style(border_style),
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

    let (border_style, title_style) = if is_focused {
        (
            Style::default().fg(Color::Cyan),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::DarkGray),
        )
    };

    if app.loading {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(Span::styled(" Results ", title_style))
            .border_style(border_style);

        let loading_text = vec![
            Line::from(""),
            Line::from(""),
            Line::from(vec![
                Span::styled("  ⏳ Searching for  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    &app.query,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("…", Style::default().fg(Color::DarkGray)),
            ]),
        ];

        let paragraph = Paragraph::new(loading_text).block(block);
        f.render_widget(paragraph, area);
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
                Span::styled(
                    format!("  {:>3}. ", num),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    result.title.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);

            let meta_line = Line::from(vec![
                Span::raw("        "),
                Span::styled(
                    result.duration.clone(),
                    Style::default().fg(Color::Green),
                ),
                Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
                Span::styled(result.channel.clone(), Style::default().fg(Color::Cyan)),
                Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
                Span::styled(result.views.clone(), Style::default().fg(Color::Gray)),
            ]);

            ListItem::new(vec![title_line, meta_line])
        })
        .collect();

    let page_info = if app.exhausted {
        let total_pages = (app.total_results + app.page_size - 1) / app.page_size;
        format!("Page {}/{} · {} total", app.page + 1, total_pages, app.total_results)
    } else {
        format!("Page {} · {}+ loaded", app.page + 1, app.total_results)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            format!(" Results ({}) ", page_info),
            title_style,
        ))
        .border_style(border_style);

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    if is_focused {
        state.select(Some(app.selected_index));
    }

    f.render_stateful_widget(list, area, &mut state);

    // Scrollbar on right edge
    if results.len() > 1 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state =
            ScrollbarState::new(results.len()).position(app.selected_index);
        f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn render_queue_panel(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::Queue;

    let (border_style, title_style) = if is_focused {
        (
            Style::default().fg(Color::Cyan),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::DarkGray),
        )
    };

    let queue_title = if app.queue.is_empty() {
        " Queue ".to_string()
    } else {
        format!(" Queue ({}) ", app.queue.len())
    };

    let items: Vec<ListItem> = if app.queue.is_empty() {
        vec![
            ListItem::new(""),
            ListItem::new(Line::from(Span::styled(
                "  No tracks queued",
                Style::default().fg(Color::DarkGray),
            ))),
            ListItem::new(""),
            ListItem::new(Line::from(vec![
                Span::styled("  Press ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "[Enter]",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" on a result", Style::default().fg(Color::DarkGray)),
            ])),
        ]
    } else {
        app.queue
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let is_playing = i == 0 && app.player_manager.is_some();

                let (num_style, track_style, prefix) = if is_playing {
                    (
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                        "▶ ",
                    )
                } else {
                    (
                        Style::default().fg(Color::DarkGray),
                        Style::default().fg(Color::White),
                        "  ",
                    )
                };

                let line = Line::from(vec![
                    Span::styled(format!("  {:>2}. ", i + 1), num_style),
                    Span::styled(prefix, num_style),
                    Span::styled(track.title.clone(), track_style),
                ]);

                ListItem::new(line)
            })
            .collect()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(queue_title, title_style))
        .border_style(border_style);

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    if is_focused && !app.queue.is_empty() {
        state.select(Some(app.queue_selected_index));
    }

    f.render_stateful_widget(list, area, &mut state);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    if app.player_manager.is_some() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
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

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(area);

        // Line 1: play/pause icon + title
        let play_icon = if status.paused { "⏸ " } else { "▶ " };
        let title_line = Line::from(vec![
            Span::styled(
                format!(" {} ", play_icon),
                Style::default()
                    .fg(Color::Green)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                status.title.clone(),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(
            Paragraph::new(title_line).style(Style::default().bg(Color::Black)),
            chunks[0],
        );

        // Line 2: progress bar + time + volume
        let time_vol_width: u16 = 30;
        let bar_width = chunks[1].width.saturating_sub(time_vol_width) as usize;

        let progress_frac = if status.duration > 0.0 {
            status.time_pos / status.duration
        } else {
            0.0
        };

        // Build bar: filled ━━━● empty ───
        let thumb_pos = if bar_width > 0 {
            ((progress_frac * (bar_width.saturating_sub(1)) as f64) as usize)
                .min(bar_width.saturating_sub(1))
        } else {
            0
        };

        let filled = "━".repeat(thumb_pos);
        let empty = "─".repeat(bar_width.saturating_sub(thumb_pos + 1));

        let elapsed = format_duration(status.time_pos as u64);
        let duration = format_duration(status.duration as u64);

        let mut spans: Vec<Span> = vec![
            Span::styled(" ", Style::default().bg(Color::Black)),
            Span::styled(
                filled,
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        if bar_width > 0 {
            spans.push(Span::styled(
                "●",
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        spans.extend([
            Span::styled(
                empty,
                Style::default().fg(Color::DarkGray).bg(Color::Black),
            ),
            Span::styled(
                format!(" {} ", elapsed),
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("/", Style::default().fg(Color::DarkGray).bg(Color::Black)),
            Span::styled(
                format!(" {} ", duration),
                Style::default().fg(Color::Gray).bg(Color::Black),
            ),
            Span::styled("│", Style::default().fg(Color::DarkGray).bg(Color::Black)),
            Span::styled(
                format!(" 🔊 {}% ", status.volume),
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        f.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Black)),
            chunks[1],
        );
    }
}

fn render_controls_line(f: &mut Frame, app: &App, area: Rect) {
    let line = match app.focused_panel {
        FocusedPanel::SearchBar => controls_line(&[
            ("Enter", "Search"),
            ("Esc", "Cancel"),
            ("Tab", "Switch"),
            ("F2", "Settings"),
        ]),
        FocusedPanel::Results => {
            if !app.number_input.is_empty() {
                Line::from(vec![
                    Span::styled("Go to: ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        app.number_input.clone(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("_", Style::default().fg(Color::Yellow)),
                    Span::raw("   "),
                    Span::styled(
                        "[Enter]",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" Confirm   ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        "[Bksp]",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" Clear", Style::default().fg(Color::Gray)),
                ])
            } else {
                controls_line(&[
                    ("↑↓", "Navigate"),
                    ("Enter", "Queue"),
                    ("Space", "Pause"),
                    ("< >", "Seek"),
                    ("n/p", "Page"),
                    ("Tab", "Switch"),
                    ("h", "Help"),
                    ("q", "Quit"),
                ])
            }
        }
        FocusedPanel::Queue => controls_line(&[
            ("↑↓", "Navigate"),
            ("Enter", "Jump"),
            ("Del", "Remove"),
            ("n", "Next"),
            ("c", "Clear"),
            ("Tab", "Switch"),
            ("h", "Help"),
        ]),
    };

    let footer = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(footer, area);
}

/// Build a styled controls hint line: [Key] Action  [Key] Action ...
fn controls_line(controls: &[(&str, &str)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, desc)) in controls.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(
            format!("[{}]", key),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {}", desc),
            Style::default().fg(Color::Gray),
        ));
    }
    Line::from(spans)
}

fn render_help_overlay(f: &mut Frame, _app: &App) {
    let area = centered_rect(62, 82, f.area());
    f.render_widget(Clear, area);

    let help_text = vec![
        Line::from(Span::styled(
            "  Focus Navigation",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        help_row("    Tab         ", "Cycle focus  Search › Results › Queue"),
        help_row("    Shift+Tab   ", "Reverse cycle"),
        Line::from(""),
        Line::from(Span::styled(
            "  Search Bar",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        help_row("    Enter       ", "Submit search"),
        help_row("    Esc         ", "Clear and return to Results"),
        Line::from(""),
        Line::from(Span::styled(
            "  Results",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        help_row("    ↑ / ↓       ", "Move selection"),
        help_row("    Enter       ", "Add to queue and play"),
        help_row("    1-9         ", "Quick-pick by number"),
        help_row("    n / p       ", "Next / Previous page"),
        help_row("    s           ", "Focus search bar"),
        Line::from(""),
        Line::from(Span::styled(
            "  Queue",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        help_row("    ↑ / ↓       ", "Navigate queue"),
        help_row("    Enter       ", "Jump to track"),
        help_row("    Del / Bksp  ", "Remove track"),
        help_row("    n / c       ", "Next track / Clear queue"),
        Line::from(""),
        Line::from(Span::styled(
            "  Playback (global)",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        help_row("    Space       ", "Play / Pause"),
        help_row("    < / >       ", "Seek -/+ 10 seconds"),
        help_row("    + / -       ", "Volume up / down"),
        help_row("    m           ", "Mute toggle"),
        Line::from(""),
        Line::from(Span::styled(
            "  Other",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        help_row("    h           ", "Toggle this help"),
        help_row("    F2          ", "Settings"),
        help_row("    q / Esc     ", "Quit"),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " Help ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(Color::Black));

    let paragraph = Paragraph::new(help_text).block(block);
    f.render_widget(paragraph, area);
}

fn help_row(key: &'static str, desc: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(key, Style::default().fg(Color::Cyan)),
        Span::styled(desc, Style::default().fg(Color::White)),
    ])
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
    let area = centered_rect(60, 78, f.area());
    f.render_widget(Clear, area);

    let items = settings_items(app);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " Settings ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(Color::Black));

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn settings_items(app: &App) -> Vec<ListItem<'static>> {
    let selected = app.settings_selected_index;
    let editing = &app.settings_editing;

    vec![
        section_header("  Playback"),
        section_rule(),
        checkbox_item(2, "Audio Only", app.config.audio_only, selected),
        checkbox_item(3, "Bandwidth Limit (360p video / 128k audio)", app.config.bandwidth_limit, selected),
        checkbox_item(4, "Keep Temporary Files", app.config.keep_temp, selected),
        checkbox_item(5, "Include YouTube Shorts", app.config.include_shorts, selected),
        checkbox_item(6, "Auto Play Queue", app.config.auto_play_queue, selected),
        ListItem::new(""),
        section_header("  Downloads"),
        section_rule(),
        checkbox_item(10, "Download Mode (save permanently)", app.config.download_mode, selected),
        text_field_item(11, "Download Directory", &app.config.download_dir, selected, editing, SettingsField::DownloadDir),
        ListItem::new(""),
        section_header("  Display"),
        section_rule(),
        text_field_item(
            15,
            "Results Per Page",
            app.results_per_page_input
                .as_deref()
                .unwrap_or(&app.config.results_per_page.to_string()),
            selected,
            editing,
            SettingsField::ResultsPerPage,
        ),
        ListItem::new(""),
        section_header("  Advanced"),
        section_rule(),
        text_field_item(19, "Custom Format", &app.config.custom_format, selected, editing, SettingsField::CustomFormat),
        ListItem::new(Line::from(Span::styled(
            "  (leave empty for auto)",
            Style::default().fg(Color::DarkGray),
        ))),
        ListItem::new(""),
        ListItem::new(Line::from(vec![
            Span::styled("  ✓ ", Style::default().fg(Color::Green)),
            Span::styled(
                "Changes saved automatically   ",
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                "[Esc]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Close", Style::default().fg(Color::Gray)),
        ])),
    ]
}

fn section_header(title: &'static str) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        title,
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )))
}

fn section_rule() -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        "  ────────────────────────────────────",
        Style::default().fg(Color::DarkGray),
    )))
}

fn checkbox_item(
    idx: usize,
    label: &'static str,
    checked: bool,
    selected: usize,
) -> ListItem<'static> {
    let is_selected = idx == selected;
    let checkbox = if checked { "✓" } else { " " };

    let (checkbox_style, label_style, bg) = if is_selected {
        (
            Style::default().fg(if checked { Color::Green } else { Color::DarkGray }).bg(Color::Blue),
            Style::default().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD),
            Style::default().bg(Color::Blue),
        )
    } else {
        (
            Style::default().fg(if checked { Color::Green } else { Color::DarkGray }),
            Style::default().fg(Color::White),
            Style::default(),
        )
    };

    let line = Line::from(vec![
        Span::styled("  ", bg),
        Span::styled(format!("[{}] ", checkbox), checkbox_style),
        Span::styled(label, label_style),
    ]);

    ListItem::new(line)
}

fn text_field_item(
    idx: usize,
    label: &'static str,
    value: &str,
    selected: usize,
    editing: &Option<SettingsField>,
    field: SettingsField,
) -> ListItem<'static> {
    let is_selected = idx == selected;
    let is_editing = editing.as_ref().map_or(false, |f| *f == field && is_selected);
    let cursor = if is_editing { "█" } else { "" };
    let value_owned = format!("[{}{}]", value, cursor);

    let line = if is_selected {
        Line::from(vec![
            Span::styled("  ", Style::default().bg(Color::Blue)),
            Span::styled(
                format!("{}: ", label),
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                value_owned,
                Style::default().fg(Color::White).bg(Color::Blue),
            ),
        ])
    } else {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{}: ", label),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(value_owned, Style::default().fg(Color::White)),
        ])
    };

    ListItem::new(line)
}
