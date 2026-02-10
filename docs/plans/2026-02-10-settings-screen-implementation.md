# Settings Screen Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace CLI arguments with in-app settings modal that persists to config file.

**Architecture:** Rewrite Config to use TOML file instead of clap CLI. Add settings modal UI with checkboxes and text fields. Save immediately on every change. Launch with no arguments opens empty search bar.

**Tech Stack:** Rust, ratatui 0.26, serde, toml 0.8, dirs 5.0

---

## Phase 1: Dependencies & Config Foundation (4 tasks)

### Task 1: Add Dependencies

**Files:**
- Modify: `Cargo.toml:12-20`

**Step 1: Add new dependencies**

Add to `Cargo.toml` dependencies:

```toml
serde = { version = "1.0", features = ["derive"] }
toml = "0.8"
dirs = "5.0"
```

**Step 2: Verify build**

Run: `cargo check`
Expected: Downloads dependencies, compiles successfully

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "deps: add serde, toml, dirs for config file support"
```

---

### Task 2: Rewrite Config Module

**Files:**
- Modify: `src/config.rs:1-56` (complete rewrite)

**Step 1: Rewrite Config struct with serde**

Replace entire `src/config.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::fs;
use anyhow::{Context, Result};
use crate::player::PlayerType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip)]
    pub player: PlayerType,

    pub audio_only: bool,
    pub bandwidth_limit: bool,
    pub keep_temp: bool,
    pub include_shorts: bool,
    pub download_mode: bool,
    pub download_dir: String,
    pub results_per_page: usize,
    pub custom_format: String,
}

impl Config {
    pub fn load_or_create() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let contents = fs::read_to_string(&config_path)
                .context("Failed to read config file")?;
            let config: Config = toml::from_str(&contents)
                .context("Failed to parse config file")?;
            Ok(config)
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        fs::create_dir_all(config_path.parent().unwrap())?;
        let toml_string = toml::to_string_pretty(self)?;
        fs::write(config_path, toml_string)?;
        Ok(())
    }

    fn config_path() -> Result<std::path::PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("No config directory found"))?;
        Ok(config_dir.join("yt-search-play/config.toml"))
    }

    pub fn toggle_audio_only(&mut self) -> Result<()> {
        self.audio_only = !self.audio_only;
        self.save()
    }

    pub fn toggle_bandwidth_limit(&mut self) -> Result<()> {
        self.bandwidth_limit = !self.bandwidth_limit;
        self.save()
    }

    pub fn toggle_keep_temp(&mut self) -> Result<()> {
        self.keep_temp = !self.keep_temp;
        self.save()
    }

    pub fn toggle_include_shorts(&mut self) -> Result<()> {
        self.include_shorts = !self.include_shorts;
        self.save()
    }

    pub fn toggle_download_mode(&mut self) -> Result<()> {
        self.download_mode = !self.download_mode;
        self.save()
    }

    pub fn set_download_dir(&mut self, dir: String) -> Result<()> {
        self.download_dir = dir;
        self.save()
    }

    pub fn set_results_per_page(&mut self, num: usize) -> Result<()> {
        self.results_per_page = num.max(1).min(100);
        self.save()
    }

    pub fn set_custom_format(&mut self, format: String) -> Result<()> {
        self.custom_format = format;
        self.save()
    }

    pub fn format(&self) -> String {
        if !self.custom_format.is_empty() {
            self.custom_format.clone()
        } else {
            resolve_format(self.audio_only, self.bandwidth_limit)
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            player: PlayerType::Mpv,
            audio_only: false,
            bandwidth_limit: false,
            keep_temp: false,
            include_shorts: false,
            download_mode: false,
            download_dir: dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("Downloads")
                .to_string_lossy()
                .to_string(),
            results_per_page: 20,
            custom_format: String::new(),
        }
    }
}

