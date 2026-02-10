# Rich TUI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform yt-search-play into a rich terminal UI with ratatui, featuring arrow key navigation, live status updates, and polished visual layout.

**Architecture:** Add new `ui/` module as a view layer over existing business logic. Main loop transitions from blocking stdin reads to non-blocking event loop with crossterm. Existing search/player/config modules remain unchanged.

**Tech Stack:** ratatui 0.26, crossterm 0.27, existing Rust stack (clap, colored, anyhow)

---

## Phase 1: Foundation

### Task 1: Add Dependencies

**Files:**
- Modify: `Cargo.toml:6-12`

**Step 1: Add ratatui and crossterm dependencies**

Add after line 12 in Cargo.toml:

```toml
ratatui = "0.26"
crossterm = "0.27"
```

**Step 2: Verify dependencies resolve**

Run: `cargo check`
Expected: Compiles successfully, downloads new crates

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat: add ratatui and crossterm dependencies"
```

---

### Task 2: Create UI Module Structure

**Files:**
- Create: `src/ui/mod.rs`
- Create: `src/ui/app.rs`
- Create: `src/ui/layout.rs`
- Create: `src/ui/events.rs`
- Modify: `src/main.rs:1-6`

**Step 1: Create ui module directory**

Run: `mkdir -p src/ui`

**Step 2: Create module declaration file**

Create `src/ui/mod.rs`:

```rust
pub mod app;
pub mod events;
pub mod layout;

pub use app::App;
pub use events::{AppEvent, handle_key_event};
pub use layout::render_ui;
```

**Step 3: Create app state stub**

Create `src/ui/app.rs`:

```rust
use crate::search::SearchResult;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputMode {
    Browse,
    Search,
    Help,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackState {
    Idle,
    Playing { title: String, elapsed: u64, duration: u64 },
}

pub struct App {
    pub results: Vec<SearchResult>,
    pub selected_index: usize,
    pub page: usize,
    pub page_size: usize,
    pub total_results: usize,
    pub exhausted: bool,
    pub query: String,
    pub input_mode: InputMode,
    pub search_input: String,
    pub playback_state: PlaybackState,
    pub should_quit: bool,
}

impl App {
    pub fn new(query: String, page_size: usize) -> Self {
        Self {
            results: Vec::new(),
            selected_index: 0,
            page: 0,
            page_size,
            total_results: 0,
            exhausted: false,
            query,
            input_mode: InputMode::Browse,
            search_input: String::new(),
            playback_state: PlaybackState::Idle,
            should_quit: false,
        }
    }

    pub fn current_page_results(&self) -> &[SearchResult] {
        let start = self.page * self.page_size;
        let end = (start + self.page_size).min(self.results.len());
        &self.results[start..end]
    }

    pub fn has_next_page(&self) -> bool {
        let end = (self.page + 1) * self.page_size;
        end < self.results.len() || !self.exhausted
    }

    pub fn has_prev_page(&self) -> bool {
        self.page > 0
    }
}
```

**Step 4: Create events stub**

Create `src/ui/events.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::ui::app::{App, InputMode};

#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
}

pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    match app.input_mode {
        InputMode::Browse => handle_browse_keys(app, key),
        InputMode::Search => handle_search_keys(app, key),
        InputMode::Help => handle_help_keys(app, key),
    }
}

fn handle_browse_keys(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            app.should_quit = true;
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        _ => {}
    }
}

fn handle_search_keys(app: &mut App, _key: KeyEvent) {
    // TODO: Implement search input handling
}

fn handle_help_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('h') | KeyCode::Char('q') => {
            app.input_mode = InputMode::Browse;
        }
        _ => {}
    }
}
```

**Step 5: Create layout stub**

Create `src/ui/layout.rs`:

```rust
use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Text,
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use crate::ui::app::App;

pub fn render_ui<B: Backend>(f: &mut Frame<B>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // Header
            Constraint::Min(0),     // Results
            Constraint::Length(2),  // Footer
        ])
        .split(f.size());

    render_header(f, app, chunks[0]);
    render_results(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);
}

fn render_header<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
    let header = Paragraph::new(format!("yt-search-play │ Query: {}", app.query))
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(header, area);
}

