use std::borrow::Cow;

use crate::ui::app::{App, FocusedPanel, InputMode, SearchPhase, SettingsField};
use crate::video::{Frame as VideoFrame, VideoDisplay};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState,
    },
};

const MIN_USABLE_WIDTH: u16 = 24;
const MIN_USABLE_HEIGHT: u16 = 8;
const WIDE_LAYOUT_WIDTH: u16 = 72;

pub fn render_ui(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.width < MIN_USABLE_WIDTH || area.height < MIN_USABLE_HEIGHT {
        render_small_terminal(f, area);
        return;
    }

    let footer_height = footer_height(app.player_manager.is_some(), area.height);

    if app.video_view {
        // The video widget takes over everywhere the search bar and
        // results/queue panels normally live; only the footer stays.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(footer_height)])
            .split(area);

        render_video_view(f, app, chunks[0]);
        render_footer(f, app, chunks[1]);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(footer_height),
            ])
            .split(area);

        render_search_bar(f, app, chunks[0]);
        render_main_content(f, app, chunks[1]);
        render_footer(f, app, chunks[2]);
    }

    if app.input_mode == InputMode::Help {
        render_help_overlay(f, app);
    }

    if app.settings_open {
        render_settings_modal(f, app);
    }
}

fn render_small_terminal(f: &mut Frame, area: Rect) {
    let text = if area.height >= 3 {
        vec![
            Line::from("Terminal too small"),
            Line::from("Resize to at least 24×8 · Ctrl-C quits"),
        ]
    } else {
        vec![Line::from("Resize terminal · Ctrl-C quits")]
    };

    let notice = Paragraph::new(text)
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Yellow))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" youtui "),
        );
    f.render_widget(notice, area);
}

fn render_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::SearchBar;

    let (border_style, title_style) = if is_focused {
        (
            Style::default().fg(Color::Cyan),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::DarkGray),
        )
    };

    let display_text = if is_focused {
        visible_input(&app.search_input, area.width.saturating_sub(2) as usize)
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
    if area.width >= WIDE_LAYOUT_WIDTH {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
            .split(area);

        render_results(f, app, chunks[0]);
        render_queue_panel(f, app, chunks[1]);
    } else if area.height >= 20 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(area);

        render_results(f, app, chunks[0]);
        render_queue_panel(f, app, chunks[1]);
    } else if app.focused_panel == FocusedPanel::Queue {
        render_queue_panel(f, app, area);
    } else {
        // On short, narrow terminals, preserve a useful number of rows and show
        // the focused panel instead of squeezing both panels into unusability.
        render_results(f, app, area);
    }
}

/// Height of the footer (status + controls, or just controls) for a given
/// terminal height and whether a player is active. Shared with the runner's
/// status-poll loop so the ffmpeg pane it sizes matches what gets rendered.
pub fn footer_height(has_player: bool, area_height: u16) -> u16 {
    if has_player && area_height >= 14 {
        4
    } else {
        2
    }
}

/// Terminal cell dimensions available to the video pane for a given
/// terminal size, mirroring the layout `render_ui` uses when `video_view`
/// is active (full width, height minus the footer).
pub fn video_pane_size(has_player: bool, area_width: u16, area_height: u16) -> (u16, u16) {
    let rows = area_height.saturating_sub(footer_height(has_player, area_height));
    (area_width, rows)
}

fn render_video_view(f: &mut Frame, app: &App, area: Rect) {
    match app.video.render_state() {
        VideoDisplay::Error(message) => render_video_message(f, area, message, Color::Red),
        VideoDisplay::Loading => render_video_message(f, area, "loading video…", Color::Yellow),
        VideoDisplay::Placeholder => {
            render_video_message(f, area, "no video playing", Color::DarkGray)
        }
        VideoDisplay::Frame(frame, paused) => {
            let lines = frame_to_lines(frame);
            f.render_widget(Paragraph::new(lines), area);
            if paused {
                render_pause_overlay(f, area);
            }
        }
    }
}

fn render_video_message(f: &mut Frame, area: Rect, message: &str, color: Color) {
    let paragraph = Paragraph::new(Line::from(Span::styled(
        message,
        Style::default().fg(color),
    )))
    .alignment(Alignment::Center);
    let centered = Rect::new(
        area.x,
        area.y + area.height / 2,
        area.width,
        1.min(area.height),
    );
    f.render_widget(paragraph, centered);
}