fn resolve_format(audio_only: bool, limit: bool) -> String {
    match (audio_only, limit) {
        (true, true) => "bestaudio[abr<=128]/bestaudio/best".to_string(),
        (true, false) => "bestaudio/best".to_string(),
        (false, true) => "bestvideo[height<=360]+bestaudio/best[height<=360]/best".to_string(),
        (false, false) => "bestvideo+bestaudio/best".to_string(),
    }
}
```

**Step 2: Verify build**

Run: `cargo check`
Expected: May show errors about from_cli being removed - that's ok, we'll fix in next task

**Step 3: Commit**

```bash
git add src/config.rs
git commit -m "refactor: rewrite Config to use TOML file instead of CLI"
```

---

### Task 3: Remove CLI Module and Update Main

**Files:**
- Delete: `src/cli.rs`
- Modify: `src/main.rs:1-71`
- Modify: `Cargo.toml:12-20`

**Step 1: Delete cli.rs**

```bash
rm src/cli.rs
```

**Step 2: Remove clap dependency**

Remove from `Cargo.toml`:
```toml
clap = { version = "4.5", features = ["derive"] }
```

**Step 3: Rewrite main.rs**

Replace main function:

```rust
mod cleanup;
mod config;
mod display;
mod player;
mod player_manager;
mod queue;
mod ipc;
mod search;
mod ui;

use anyhow::Result;
use colored::Colorize;

use cleanup::{ManagedTempDir, setup_signal_handler};
use config::Config;
use player::detect_player;
use search::{PaginatedSearch, check_ytdlp};
use ui::app::FocusedPanel;

fn main() -> Result<()> {
    // Load or create config
    let mut config = Config::load_or_create()?;

    // Check dependencies
    check_ytdlp()?;
    let player = detect_player()?;
    config.player = player;

    // Create managed temp dir
    let temp_dir = ManagedTempDir::new(config.keep_temp)?;
    setup_signal_handler();

    // Initialize TUI with empty search
    let terminal = ui::init_terminal()?;
    let mut terminal_guard = ui::TerminalGuard::new(terminal);

    // Create app with empty query, focus on search bar
    let page_size = config.results_per_page;
    let mut app = ui::App::new(String::new(), page_size);
    app.focused_panel = FocusedPanel::SearchBar;
    app.config = config.clone();

    // Search manager with no initial query (will search when user enters query)
    let mut search = PaginatedSearch::new("", page_size, !config.include_shorts);

    // Run TUI
    let result = ui::run_app(terminal_guard.get_mut(), app, &mut config, &mut search, temp_dir.path());

    drop(terminal_guard);
    result
}
```

**Step 4: Verify build**

Run: `cargo check`
Expected: May have errors in App struct - we'll add config field next

**Step 5: Commit**

```bash
git add src/main.rs src/cli.rs Cargo.toml
git commit -m "refactor: remove CLI module and launch without arguments"
```

---

### Task 4: Add Config and Settings Fields to App

**Files:**
- Modify: `src/ui/app.rs:26-65`

**Step 1: Add SettingsField enum**

Add after FocusedPanel:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsField {
    DownloadDir,
    ResultsPerPage,
    CustomFormat,
}
```

**Step 2: Add fields to App struct**

Add to App struct:

```rust
use crate::config::Config;

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
    pub number_input: String,
    pub pending_action: AppAction,
    pub should_quit: bool,
    pub player_manager: Option<PlayerManager>,
    pub queue: Queue,
    pub queue_selected_index: usize,
    pub focused_panel: FocusedPanel,
    pub loading: bool,
    // New fields:
    pub settings_open: bool,
    pub settings_selected_index: usize,
    pub settings_editing: Option<SettingsField>,
    pub config: Config,
}
```

**Step 3: Update App::new() to accept Config**

```rust
impl App {
    pub fn new(query: String, page_size: usize, config: Config) -> Self {
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
            number_input: String::new(),
            pending_action: AppAction::None,
            should_quit: false,
            player_manager: None,
            queue: Queue::new(),
            queue_selected_index: 0,
            focused_panel: FocusedPanel::Results,
            loading: false,
            settings_open: false,
            settings_selected_index: 2,  // Start on first selectable item
            settings_editing: None,
            config,
        }
    }

    // ... rest unchanged
}
```

**Step 4: Update main.rs App::new call**