fn render_results<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Search Results");
    let text = Text::from(format!("{} results", app.results.len()));
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn render_footer<B: Backend>(f: &mut Frame<B>, _app: &App, area: Rect) {
    let footer = Paragraph::new("q: Quit")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, area);
}
```

**Step 6: Add ui module to main.rs**

Add after line 6 in `src/main.rs`:

```rust
mod ui;
```

**Step 7: Verify compilation**

Run: `cargo check`
Expected: Compiles successfully

**Step 8: Commit**

```bash
git add src/ui/ src/main.rs
git commit -m "feat: add ui module structure with app state and stubs"
```

---

### Task 3: Terminal Setup and Cleanup

**Files:**
- Create: `src/ui/terminal.rs`
- Modify: `src/ui/mod.rs:1-7`

**Step 1: Create terminal management module**

Create `src/ui/terminal.rs`:

```rust
use std::io;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use anyhow::Result;

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

pub fn init_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

pub fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

pub struct TerminalGuard {
    terminal: Option<Tui>,
}

impl TerminalGuard {
    pub fn new(terminal: Tui) -> Self {
        Self {
            terminal: Some(terminal),
        }
    }

    pub fn get_mut(&mut self) -> &mut Tui {
        self.terminal.as_mut().unwrap()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(mut terminal) = self.terminal.take() {
            let _ = restore_terminal(&mut terminal);
        }
    }
}
```

**Step 2: Export terminal module**

Modify `src/ui/mod.rs` to add:

```rust
pub mod terminal;

pub use terminal::{init_terminal, restore_terminal, TerminalGuard};
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ui/terminal.rs src/ui/mod.rs
git commit -m "feat: add terminal setup and cleanup with Drop guard"
```

---

## Phase 2: Core TUI Loop

### Task 4: Basic Event Loop

**Files:**
- Create: `src/ui/runner.rs`
- Modify: `src/ui/mod.rs:1-10`

**Step 1: Create event loop runner**

Create `src/ui/runner.rs`:

```rust
use std::time::{Duration, Instant};
use crossterm::event::{self, Event};
use anyhow::Result;
use crate::ui::{App, AppEvent, handle_key_event, layout::render_ui, terminal::Tui};

const TICK_RATE: Duration = Duration::from_millis(250);

pub fn run_app(terminal: &mut Tui, mut app: App) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| render_ui(f, &app))?;

        let timeout = TICK_RATE
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(&mut app, key);
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
```

**Step 2: Export runner**

Modify `src/ui/mod.rs` to add:

```rust
pub mod runner;

pub use runner::run_app;
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ui/runner.rs src/ui/mod.rs
git commit -m "feat: add basic TUI event loop with polling"
```

---

### Task 5: Integrate TUI into Main

**Files:**
- Modify: `src/main.rs:19-131`

**Step 1: Replace main loop with TUI**

Replace the entire `main()` function in `src/main.rs`:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();

    // Check dependencies
    check_ytdlp()?;
    let player = detect_player()?;

    // Build config
    let config = Config::from_cli(&cli, player);

    // Create managed temp dir
    let temp_dir = ManagedTempDir::new(config.keep_temp)?;

    // Set up Ctrl-C handler
    setup_signal_handler();

    // Paginated search — fetches one page at a time, caches previous pages
    let page_size = config.num_results;
    let mut search = PaginatedSearch::new(&config.query, page_size, !config.include_shorts);

    // Fetch first page
    println!("{} {}", "Searching for:".blue(), config.query);
    search.ensure_page(0)?;

    if search.results.is_empty() {
        eprintln!("{}", "No results found.".red());
        if !config.include_shorts {
            eprintln!(
                "{}",
                "Try again with -i/--include-shorts option if you want to include shorter videos".yellow()
            );
        }
        std::process::exit(1);
    }

    // Initialize TUI
    let mut terminal = ui::init_terminal()?;
    let terminal_guard = ui::TerminalGuard::new(terminal);
    terminal = terminal_guard.get_mut().clone();

    // Create app state
    let mut app = ui::App::new(config.query.clone(), page_size);
    app.results = search.results.clone();
    app.total_results = search.results.len();
    app.exhausted = search.exhausted;

    // Run TUI
    let result = ui::run_app(&mut terminal, app);

    // Terminal cleanup happens via Drop guard
    drop(terminal_guard);

    result
}
```

**Step 2: Build and test**

Run: `cargo build`
Expected: Compiles successfully

**Step 3: Test basic TUI**