fn render_pause_overlay(f: &mut Frame, area: Rect) {
    let text = " ⏸ PAUSED ";
    let width = (Line::from(text).width() as u16).min(area.width);
    let overlay = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height / 2,
        width,
        1.min(area.height),
    );
    let paragraph = Paragraph::new(Span::styled(
        text,
        Style::default()
            .fg(Color::White)
            .bg(Color::Black)
            .add_modifier(Modifier::BOLD),
    ))
    .alignment(Alignment::Center);
    f.render_widget(paragraph, overlay);
}

/// Pure mapping from a decoded frame to the `▀` cell lines used to render
/// it: each cell's foreground is the top pixel, background the bottom pixel,
/// giving 2x vertical resolution. Kept separate from rendering so it is
/// testable without a `Frame`/`Terminal`.
fn frame_to_lines(frame: &VideoFrame) -> Vec<Line<'static>> {
    let width = frame.width as usize;
    let rows = (frame.height_px / 2) as usize;
    (0..rows)
        .map(|row| {
            let spans: Vec<Span<'static>> = (0..width)
                .map(|col| {
                    let (fr, fg_g, fb) = video_pixel_at(frame, col, row * 2);
                    let (br, bg_g, bb) = video_pixel_at(frame, col, row * 2 + 1);
                    Span::styled(
                        "▀",
                        Style::default()
                            .fg(Color::Rgb(fr, fg_g, fb))
                            .bg(Color::Rgb(br, bg_g, bb)),
                    )
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

fn video_pixel_at(frame: &VideoFrame, x: usize, y: usize) -> (u8, u8, u8) {
    let idx = (y * frame.width as usize + x) * 3;
    (frame.rgb[idx], frame.rgb[idx + 1], frame.rgb[idx + 2])
}

/// Keep the edit cursor visible without slicing through a UTF-8 code point.
/// `Line::width` also accounts for wide terminal glyphs.
fn visible_input(input: &str, available_width: usize) -> String {
    if available_width == 0 {
        return String::new();
    }

    let cursor_width = 1;
    let text_width = available_width.saturating_sub(cursor_width);
    if Line::from(input).width() <= text_width {
        return format!("{input}█");
    }

    let tail_width = text_width.saturating_sub(1);
    let mut start = input.len();
    for (idx, _) in input.char_indices().rev() {
        if Line::from(&input[idx..]).width() > tail_width {
            break;
        }
        start = idx;
    }

    if text_width == 0 {
        "█".to_string()
    } else {
        format!("…{}█", &input[start..])
    }
}

/// Left-align to `width` display columns (wide-glyph aware via `Span::width`).
fn pad_column(text: &str, width: usize) -> String {
    let padding = " ".repeat(width.saturating_sub(Span::raw(text).width()));
    format!("{text}{padding}")
}

fn render_results(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::Results;

    let (border_style, title_style) = if is_focused {
        (
            Style::default().fg(Color::Cyan),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::DarkGray),
        )
    };

    let results = app.current_page_results();
    let start_idx = app.page.saturating_mul(app.page_size.max(1));

    let items: Vec<ListItem> = if results.is_empty() {
        if app.loading {
            vec![
                ListItem::new(""),
                ListItem::new(Line::from(vec![
                    Span::styled("  Searching for ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        app.query.as_str(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])),
                ListItem::new(Line::from(Span::styled(
                    "  Results will appear as they arrive · Esc cancels",
                    Style::default().fg(Color::Gray),
                ))),
            ]
        } else {
            vec![
                ListItem::new(""),
                ListItem::new(Line::from(vec![
                    Span::styled("  No results", Style::default().fg(Color::DarkGray)),
                    Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("press / to search", Style::default().fg(Color::Yellow)),
                ])),
            ]
        }
    } else {
        // Column widths over the visible page so duration | channel | views |
        // published line up across rows.
        let column_width = |field: fn(&crate::search::SearchResult) -> &str| {
            results
                .iter()
                .map(|result| Span::raw(field(result)).width())
                .max()
                .unwrap_or(0)
        };
        let duration_width = column_width(|r| &r.duration);
        let channel_width = column_width(|r| &r.channel);
        let views_width = column_width(|r| &r.views);

        results
            .iter()
            .enumerate()
            .map(|(i, result)| {
                let num = start_idx.saturating_add(i).saturating_add(1);
                let title_line = Line::from(vec![
                    Span::styled(
                        format!("{:>3}. ", num),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        result.title.as_str(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]);

                let mut meta_spans = vec![
                    Span::raw("     "),
                    Span::styled(
                        pad_column(&result.duration, duration_width),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        pad_column(&result.channel, channel_width),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        pad_column(&result.views, views_width),
                        Style::default().fg(Color::Gray),
                    ),
                ];
                if !result.published.is_empty() {
                    meta_spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
                    meta_spans.push(Span::styled(
                        result.published.as_str(),
                        Style::default().fg(Color::Gray),
                    ));
                }
                let meta_line = Line::from(meta_spans);

                ListItem::new(vec![title_line, meta_line])
            })
            .collect()
    };

    let page_info = if let Some(progress) = search_progress_label(app) {
        format!("Page {} · {progress}", app.page.saturating_add(1))
    } else if app.exhausted {
        let total_pages = app.total_results.div_ceil(app.page_size.max(1)).max(1);
        format!(
            "Page {}/{} · {} total",
            app.page.saturating_add(1),
            total_pages,
            app.total_results
        )
    } else {
        format!(
            "Page {} · {}+ loaded",
            app.page.saturating_add(1),
            app.total_results
        )
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
        .highlight_symbol("▶ ")
        .scroll_padding(1)
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    if is_focused && !results.is_empty() {
        state.select(Some(
            app.selected_index.min(results.len().saturating_sub(1)),
        ));
    }

    f.render_stateful_widget(list, area, &mut state);

    // Scrollbar on right edge
    let visible_rows = area.height.saturating_sub(2) as usize / 2;
    if results.len() > visible_rows.max(1) {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state = ScrollbarState::new(results.len())
            .position(app.selected_index.min(results.len().saturating_sub(1)));
        f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn search_progress_label(app: &App) -> Option<String> {
    if !app.loading {
        return None;
    }

    let page_size = app.page_size.max(1);
    Some(match app.search_phase {
        Some(SearchPhase::Initial) => format!(
            "Searching · {}/{} results",
            app.results.len().min(page_size),
            page_size
        ),
        Some(SearchPhase::RequestedPage { target_page }) => {
            let received = app
                .results
                .len()
                .saturating_sub(target_page.saturating_mul(page_size))
                .min(page_size);
            format!(
                "Loading page {} · {received}/{page_size} results",
                target_page.saturating_add(1)
            )
        }
        Some(SearchPhase::Prefetch { target_page }) => {
            let received = app
                .results
                .len()
                .saturating_sub(target_page.saturating_mul(page_size))
                .min(page_size);
            format!(
                "Fetching page {} · {received}/{page_size} results",
                target_page.saturating_add(1)
            )
        }
        None => "Searching".to_string(),
    })
}

fn render_queue_panel(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::Queue;

    let (border_style, title_style) = if is_focused {
        (
            Style::default().fg(Color::Cyan),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
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
                    Span::styled(format!("{:>2}. ", i + 1), num_style),
                    Span::styled(prefix, num_style),
                    Span::styled(track.title.as_str(), track_style),
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

    let list = List::new(items)
        .block(block)
        .highlight_symbol("› ")
        .scroll_padding(1)
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    if is_focused && !app.queue.is_empty() {
        state.select(Some(
            app.queue_selected_index
                .min(app.queue.len().saturating_sub(1)),
        ));
    }

    f.render_stateful_widget(list, area, &mut state);

    let visible_rows = area.height.saturating_sub(2) as usize;
    if app.queue.len() > visible_rows.max(1) {
        let mut scrollbar_state = ScrollbarState::new(app.queue.len()).position(
            app.queue_selected_index
                .min(app.queue.len().saturating_sub(1)),
        );
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    if app.player_manager.is_some() && area.height >= 4 {
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
        let play_icon = if status.paused { "⏸" } else { "▶" };
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

        if let Some(input) = app.timestamp_input.as_deref() {
            let prompt = Line::from(vec![
                Span::styled(
                    " Jump to: ",
                    Style::default().fg(Color::Gray).bg(Color::Black),
                ),
                Span::styled(
                    input,
                    Style::default()
                        .fg(Color::Yellow)
                        .bg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("▌", Style::default().fg(Color::Yellow).bg(Color::Black)),
            ]);
            f.render_widget(
                Paragraph::new(prompt).style(Style::default().bg(Color::Black)),
                chunks[1],
            );
            return;
        }

        // Line 2: progress bar + time + volume
        let elapsed = format_duration(status.time_pos.max(0.0) as u64);
        let duration = format_duration(status.duration.max(0.0) as u64);
        let metadata = format!(" {elapsed} / {duration} │ 🔊 {}% ", status.volume);
        let metadata_width = Line::from(metadata.as_str()).width();
        let bar_width = chunks[1]
            .width
            .saturating_sub(metadata_width as u16)
            .saturating_sub(1) as usize;

        let progress_frac = if status.duration > 0.0 && status.time_pos.is_finite() {
            (status.time_pos / status.duration).clamp(0.0, 1.0)
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
            Span::styled(empty, Style::default().fg(Color::DarkGray).bg(Color::Black)),
            Span::styled(
                format!(" {elapsed} "),
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("/", Style::default().fg(Color::DarkGray).bg(Color::Black)),
            Span::styled(
                format!(" {duration} "),
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
    let line = if let Some(message) = app.status_message.as_deref() {
        Line::from(vec![
            Span::styled(
                "[!] ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(message, Style::default().fg(Color::Yellow)),
        ])
    } else if app.video_view {
        controls_line(
            &[
                ("v", "Back"),
                ("Space", "Pause"),
                ("< >", "Seek"),
                ("n", "Next"),
                ("q", "Quit"),
            ],
            area.width as usize,
        )
    } else {
        match app.focused_panel {
            FocusedPanel::SearchBar => controls_line(
                &[
                    ("Enter", "Search"),
                    ("Esc", "Cancel"),
                    ("Tab", "Panel"),
                    ("F2", "Settings"),
                ],
                area.width as usize,
            ),
            FocusedPanel::Results => {
                if app.loading {
                    let mut controls = vec![("Esc", "Cancel")];
                    if !app.current_page_results().is_empty() {
                        controls.extend([("Enter", "Queue"), ("↑↓/jk", "Move"), ("n/p", "Page")]);
                    }
                    controls.extend([("/", "New search"), ("?", "Help"), ("Tab", "Panel")]);
                    controls_line(&controls, area.width as usize)
                } else if !app.number_input.is_empty() {
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
                    controls_line(
                        &[
                            ("q", "Quit"),
                            ("Enter", "Queue"),
                            ("↑↓/jk", "Move"),
                            ("n/p", "Page"),
                            ("/", "Search"),
                            ("?", "Help"),
                            ("Space", "Pause"),
                            ("< >", "Seek"),
                            ("Tab", "Panel"),
                        ],
                        area.width as usize,
                    )
                }
            }
            FocusedPanel::Queue => {
                let mut controls = if app.loading {
                    vec![("Esc", "Cancel search")]
                } else {
                    vec![("q", "Quit")]
                };
                controls.extend([
                    ("Enter", "Play"),
                    ("↑↓/jk", "Move"),
                    ("Del", "Remove"),
                    ("n", "Next"),
                    ("c", "Clear"),
                    ("?", "Help"),
                    ("Tab", "Panel"),
                ]);
                controls_line(&controls, area.width as usize)
            }
        }
    };

    let footer = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(footer, area);
}

/// Build a styled controls hint line: [Key] Action  [Key] Action ...
fn controls_line(controls: &[(&str, &str)], available_width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used_width = 0;
    for (i, (key, desc)) in controls.iter().enumerate() {
        let separator_width = usize::from(i > 0) * 3;
        let control_width = Line::from(format!("[{key}] {desc}")).width();
        if i > 0 && used_width + separator_width + control_width > available_width {
            break;
        }
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
        used_width += separator_width + control_width;
    }
    Line::from(spans)
}

fn render_help_overlay(f: &mut Frame, app: &App) {
    let show_full_help = f.area().width >= 72 && f.area().height >= 38;
    let mut help_text = if show_full_help {
        vec![
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
            help_row("    ↑↓ / j k    ", "Move selection"),
            help_row("    Enter       ", "Add to queue and play"),
            help_row(
                "    Digits      ",
                "Pick displayed or page-local #, then Enter",
            ),
            help_row("    n / p       ", "Next / Previous page"),
            help_row("    / or s      ", "Focus search bar"),
            Line::from(""),
            Line::from(Span::styled(
                "  Queue",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            help_row("    ↑↓ / j k    ", "Navigate queue"),
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
            help_row(
                "    ← / →       ",
                format!("Seek ±{}s", app.config.seek_step),
            ),
            help_row(
                "    Shift+←/→   ",
                format!("Seek ±{}s", app.config.seek_step_large),
            ),
            help_row("    t           ", "Jump to timestamp"),
            help_row("    + / -       ", "Volume up / down"),
            help_row("    m           ", "Mute toggle"),
            Line::from(""),
            Line::from(Span::styled(
                "  Other",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            help_row("    v           ", "Toggle terminal video view"),
            help_row("    ? / h       ", "Toggle this help"),
            help_row("    S / F2      ", "Settings"),
            help_row("    q           ", "Quit (outside this Help window)"),
        ]
    } else {
        compact_help_text(app.focused_panel)
    };

    let preferred_width = if show_full_help { 76 } else { 60 };
    let preferred_height = help_text.len().saturating_add(2) as u16;
    let area = popup_rect(preferred_width, preferred_height, f.area());
    let visible_lines = area.height.saturating_sub(2) as usize;
    if help_text.len() > visible_lines {
        help_text.truncate(visible_lines.saturating_sub(1));
        help_text.push(help_row("  Esc / ?     ", "Close help"));
    }

    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " Help ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " Esc / ? closes ",
            Style::default().fg(Color::DarkGray),
        ))
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(Color::Black));

    let paragraph = Paragraph::new(help_text).block(block);
    f.render_widget(paragraph, area);
}

fn compact_help_text(panel: FocusedPanel) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            " Keyboard shortcuts",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        help_row("  Tab / ⇧Tab  ", "Next / previous panel"),
    ];

    match panel {
        FocusedPanel::SearchBar => {
            lines.push(help_row("  Enter       ", "Search"));
            lines.push(help_row("  Esc         ", "Cancel search editing"));
        }
        FocusedPanel::Results => {
            lines.push(help_row("  ↑↓ / j k    ", "Move; Enter queues"));
            lines.push(help_row("  n / p       ", "Next / previous page"));
            lines.push(help_row("  /           ", "Edit search"));
        }
        FocusedPanel::Queue => {
            lines.push(help_row("  ↑↓ / j k    ", "Move; Enter plays"));
            lines.push(help_row("  Del         ", "Remove selected track"));
            lines.push(help_row("  n / c       ", "Next track / clear queue"));
        }
    }

    lines.extend([
        help_row("  Space / <>  ", "Pause / seek"),
        help_row("  v           ", "Toggle video view"),
        help_row("  S/F2 / q    ", "Settings / quit"),
        help_row("  Esc / ?     ", "Close help"),
    ]);
    lines
}

fn help_row(key: &'static str, desc: impl Into<Cow<'static, str>>) -> Line<'static> {
    Line::from(vec![
        Span::styled(key, Style::default().fg(Color::Cyan)),
        Span::styled(desc, Style::default().fg(Color::White)),
    ])
}

fn popup_rect(preferred_width: u16, preferred_height: u16, area: Rect) -> Rect {
    let width = preferred_width
        .min(area.width.saturating_sub(2))
        .max(1)
        .min(area.width);
    let height = preferred_height
        .min(area.height.saturating_sub(2))
        .max(1)
        .min(area.height);

    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
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
    let area = popup_rect(76, 27, f.area());
    f.render_widget(Clear, area);

    let items = settings_items(app);
    let bottom_title = if let Some(message) = app.status_message.as_deref() {
        Span::styled(
            format!(" ! {message} "),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            if app.settings_editing.is_some() {
                " Enter / Esc finishes editing "
            } else {
                " ↑↓/Tab move · Enter change · Esc close "
            },
            Style::default().fg(Color::DarkGray),
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            if app.settings_editing.is_some() {
                " Settings · editing "
            } else {
                " Settings "
            },
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(bottom_title)
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(Color::Black));

    let item_count = items.len();
    let list = List::new(items)
        .block(block)
        .highlight_symbol("› ")
        .scroll_padding(1);
    let mut state = ListState::default();
    if item_count > 0 {
        state.select(Some(
            app.settings_selected_index
                .min(item_count.saturating_sub(1)),
        ));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn settings_items(app: &App) -> Vec<ListItem<'static>> {
    let selected = app.settings_selected_index;
    let editing = &app.settings_editing;
    let buffered_value = app.settings_text_input.as_deref();
    let results_per_page = app.config.results_per_page.to_string();
    let seek_step = app.config.seek_step.to_string();
    let seek_step_large = app.config.seek_step_large.to_string();
    let download_dir = if editing == &Some(SettingsField::DownloadDir) {
        buffered_value.unwrap_or(&app.config.download_dir)
    } else {
        &app.config.download_dir
    };
    let results_per_page = if editing == &Some(SettingsField::ResultsPerPage) {
        buffered_value.unwrap_or(&results_per_page)
    } else {
        &results_per_page
    };
    let custom_format = if editing == &Some(SettingsField::CustomFormat) {
        buffered_value.unwrap_or(&app.config.custom_format)
    } else {
        &app.config.custom_format
    };
    let seek_step = if editing == &Some(SettingsField::SeekStep) {
        buffered_value.unwrap_or(&seek_step)
    } else {
        &seek_step
    };
    let seek_step_large = if editing == &Some(SettingsField::SeekStepLarge) {
        buffered_value.unwrap_or(&seek_step_large)
    } else {
        &seek_step_large
    };

    vec![
        section_header("  Playback"),
        section_rule(),
        checkbox_item(2, "Audio Only", app.config.audio_only, selected),
        checkbox_item(
            3,
            "Bandwidth Limit (360p video / 128k audio)",
            app.config.bandwidth_limit,
            selected,
        ),
        checkbox_item(4, "Keep Temporary Files", app.config.keep_temp, selected),
        checkbox_item(
            5,
            "Include YouTube Shorts",
            app.config.include_shorts,
            selected,
        ),
        checkbox_item(6, "Auto Play Queue", app.config.auto_play_queue, selected),
        text_field_item(
            7,
            "Seek step (s)",
            seek_step,
            selected,
            editing,
            SettingsField::SeekStep,
        ),
        text_field_item(
            8,
            "Large seek step (s)",
            seek_step_large,
            selected,
            editing,
            SettingsField::SeekStepLarge,
        ),
        ListItem::new(""),
        section_header("  Downloads"),
        section_rule(),
        checkbox_item(
            12,
            "Download Mode (save permanently)",
            app.config.download_mode,
            selected,
        ),
        text_field_item(
            13,
            "Download Directory",
            download_dir,
            selected,
            editing,
            SettingsField::DownloadDir,
        ),
        ListItem::new(""),
        section_header("  Display"),
        section_rule(),
        text_field_item(
            17,
            "Results Per Page",
            results_per_page,
            selected,
            editing,
            SettingsField::ResultsPerPage,
        ),
        ListItem::new(""),
        section_header("  Advanced"),
        section_rule(),
        text_field_item(
            21,
            "Custom Format",
            custom_format,
            selected,
            editing,
            SettingsField::CustomFormat,
        ),
        ListItem::new(Line::from(Span::styled(
            "  (leave empty for auto)",
            Style::default().fg(Color::DarkGray),
        ))),
        ListItem::new(""),
        ListItem::new(Line::from(vec![
            Span::styled("  ✓ ", Style::default().fg(Color::Green)),
            Span::styled(
                "Text changes save when editing ends   ",
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
            Style::default()
                .fg(if checked {
                    Color::Green
                } else {
                    Color::DarkGray
                })
                .bg(Color::Blue),
            Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
            Style::default().bg(Color::Blue),
        )
    } else {
        (
            Style::default().fg(if checked {
                Color::Green
            } else {
                Color::DarkGray
            }),
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
    let is_editing = editing.as_ref().is_some_and(|f| *f == field && is_selected);
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
            Span::styled(format!("{}: ", label), Style::default().fg(Color::Cyan)),
            Span::styled(value_owned, Style::default().fg(Color::White)),
        ])
    };

    ListItem::new(line)
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::config::Config;
    use crate::search::SearchResult;

    fn app_with_unicode_content() -> App {
        let mut app = App::new("音楽と café 🎵".to_string(), 10, Config::default());
        app.search_input = "長い検索 query 🦀🦀🦀".to_string();
        app.results.push(SearchResult {
            title: "非常に長いタイトル — naïve café 🎶".to_string(),
            duration: "3:45".to_string(),
            channel: "チャンネル".to_string(),
            views: "1M".to_string(),
            published: String::new(),
            id: "unicode".to_string(),
        });
        app.total_results = 1;
        app.exhausted = true;
        app
    }

    #[test]
    fn frame_to_lines_maps_top_pixel_to_fg_and_bottom_pixel_to_bg() {
        // 2x4 pixel frame -> 2 columns, 2 cell rows of half-block glyphs.
        #[rustfmt::skip]
        let rgb = vec![
            255, 0, 0,     0, 255, 0,   // pixel row 0
            0, 0, 255,     255, 255, 0, // pixel row 1
            10, 20, 30,    40, 50, 60,  // pixel row 2
            70, 80, 90,    100, 110, 120, // pixel row 3
        ];
        let frame = VideoFrame {
            width: 2,
            height_px: 4,
            rgb,
        };

        let lines = frame_to_lines(&frame);
        assert_eq!(lines.len(), 2);

        let top_left = &lines[0].spans[0];
        assert_eq!(top_left.content, "▀");
        assert_eq!(top_left.style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(top_left.style.bg, Some(Color::Rgb(0, 0, 255)));

        let top_right = &lines[0].spans[1];
        assert_eq!(top_right.style.fg, Some(Color::Rgb(0, 255, 0)));
        assert_eq!(top_right.style.bg, Some(Color::Rgb(255, 255, 0)));

        let bottom_left = &lines[1].spans[0];
        assert_eq!(bottom_left.style.fg, Some(Color::Rgb(10, 20, 30)));
        assert_eq!(bottom_left.style.bg, Some(Color::Rgb(70, 80, 90)));

        let bottom_right = &lines[1].spans[1];
        assert_eq!(bottom_right.style.fg, Some(Color::Rgb(40, 50, 60)));
        assert_eq!(bottom_right.style.bg, Some(Color::Rgb(100, 110, 120)));
    }

    #[test]
    fn video_view_placeholder_renders_without_panicking() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = app_with_unicode_content();
        app.video_view = true;

        terminal.draw(|frame| render_ui(frame, &app)).unwrap();
    }

    #[test]
    fn visible_input_keeps_cursor_and_wide_glyphs_within_width() {
        assert_eq!(visible_input("abc", 4), "abc█");
        assert_eq!(visible_input("hello", 4), "…lo█");

        let visible = visible_input("界界", 4);
        assert_eq!(visible, "…界█");
        assert!(Line::from(visible).width() <= 4);
    }

    #[test]
    fn render_is_safe_across_tiny_and_responsive_terminal_sizes() {
        for (width, height) in [(1, 1), (10, 3), (24, 8), (40, 12), (60, 24), (100, 35)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let app = app_with_unicode_content();

            terminal.draw(|frame| render_ui(frame, &app)).unwrap();
        }
    }

    #[test]
    fn compact_help_and_scrolled_settings_render_without_panicking() {
        let backend = TestBackend::new(32, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = app_with_unicode_content();
        app.input_mode = InputMode::Help;
        terminal.draw(|frame| render_ui(frame, &app)).unwrap();

        app.input_mode = InputMode::Browse;
        app.settings_open = true;
        app.settings_selected_index = 21;
        terminal.draw(|frame| render_ui(frame, &app)).unwrap();
    }

    #[test]
    fn loading_progress_labels_report_partial_and_prefetched_pages() {
        let mut app = app_with_unicode_content();
        app.page_size = 10;
        app.loading = true;
        app.search_phase = Some(SearchPhase::Initial);
        assert_eq!(
            search_progress_label(&app).as_deref(),
            Some("Searching · 1/10 results")
        );

        app.results.extend((1..12).map(|index| SearchResult {
            title: format!("Track {index}"),
            duration: "3:00".to_string(),
            channel: "Channel".to_string(),
            views: "1K".to_string(),
            published: String::new(),
            id: index.to_string(),
        }));
        app.page = 1;
        app.search_phase = Some(SearchPhase::RequestedPage { target_page: 1 });
        assert_eq!(
            search_progress_label(&app).as_deref(),
            Some("Loading page 2 · 2/10 results")
        );
    }

    #[test]
    fn partial_results_remain_visible_during_loading() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = app_with_unicode_content();
        app.loading = true;
        app.search_phase = Some(SearchPhase::Initial);

        terminal.draw(|frame| render_ui(frame, &app)).unwrap();

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("naïve café"));
        assert!(screen.contains("Searching"));
        assert!(screen.contains("Esc"));
        assert!(screen.contains("Cancel"));
    }
}