In `src/main.rs`, change:
```rust
let mut app = ui::App::new(String::new(), page_size, config.clone());
```

**Step 5: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 6: Commit**

```bash
git add src/ui/app.rs src/main.rs
git commit -m "feat: add settings modal state to App"
```

---

## Phase 2: Settings Modal UI (3 tasks)

### Task 5: Add Settings Modal Rendering

**Files:**
- Modify: `src/ui/layout.rs:10-41`
- Modify: `src/ui/layout.rs:end` (add new functions)

**Step 1: Add settings modal overlay to render_ui**

Update `render_ui` to add overlay at the end:

```rust
pub fn render_ui(f: &mut Frame, app: &App) {
    // ... existing layout code ...

    // Overlay help if in help mode
    if app.input_mode == InputMode::Help {
        render_help_overlay(f, app);
    }

    // Overlay settings if open
    if app.settings_open {
        render_settings_modal(f, app);
    }
}
```

**Step 2: Add render_settings_modal function**

Add at end of `src/ui/layout.rs`:

```rust
fn render_settings_modal(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 70, f.size());

    let items = vec![
        ListItem::new(""),
        ListItem::new(Line::from(Span::styled("Playback",
            Style::default().add_modifier(Modifier::BOLD)))),
        ListItem::new("────────"),
        checkbox_item(2, "Audio Only", app.config.audio_only, app.settings_selected_index),
        checkbox_item(3, "Bandwidth Limit (360p video, 128k audio)",
            app.config.bandwidth_limit, app.settings_selected_index),
        checkbox_item(4, "Keep Temporary Files", app.config.keep_temp, app.settings_selected_index),
        checkbox_item(5, "Include YouTube Shorts", app.config.include_shorts, app.settings_selected_index),
        ListItem::new(""),
        ListItem::new(Line::from(Span::styled("Downloads",
            Style::default().add_modifier(Modifier::BOLD)))),
        ListItem::new("─────────"),
        checkbox_item(6, "Download Mode (save permanently)",
            app.config.download_mode, app.settings_selected_index),
        text_field_item(7, "Download Directory", &app.config.download_dir,
            app.settings_selected_index, &app.settings_editing, SettingsField::DownloadDir),
        ListItem::new(""),
        ListItem::new(Line::from(Span::styled("Display",
            Style::default().add_modifier(Modifier::BOLD)))),
        ListItem::new("───────"),
        text_field_item(8, "Results Per Page", &app.config.results_per_page.to_string(),
            app.settings_selected_index, &app.settings_editing, SettingsField::ResultsPerPage),
        ListItem::new(""),
        ListItem::new(Line::from(Span::styled("Advanced",
            Style::default().add_modifier(Modifier::BOLD)))),
        ListItem::new("────────"),
        text_field_item(9, "Custom Format", &app.config.custom_format,
            app.settings_selected_index, &app.settings_editing, SettingsField::CustomFormat),
        ListItem::new(Line::from(Span::styled("(leave empty for auto)",
            Style::default().fg(Color::DarkGray)))),
        ListItem::new(""),
        ListItem::new(Line::from(Span::styled("Press S/F2/Esc to close",
            Style::default().fg(Color::Cyan)))),
    ];

    let block = Block::default()
        .title(" Settings ")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black).fg(Color::White));

    let list = List::new(items).block(block);

    f.render_widget(block.clone(), area);
    f.render_widget(list, area);
}

fn checkbox_item(idx: usize, label: &str, checked: bool, selected: usize) -> ListItem {
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
                   editing: &Option<SettingsField>, field: SettingsField) -> ListItem {
    let is_editing = matches!(editing, Some(f) if *f == field) && idx == selected;
    let cursor = if is_editing { "█" } else { "" };
    let text = format!("  {}: [{}{}]", label, value, cursor);
    let style = if idx == selected {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default().fg(Color::White)
    };
    ListItem::new(text).style(style)
}
```

**Step 3: Add SettingsField import**

Add to top of `src/ui/layout.rs`:

```rust
use crate::ui::app::{App, FocusedPanel, InputMode, SettingsField};
```