Run: `cargo run -- "rust tutorials"`
Expected: TUI starts, shows placeholder, pressing 'q' exits cleanly

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: integrate TUI into main loop"
```

---

### Task 6: Render Results List

**Files:**
- Modify: `src/ui/layout.rs:30-45`

**Step 1: Implement results rendering**

Replace `render_results` function in `src/ui/layout.rs`:

```rust
fn render_results<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
    let results = app.current_page_results();
    let start_idx = app.page * app.page_size;

    let items: Vec<String> = results
        .iter()
        .enumerate()
        .map(|(i, result)| {
            let num = start_idx + i + 1;
            format!(
                "{:>3}. {}\n     Duration: {} | Channel: {} | Views: {} | ID: {}",
                num, result.title, result.duration, result.channel, result.views, result.id
            )
        })
        .collect();

    let page_info = if app.exhausted {
        format!("Page {}/{} • {} total", app.page + 1, (app.total_results + app.page_size - 1) / app.page_size, app.total_results)
    } else {
        format!("Page {} • {}+ loaded", app.page + 1, app.total_results)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Search Results ({})", page_info))
        .border_style(Style::default().fg(Color::DarkGray));

    let text = items.join("\n\n");
    let paragraph = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(Color::White));

    f.render_widget(paragraph, area);
}
```

**Step 2: Test rendering**

Run: `cargo run -- "rust tutorials"`
Expected: Shows actual search results in bordered panel

**Step 3: Commit**

```bash
git add src/ui/layout.rs
git commit -m "feat: render search results in TUI list"
```

---

### Task 7: Arrow Key Navigation

**Files:**
- Modify: `src/ui/events.rs:25-35`
- Modify: `src/ui/layout.rs:30-60`

**Step 1: Add navigation key handling**

Replace `handle_browse_keys` in `src/ui/events.rs`:

```rust
fn handle_browse_keys(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            app.should_quit = true;
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        (KeyCode::Up, _) => {
            if app.selected_index > 0 {
                app.selected_index -= 1;
            }
        }
        (KeyCode::Down, _) => {
            let page_results = app.current_page_results();
            if app.selected_index < page_results.len().saturating_sub(1) {
                app.selected_index += 1;
            }
        }
        (KeyCode::Char('n'), _) if app.has_next_page() => {
            app.page += 1;
            app.selected_index = 0;
        }
        (KeyCode::Char('p'), _) if app.has_prev_page() => {
            app.page -= 1;
            app.selected_index = 0;
        }
        (KeyCode::Char('h'), _) => {
            app.input_mode = InputMode::Help;
        }
        _ => {}
    }
}
```

**Step 2: Highlight selected item in rendering**

Update `render_results` in `src/ui/layout.rs` to use `List` widget:

```rust
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