**Step 4: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 5: Commit**

```bash
git add src/ui/layout.rs
git commit -m "feat: add settings modal rendering with checkboxes and text fields"
```

---

### Task 6: Add Settings Toggle Key

**Files:**
- Modify: `src/ui/events.rs:4-19`

**Step 1: Add settings toggle in handle_key_event**

Update the function to check for settings key first:

```rust
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // Global settings toggle (works except in Help mode)
    if app.input_mode != InputMode::Help {
        if matches!(key.code, KeyCode::Char('S') | KeyCode::F(2)) {
            app.settings_open = !app.settings_open;
            return;
        }
    }

    // Handle settings modal input
    if app.settings_open {
        handle_settings_keys(app, key);
        return;
    }

    // Global Tab key for focus cycling (works in any mode except Help)
    if app.input_mode != InputMode::Help && key.code == KeyCode::Tab {
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            cycle_focus_backward(app);
        } else {
            cycle_focus_forward(app);
        }
        return;
    }

    match app.input_mode {
        InputMode::Browse => handle_browse_keys(app, key),
        InputMode::Help => handle_help_keys(app, key),
    }
}
```

**Step 2: Add stub handle_settings_keys**

Add at end of file:

```rust
fn handle_settings_keys(app: &mut App, key: KeyEvent) {
    if app.settings_editing.is_some() {
        handle_settings_edit_keys(app, key);
    } else {
        handle_settings_nav_keys(app, key);
    }
}

fn handle_settings_nav_keys(app: &mut App, key: KeyEvent) {
    // TODO: Implement in next task
}

fn handle_settings_edit_keys(app: &mut App, key: KeyEvent) {
    // TODO: Implement in next task
}
```

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Test manually**