fn render_results<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
    let results = app.current_page_results();
    let start_idx = app.page * app.page_size;

    let items: Vec<ListItem> = results
        .iter()
        .enumerate()
        .map(|(i, result)| {
            let num = start_idx + i + 1;
            let content = format!(
                "{:>3}. {}\n     Duration: {} | Channel: {} | Views: {}",
                num, result.title, result.duration, result.channel, result.views
            );

            let style = if i == app.selected_index {
                Style::default().bg(Color::Yellow).fg(Color::Black)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(content).style(style)
        })
        .collect();

    let page_info = if app.exhausted {
        format!("Page {}/{} • {} total", app.page + 1, (app.total_results + app.page_size - 1) / app.page_size, app.total_results)
    } else {
        format!("Page {} • {}+ loaded", app.page + 1, app.total_results)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Search Results ({})", page_info))
        .border_style(Style::default().fg(Color::DarkGray));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

    let mut state = ListState::default();
    state.select(Some(app.selected_index));

    f.render_stateful_widget(list, area, &mut state);
}
```

**Step 3: Test navigation**

Run: `cargo run -- "rust tutorials"`
Expected: Arrow keys move selection, selected item highlighted yellow

**Step 4: Commit**

```bash
git add src/ui/events.rs src/ui/layout.rs
git commit -m "feat: add arrow key navigation with visual selection"
```

---

## Phase 3: Rich Rendering

### Task 8: Polish Visual Styling

**Files:**
- Modify: `src/ui/layout.rs:1-100`

**Step 1: Add unicode symbols and better formatting**

Update entire `src/ui/layout.rs`:

```rust
use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use crate::ui::app::{App, InputMode, PlaybackState};

pub fn render_ui<B: Backend>(f: &mut Frame<B>, app: &App) {
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

fn render_header<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
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

fn render_results<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
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

fn render_status_bar<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
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

fn render_footer<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
    let hints = match app.input_mode {
        InputMode::Browse => {
            let mut h = vec![
                "↑/↓: Navigate",
                "Enter: Play",
                "n/p: Next/Prev",
                "h: Help",
                "q: Quit",
            ];
            h
        }
        InputMode::Search => vec!["Type to search", "Enter: Submit", "Esc: Cancel"],
        InputMode::Help => vec!["Press h or Esc to close help"],
    };

    let text = hints.join(" • ");
    let footer = Paragraph::new(text).style(Style::default().fg(Color::Cyan));
    f.render_widget(footer, area);
}

fn render_help_overlay<B: Backend>(f: &mut Frame<B>, _app: &App) {
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
```

**Step 2: Test styled UI**

Run: `cargo run -- "rust tutorials"`
Expected: Unicode symbols, bold titles, colored header, help overlay with 'h'

**Step 3: Commit**

```bash
git add src/ui/layout.rs
git commit -m "feat: add rich visual styling with unicode symbols and colors"
```

---

## Phase 4: Hybrid Navigation

### Task 9: Number Quick-Pick

**Files:**
- Modify: `src/ui/app.rs:20-25`
- Modify: `src/ui/events.rs:25-50`

**Step 1: Add number input buffer to App**

Add field to `App` struct in `src/ui/app.rs`:

```rust
pub number_input: String,
```

Initialize in `App::new()`:

```rust
number_input: String::new(),
```

**Step 2: Handle number input in events**

Update `handle_browse_keys` in `src/ui/events.rs`:

```rust
fn handle_browse_keys(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            app.should_quit = true;
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        (KeyCode::Up, _) => {
            if app.selected_index > 0 {
                app.selected_index -= 1;
            }
        }
        (KeyCode::Down, _) => {
            let page_results = app.current_page_results();
            if app.selected_index < page_results.len().saturating_sub(1) {
                app.selected_index += 1;
            }
        }
        (KeyCode::Char('n'), _) if app.has_next_page() => {
            app.page += 1;
            app.selected_index = 0;
        }
        (KeyCode::Char('p'), _) if app.has_prev_page() => {
            app.page -= 1;
            app.selected_index = 0;
        }
        (KeyCode::Char('h'), _) => {
            app.input_mode = InputMode::Help;
        }
        (KeyCode::Char('s'), _) => {
            app.input_mode = InputMode::Search;
            app.search_input.clear();
        }
        (KeyCode::Char(c), _) if c.is_ascii_digit() => {
            // Number quick-pick
            app.number_input.push(c);
        }
        (KeyCode::Enter, _) => {
            // Handle enter for number input or arrow selection
            if !app.number_input.is_empty() {
                if let Ok(num) = app.number_input.parse::<usize>() {
                    if num > 0 && num <= app.results.len() {
                        // TODO: Trigger play action
                        app.number_input.clear();
                    }
                }
                app.number_input.clear();
            } else {
                // Play selected item
                // TODO: Trigger play action for selected_index
            }
        }
        (KeyCode::Backspace, _) => {
            app.number_input.pop();
        }
        _ => {}
    }
}
```

**Step 3: Display number input in footer**

Update `render_footer` in `src/ui/layout.rs`:

```rust
fn render_footer<B: Backend>(f: &mut Frame<B>, app: &App, area: Rect) {
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
```

**Step 4: Test number input**

Run: `cargo run -- "rust tutorials"`
Expected: Typing numbers shows in footer, backspace works

**Step 5: Commit**

```bash
git add src/ui/app.rs src/ui/events.rs src/ui/layout.rs
git commit -m "feat: add number quick-pick input buffer"
```

---

### Task 10: Search Input Mode

**Files:**
- Modify: `src/ui/events.rs:40-60`

**Step 1: Implement search input handling**

Replace `handle_search_keys` in `src/ui/events.rs`:

```rust
fn handle_search_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => {
            app.search_input.push(c);
        }
        KeyCode::Backspace => {
            app.search_input.pop();
        }
        KeyCode::Enter => {
            if !app.search_input.is_empty() {
                app.query = app.search_input.clone();
                // TODO: Trigger new search
                app.input_mode = InputMode::Browse;
            }
        }
        KeyCode::Esc => {
            app.input_mode = InputMode::Browse;
            app.search_input.clear();
        }
        _ => {}
    }
}
```

**Step 2: Test search input**

Run: `cargo run -- "rust tutorials"`
Expected: Press 's', type query, shows in footer, Esc cancels

**Step 3: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: implement search input mode"
```

---

## Phase 5: Player Integration

### Task 11: Play Action Integration

**Files:**
- Modify: `src/ui/app.rs:1-80`
- Modify: `src/ui/runner.rs:1-50`
- Modify: `src/main.rs:50-70`

**Step 1: Add play action to App**

Add to `src/ui/app.rs`:

```rust
pub enum AppAction {
    None,
    Play(usize),
    NewSearch(String),
    FetchNextPage,
}

impl App {
    // ... existing methods ...

    pub fn get_action(&mut self) -> AppAction {
        // This will be set by event handlers
        AppAction::None
    }
}
```

Add field to App:

```rust
pub pending_action: AppAction,
```

Initialize in `new()`:

```rust
pending_action: AppAction::None,
```

**Step 2: Trigger play action from events**

Update `handle_browse_keys` in `src/ui/events.rs` where TODO comments are:

```rust
(KeyCode::Enter, _) => {
    if !app.number_input.is_empty() {
        if let Ok(num) = app.number_input.parse::<usize>() {
            if num > 0 && num <= app.results.len() {
                app.pending_action = AppAction::Play(num - 1);
            }
        }
        app.number_input.clear();
    } else {
        let global_idx = app.page * app.page_size + app.selected_index;
        app.pending_action = AppAction::Play(global_idx);
    }
}
```

**Step 3: Handle actions in runner**

Update `run_app` in `src/ui/runner.rs`:

```rust
pub fn run_app(
    terminal: &mut Tui,
    mut app: App,
    config: &Config,
    search: &mut PaginatedSearch,
    temp_dir: &Path,
) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| render_ui(f, &app))?;

        // Handle pending actions
        match std::mem::replace(&mut app.pending_action, AppAction::None) {
            AppAction::Play(idx) => {
                if idx < app.results.len() {
                    let result = &app.results[idx];
                    // Exit TUI temporarily for playback
                    let mut terminal_guard = crate::ui::terminal::restore_terminal(terminal)?;

                    println!("{} {}", "Playing:".green(), result.title);
                    crate::display::show_controls(config.player);

                    let _ = crate::player::play_video(
                        config,
                        &result.id,
                        &result.title,
                        &result.safe_title(),
                        temp_dir,
                    );

                    // Re-enter TUI
                    *terminal = crate::ui::terminal::init_terminal()?;
                    println!("\nReturning to search results...");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            AppAction::NewSearch(query) => {
                app.query = query;
                search.reset(&app.query);
                search.ensure_page(0)?;
                app.results = search.results.clone();
                app.total_results = search.results.len();
                app.exhausted = search.exhausted;
                app.page = 0;
                app.selected_index = 0;
            }
            AppAction::FetchNextPage => {
                search.ensure_page(app.page)?;
                app.results = search.results.clone();
                app.total_results = search.results.len();
                app.exhausted = search.exhausted;
            }
            AppAction::None => {}
        }

        let timeout = TICK_RATE
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(&mut app, key);
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
```

Add imports:

```rust
use std::path::Path;
use colored::Colorize;
use crate::config::Config;
use crate::search::PaginatedSearch;
use crate::ui::app::AppAction;
```

**Step 4: Update main.rs to pass dependencies**

Update the TUI section in `src/main.rs`:

```rust
// Run TUI
let result = ui::run_app(&mut terminal, app, &config, &mut search, temp_dir.path());
```

**Step 5: Export AppAction**

Add to `src/ui/mod.rs`:

```rust
pub use app::AppAction;
```

**Step 6: Test play integration**

Run: `cargo run -- "rust tutorials"`
Expected: Arrow to select, Enter plays video, returns to TUI after

**Step 7: Commit**

```bash
git add src/ui/app.rs src/ui/events.rs src/ui/runner.rs src/ui/mod.rs src/main.rs
git commit -m "feat: integrate video playback with TUI"
```

---

### Task 12: Page Fetching Integration

**Files:**
- Modify: `src/ui/events.rs:35-45`

**Step 1: Trigger fetch on page navigation**

Update page navigation in `handle_browse_keys`:

```rust
(KeyCode::Char('n'), _) if app.has_next_page() => {
    let old_page = app.page;
    app.page += 1;
    app.selected_index = 0;

    // Trigger fetch if needed
    let end = (app.page + 1) * app.page_size;
    if end > app.results.len() && !app.exhausted {
        app.pending_action = AppAction::FetchNextPage;
    }
}
```