Run: `cargo run`
Expected: Press 'S', modal appears (even if navigation doesn't work yet)

**Step 5: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: add S/F2 key to toggle settings modal"
```

---

### Task 7: Implement Settings Navigation

**Files:**
- Modify: `src/ui/events.rs:handle_settings_nav_keys`

**Step 1: Implement navigation logic**

Replace `handle_settings_nav_keys`:

```rust
fn handle_settings_nav_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Up => {
            loop {
                if app.settings_selected_index > 0 {
                    app.settings_selected_index -= 1;
                    if is_selectable_setting(app.settings_selected_index) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        KeyCode::Down => {
            loop {
                if app.settings_selected_index < 9 {
                    app.settings_selected_index += 1;
                    if is_selectable_setting(app.settings_selected_index) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            handle_settings_activate(app);
        }
        KeyCode::Esc | KeyCode::Char('S') | KeyCode::F(2) => {
            app.settings_open = false;
        }
        _ => {}
    }
}

fn is_selectable_setting(idx: usize) -> bool {
    // Selectable: 2-9 (skips headers, blank lines, help text)
    matches!(idx, 2..=9)
}

fn handle_settings_activate(app: &mut App) {
    use crate::ui::app::SettingsField;

    match app.settings_selected_index {
        2 => { let _ = app.config.toggle_audio_only(); }
        3 => { let _ = app.config.toggle_bandwidth_limit(); }
        4 => { let _ = app.config.toggle_keep_temp(); }
        5 => { let _ = app.config.toggle_include_shorts(); }
        6 => { let _ = app.config.toggle_download_mode(); }
        7 => { app.settings_editing = Some(SettingsField::DownloadDir); }
        8 => { app.settings_editing = Some(SettingsField::ResultsPerPage); }
        9 => { app.settings_editing = Some(SettingsField::CustomFormat); }
        _ => {}
    }
}
```

**Step 2: Add SettingsField import**

Add to imports at top:

```rust
use crate::ui::app::{App, AppAction, FocusedPanel, InputMode, SettingsField};
```

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: implement settings navigation and checkbox toggles"
```

---

## Phase 3: Text Field Editing (2 tasks)

### Task 8: Implement Text Field Editing

**Files:**
- Modify: `src/ui/events.rs:handle_settings_edit_keys`

**Step 1: Implement edit mode handling**

Replace `handle_settings_edit_keys`:

```rust
fn handle_settings_edit_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => {
            match &app.settings_editing {
                Some(SettingsField::DownloadDir) => {
                    app.config.download_dir.push(c);
                    let _ = app.config.save();
                }
                Some(SettingsField::ResultsPerPage) => {
                    if c.is_ascii_digit() {
                        let mut val = app.config.results_per_page.to_string();
                        if val == "0" {
                            val.clear();
                        }
                        val.push(c);
                        if let Ok(num) = val.parse::<usize>() {
                            let _ = app.config.set_results_per_page(num);
                        }
                    }
                }
                Some(SettingsField::CustomFormat) => {
                    app.config.custom_format.push(c);
                    let _ = app.config.save();
                }
                None => {}
            }
        }
        KeyCode::Backspace => {
            match &app.settings_editing {
                Some(SettingsField::DownloadDir) => {
                    app.config.download_dir.pop();
                    let _ = app.config.save();
                }
                Some(SettingsField::ResultsPerPage) => {
                    let mut val = app.config.results_per_page.to_string();
                    val.pop();
                    if val.is_empty() {
                        app.config.results_per_page = 0;
                    } else {
                        app.config.results_per_page = val.parse().unwrap_or(20);
                    }
                    let _ = app.config.save();
                }
                Some(SettingsField::CustomFormat) => {
                    app.config.custom_format.pop();
                    let _ = app.config.save();
                }
                None => {}
            }
        }
        KeyCode::Enter | KeyCode::Esc => {
            // Exit edit mode
            app.settings_editing = None;
        }
        _ => {}
    }
}
```

**Step 2: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: implement text field editing in settings modal"
```

---

### Task 9: Update Help Overlay with Settings Key

**Files:**
- Modify: `src/ui/layout.rs:262-310`

**Step 1: Update help text**

Update `render_help_overlay` to add settings key:

```rust
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
    Line::from("Other:"),
    Line::from("  S / F2      - Settings"),
    Line::from("  h           - Toggle this help"),
    Line::from("  q/Esc       - Quit"),
    Line::from("  Ctrl+C      - Force quit"),
];
```

**Step 2: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src/ui/layout.rs
git commit -m "docs: add settings key to help overlay"
```

---

## Phase 4: Integration & Fixes (3 tasks)

### Task 10: Update Runner to Apply Config Changes

**Files:**
- Modify: `src/ui/runner.rs:72-88`

**Step 1: Update NewSearch action to respect config**

Modify NewSearch handler:

```rust
AppAction::NewSearch(query) => {
    // Clear old results immediately
    app.results.clear();
    app.total_results = 0;
    app.page = 0;
    app.selected_index = 0;

    // Update search with new filters from config
    let filter_shorts = !app.config.include_shorts;
    let page_size = app.config.results_per_page;

    // Perform search
    search.filter_shorts = filter_shorts;
    search.page_size = page_size;
    search.reset(&query);
    search.ensure_page(0)?;

    // Update app with new page size if changed
    app.page_size = page_size;

    // Update with new results
    app.results = search.results.clone();
    app.total_results = search.results.len();
    app.exhausted = search.exhausted;
    app.loading = false;
}
```

**Step 2: Update FetchNextPage to respect page size**

Modify FetchNextPage handler:

```rust
AppAction::FetchNextPage => {
    app.page_size = app.config.results_per_page;
    search.page_size = app.config.results_per_page;
    search.ensure_page(app.page)?;
    app.results = search.results.clone();
    app.total_results = search.results.len();
    app.exhausted = search.exhausted;
}
```

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ui/runner.rs
git commit -m "feat: apply config changes to search behavior dynamically"
```

---

### Task 11: Make PaginatedSearch Fields Public

**Files:**
- Modify: `src/search.rs:55-64`

**Step 1: Change fields to pub**

Update struct definition:

```rust
pub struct PaginatedSearch {
    query: String,
    pub page_size: usize,
    pub filter_shorts: bool,
    /// All results that have passed filtering so far.
    pub results: Vec<SearchResult>,
    /// How many raw yt-dlp playlist items we have consumed (1-indexed high-water mark).
    raw_cursor: usize,
    /// No more results available from yt-dlp.
    pub exhausted: bool,
}
```

**Step 2: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src/search.rs
git commit -m "refactor: make PaginatedSearch fields public for config updates"
```

---

### Task 12: Fix Compilation Errors

**Files:**
- Various

**Step 1: Build and identify errors**

Run: `cargo build 2>&1 | head -80`

**Step 2: Fix each error systematically**

Common issues to expect:
- Missing imports for Config in files
- PlayerType not having serde derives
- Config.format vs Config.format() method confusion
- Missing dirs crate usage

**Step 3: Verify clean build**

Run: `cargo build`
Expected: Compiles successfully with only warnings

**Step 4: Commit fixes**

```bash
git add -A
git commit -m "fix: resolve compilation errors after config refactor"
```

---

## Phase 5: Testing & Polish (3 tasks)

### Task 13: Add Config Tests

**Files:**
- Create: `src/config.rs:tests` (add to end of file)

**Step 1: Add test module**

Add to end of `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.audio_only, false);
        assert_eq!(config.bandwidth_limit, false);
        assert_eq!(config.results_per_page, 20);
        assert_eq!(config.custom_format, "");
    }

    #[test]
    fn test_format_resolution() {
        let config = Config::default();
        let format = config.format();
        assert_eq!(format, "bestvideo+bestaudio/best");
    }

    #[test]
    fn test_format_resolution_audio_only() {
        let mut config = Config::default();
        config.audio_only = true;
        let format = config.format();
        assert_eq!(format, "bestaudio/best");
    }

    #[test]
    fn test_format_resolution_bandwidth_limit() {
        let mut config = Config::default();
        config.bandwidth_limit = true;
        let format = config.format();
        assert!(format.contains("height<=360"));
    }

    #[test]
    fn test_custom_format_override() {
        let mut config = Config::default();
        config.custom_format = "worst".to_string();
        let format = config.format();
        assert_eq!(format, "worst");
    }
}
```

**Step 2: Run tests**

Run: `cargo test config::tests`
Expected: All tests pass

**Step 3: Commit**

```bash
git add src/config.rs
git commit -m "test: add config tests for defaults and format resolution"
```

---

### Task 14: Add Settings Navigation Tests

**Files:**
- Modify: `src/ui/events.rs:tests` (add to existing test module)

**Step 1: Add test helpers**

Add to test module:

```rust
fn create_test_app_with_config() -> App {
    let config = crate::config::Config::default();
    App::new("test".to_string(), 10, config)
}

#[test]
fn test_settings_nav_down() {
    let mut app = create_test_app_with_config();
    app.settings_open = true;
    app.settings_selected_index = 2;

    handle_settings_nav_keys(&mut app, KeyEvent::from(KeyCode::Down));
    assert_eq!(app.settings_selected_index, 3);
}

#[test]
fn test_settings_nav_up() {
    let mut app = create_test_app_with_config();
    app.settings_open = true;
    app.settings_selected_index = 3;

    handle_settings_nav_keys(&mut app, KeyEvent::from(KeyCode::Up));
    assert_eq!(app.settings_selected_index, 2);
}

#[test]
fn test_settings_toggle_checkbox() {
    let mut app = create_test_app_with_config();
    app.settings_open = true;
    app.settings_selected_index = 2; // Audio Only

    let initial = app.config.audio_only;
    handle_settings_nav_keys(&mut app, KeyEvent::from(KeyCode::Enter));
    assert_ne!(app.config.audio_only, initial);
}

#[test]
fn test_settings_close_on_esc() {
    let mut app = create_test_app_with_config();
    app.settings_open = true;

    handle_settings_nav_keys(&mut app, KeyEvent::from(KeyCode::Esc));
    assert!(!app.settings_open);
}
```

**Step 2: Run tests**

Run: `cargo test ui::events::tests`
Expected: All tests pass

**Step 3: Commit**

```bash
git add src/ui/events.rs
git commit -m "test: add settings modal navigation tests"
```

---

### Task 15: End-to-End Testing

**Files:**
- None (testing only)

**Step 1: Test full workflow**

```bash
cargo run
```

Test checklist:
- [ ] App launches with empty search bar (focused)
- [ ] Type query, Enter searches
- [ ] Press 'S', settings modal opens
- [ ] Arrow keys navigate settings
- [ ] Space toggles checkboxes
- [ ] Enter on text field enters edit mode
- [ ] Type in text fields updates value
- [ ] Backspace deletes characters
- [ ] Enter exits edit mode
- [ ] Esc closes modal
- [ ] Config file created at ~/.config/yt-search-play/config.toml
- [ ] Settings persist after restart
- [ ] Changing results_per_page affects next search
- [ ] Changing include_shorts filters results

**Step 2: Test config file**

```bash
cat ~/.config/yt-search-play/config.toml
```

Expected: Valid TOML with saved settings

**Step 3: Test config persistence**

- Open app, change setting, quit
- Reopen app, verify setting persisted
- Edit config file manually, reopen app, verify loaded

**Step 4: Document any issues**

If bugs found, create follow-up tasks.

**Step 5: Final commit**

```bash
git add -A
git commit -m "test: verify settings screen end-to-end workflow"
```

---

## Phase 6: Cleanup (2 tasks)

### Task 16: Update README

**Files:**
- Modify: `README.md:17-52`

**Step 1: Update Installation and Usage sections**

Replace Usage section:

```markdown
## Usage

Launch without arguments:

```bash
yt-search-play
```

The app opens with an empty search bar. Type your query and press Enter to search.

### Keyboard Controls

**Focus Navigation:**
- `Tab` - Cycle focus (Search → Results → Queue)
- `Shift+Tab` - Reverse cycle

**Search Bar (when focused):**
- Type - Enter query
- `Enter` - Submit search
- `Esc` - Clear and return to Results

**Results (when focused):**
- `↑/↓` - Move selection
- `Enter` - Add to queue and play
- `1-9` - Quick-pick by number
- `n/p` - Next/Previous page
- `s` - Focus search bar

**Queue (when focused):**
- `↑/↓` - Navigate queue
- `Enter` - Jump to track
- `Del/Backspace` - Remove track
- `c` - Clear queue

**Playback (global):**
- `Space` - Play/Pause
- `n` - Next track
- `</>` - Seek ±10 seconds
- `+/-` - Volume up/down
- `m` - Mute toggle

**Other:**
- `S` or `F2` - Settings
- `h` - Toggle help
- `q` - Quit

### Settings

Press `S` or `F2` to open the settings modal. All settings are saved immediately to `~/.config/yt-search-play/config.toml`.

**Available Settings:**
- Audio Only - Play audio only (no video)
- Bandwidth Limit - Limit to 360p video and 128k audio
- Keep Temporary Files - Don't delete cached files after playback
- Include YouTube Shorts - Include videos under 3 minutes
- Download Mode - Save videos permanently to download directory
- Download Directory - Where to save permanent downloads
- Results Per Page - Number of search results per page (1-100)
- Custom Format - Custom yt-dlp format string (leave empty for auto)
```

**Step 2: Remove Options section**

Delete the old Options section that listed CLI flags.

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: update README for settings screen"
```

---

### Task 17: Build and Install

**Files:**
- None (build only)

**Step 1: Clean build**

```bash
cargo clean
cargo build --release
```

Expected: Clean build with no errors

**Step 2: Install to ~/bin**

```bash
cp target/release/yt-search-play ~/bin/
```

**Step 3: Test fresh install**

```bash
rm -f ~/.config/yt-search-play/config.toml
~/bin/yt-search-play
```

Expected:
- Config file auto-created
- Empty search bar appears
- Settings modal works

**Step 4: Final commit**

```bash
git add -A
git commit -m "chore: final build and install"
```

---

## Implementation Complete

All 17 tasks completed. Settings screen replaces CLI arguments, config persists to TOML file, and app launches with empty search bar.

**Next steps:**
1. User testing and feedback
2. Consider adding config migration for existing users
3. Future enhancement: Import/export settings