**Step 2: Test page fetching**

Run: `cargo run -- "rust tutorials"`
Expected: Next page fetches more results when needed

**Step 3: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: auto-fetch when navigating to unfetched pages"
```

---

### Task 13: Search Action Integration

**Files:**
- Modify: `src/ui/events.rs:60-75`

**Step 1: Trigger new search from input mode**

Update `handle_search_keys`:

```rust
fn handle_search_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => {
            app.search_input.push(c);
        }
        KeyCode::Backspace => {
            app.search_input.pop();
        }
        KeyCode::Enter => {
            if !app.search_input.is_empty() {
                app.pending_action = AppAction::NewSearch(app.search_input.clone());
                app.search_input.clear();
                app.input_mode = InputMode::Browse;
            }
        }
        KeyCode::Esc => {
            app.input_mode = InputMode::Browse;
            app.search_input.clear();
        }
        _ => {}
    }
}
```

**Step 2: Test new search**

Run: `cargo run -- "rust tutorials"`
Expected: Press 's', type new query, Enter performs search

**Step 3: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: integrate new search from TUI"
```

---

## Final Tasks

### Task 14: Fix Compilation Errors

**Files:**
- Various

**Step 1: Build and identify errors**

Run: `cargo build 2>&1 | head -50`

**Step 2: Fix each error systematically**

Address imports, type mismatches, lifetime issues one by one.

**Step 3: Verify clean build**

Run: `cargo build`
Expected: Compiles successfully

**Step 4: Commit fixes**

```bash
git add -A
git commit -m "fix: resolve compilation errors"
```

---

### Task 15: End-to-End Testing

**Files:**
- None (testing only)

**Step 1: Test full workflow**

```bash
cargo run -- "rust programming"
```

Test checklist:
- [ ] TUI starts and displays results
- [ ] Arrow keys navigate with visual selection
- [ ] Enter plays video from arrow selection
- [ ] Type number + Enter plays that video
- [ ] Press 's', type query, Enter performs new search
- [ ] Press 'n' loads next page
- [ ] Press 'p' goes to previous page
- [ ] Press 'h' shows help overlay
- [ ] Press 'q' exits cleanly
- [ ] Ctrl-C exits cleanly
- [ ] Terminal restores properly on exit

**Step 2: Test edge cases**

- Resize terminal while running
- Search with no results
- Navigate past last item
- Play last video on page

**Step 3: Document any issues**

Create issues for bugs found.

---

### Task 16: Update README

**Files:**
- Create: `README.md`

**Step 1: Create README**

Create `README.md`:

```markdown
# yt-search-play

A rich terminal UI for searching and playing YouTube videos.

## Features

- 🔍 Interactive search with pagination
- ⌨️  Hybrid navigation (arrow keys + number quick-pick)
- 🎨 Rich TUI with colors, borders, and unicode symbols
- ▶️  Plays videos with mpv, vlc, or mplayer
- 📥 Download mode for permanent storage
- 🎵 Audio-only mode

## Installation

Requires: `yt-dlp` and one of `mpv`, `vlc`, or `mplayer`

```bash
cargo install --path .
```

## Usage

```bash
yt-search-play "search query"
```

### Keyboard Controls

**Navigation:**
- `↑/↓` - Move selection
- `Enter` - Play selected video
- `1-9` - Quick-pick video by number
- `n/p` - Next/Previous page

**Commands:**
- `s` - New search
- `h` - Toggle help
- `q` - Quit

### Options

- `-n, --num <NUM>` - Number of results per page (default: 10)
- `-a, --audio-only` - Audio only mode
- `-f, --format <FMT>` - Video format (default: best)
- `-d, --download` - Download permanently
- `--download-dir <DIR>` - Download directory
- `-i, --include-shorts` - Include YouTube Shorts
- `-k, --keep` - Keep temp files

## License

MIT
```

**Step 2: Commit README**

```bash
git add README.md
git commit -m "docs: add README with usage and features"
```

---

## Implementation Complete

All tasks completed. The rich TUI implementation is ready for testing and review.

**Next steps:**
1. Thorough testing on different terminals
2. User feedback iteration
3. Potential enhancements:
   - ASCII thumbnail previews
   - Playlist/queue support
   - Search history sidebar
   - Real-time playback status from mpv IPC
