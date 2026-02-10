# Background Playback & Persistent Search Bar Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add background audio playback with queue management and persistent search bar to yt-search-play TUI.

**Architecture:** Spawn mpv with IPC socket for real-time status updates. Queue system with VecDeque auto-advances tracks. 4-panel layout with Tab focus cycling between search bar, results, and queue.

**Tech Stack:** Rust, ratatui 0.26, crossterm 0.27, serde_json, std::os::unix::net::UnixStream

---

## Phase 1: Foundation (5 tasks)

### Task 1: Add serde_json Dependency

**Files:**
- Modify: `Cargo.toml:12-15`

**Step 1: Add dependency**

Add to `Cargo.toml` dependencies section:

```toml
serde_json = "1.0"
```

**Step 2: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "deps: add serde_json for mpv IPC protocol"
```

---

### Task 2: Create IPC Module Stub

**Files:**
- Create: `src/ipc.rs`
- Modify: `src/main.rs:1-7` (add mod declaration)

**Step 1: Create module file**

Create `src/ipc.rs`:

```rust
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;
use anyhow::{Context, Result, bail};
use serde_json::{json, Value};

pub struct IpcClient {
    stream: UnixStream,
}

impl IpcClient {
    pub fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .context("Failed to connect to IPC socket")?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_write_timeout(Some(Duration::from_millis(100)))?;
        Ok(Self { stream })
    }

    pub fn send_command(&mut self, command: &[&str]) -> Result<()> {
        let json_cmd = json!({ "command": command });
        let mut cmd_str = serde_json::to_string(&json_cmd)?;
        cmd_str.push('\n');
        self.stream.write_all(cmd_str.as_bytes())?;
        Ok(())
    }

    pub fn get_property(&mut self, property: &str) -> Result<Value> {
        self.send_command(&["get_property", property])?;
        let mut reader = BufReader::new(&self.stream);
        let mut response = String::new();
        reader.read_line(&mut response)?;
        let json: Value = serde_json::from_str(&response)?;

        if json["error"].as_str() != Some("success") {
            bail!("Property unavailable: {}", property);
        }

        Ok(json["data"].clone())
    }
}
```

**Step 2: Add mod declaration**

Add to `src/main.rs` after other mod declarations:

```rust
mod ipc;
```

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ipc.rs src/main.rs
git commit -m "feat: add IPC module for mpv socket communication"
```

---

### Task 3: Create Queue Module

**Files:**
- Create: `src/queue.rs`
- Modify: `src/main.rs:1-7`

**Step 1: Create queue module**

Create `src/queue.rs`:

```rust
use std::collections::VecDeque;
use crate::search::SearchResult;

pub struct Queue {
    tracks: VecDeque<SearchResult>,
    pub selected_index: usize,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            tracks: VecDeque::new(),
            selected_index: 0,
        }
    }

    pub fn push_back(&mut self, track: SearchResult) {
        self.tracks.push_back(track);
    }

    pub fn pop_front(&mut self) -> Option<SearchResult> {
        if !self.tracks.is_empty() && self.selected_index > 0 {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
        self.tracks.pop_front()
    }

    pub fn remove(&mut self, index: usize) -> Option<SearchResult> {
        if index >= self.tracks.len() {
            return None;
        }
        if index < self.selected_index {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
        self.tracks.remove(index)
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.selected_index = 0;
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&SearchResult> {
        self.tracks.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SearchResult> {
        self.tracks.iter()
    }

    pub fn move_to_front(&mut self, index: usize) {
        if index < self.tracks.len() && index > 0 {
            let track = self.tracks.remove(index).unwrap();
            self.tracks.push_front(track);
            self.selected_index = 0;
        }
    }
}
```

**Step 2: Add mod declaration**

Add to `src/main.rs`:

```rust
mod queue;
```

**Step 3: Write tests**

Add to end of `src/queue.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_track(id: &str, title: &str) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: title.to_string(),
            duration: "3:00".to_string(),
            channel: "Test".to_string(),
            views: "1K".to_string(),
        }
    }

    #[test]
    fn test_push_and_len() {
        let mut queue = Queue::new();
        assert_eq!(queue.len(), 0);

        queue.push_back(create_test_track("1", "Track 1"));
        assert_eq!(queue.len(), 1);

        queue.push_back(create_test_track("2", "Track 2"));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_pop_front() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));

        let track = queue.pop_front().unwrap();
        assert_eq!(track.id, "1");
        assert_eq!(queue.len(), 1);

        let track = queue.pop_front().unwrap();
        assert_eq!(track.id, "2");
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_remove() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));

        let track = queue.remove(1).unwrap();
        assert_eq!(track.id, "2");
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_clear() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));

        queue.clear();
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }
}
```

**Step 4: Run tests**

Run: `cargo test queue::tests`
Expected: All tests pass

**Step 5: Commit**

```bash
git add src/queue.rs src/main.rs
git commit -m "feat: add queue module with VecDeque and tests"
```

---

### Task 4: Create PlayerManager Stub

**Files:**
- Create: `src/player_manager.rs`
- Modify: `src/main.rs:1-7`

**Step 1: Create player manager stub**

Create `src/player_manager.rs`:

```rust
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use anyhow::{Context, Result, bail};
use crate::ipc::IpcClient;
use crate::config::Config;

pub struct PlayerManager {
    process: Child,
    socket_path: PathBuf,
    ipc: Option<IpcClient>,
    pub status: PlaybackStatus,
}

#[derive(Debug, Clone)]
pub struct PlaybackStatus {
    pub playing: bool,
    pub paused: bool,
    pub time_pos: f64,
    pub duration: f64,
    pub volume: i32,
    pub title: String,
    pub eof_reached: bool,
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        Self {
            playing: false,
            paused: false,
            time_pos: 0.0,
            duration: 0.0,
            volume: 100,
            title: String::new(),
            eof_reached: false,
        }
    }
}

impl PlayerManager {
    pub fn new(config: &Config) -> Result<Self> {
        let socket_path = PathBuf::from(format!(
            "/tmp/yt-search-play-{}.sock",
            std::process::id()
        ));

        let mut cmd = Command::new("mpv");
        cmd.arg("--idle")
            .arg(format!("--input-ipc-server={}", socket_path.display()))
            .arg("--no-video")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let process = cmd.spawn()
            .context("Failed to spawn mpv process")?;

        Ok(Self {
            process,
            socket_path,
            ipc: None,
            status: PlaybackStatus::default(),
        })
    }

    pub fn connect(&mut self) -> Result<()> {
        // Wait for socket to exist (max 2 seconds)
        let start = Instant::now();
        while !self.socket_path.exists() {
            if start.elapsed() > Duration::from_secs(2) {
                bail!("mpv socket not created after 2 seconds");
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        self.ipc = Some(IpcClient::connect(&self.socket_path)?);
        Ok(())
    }

    pub fn play(&mut self, url: &str, title: &str) -> Result<()> {
        if self.ipc.is_none() {
            self.connect()?;
        }

        let ipc = self.ipc.as_mut().unwrap();
        ipc.send_command(&["loadfile", url])?;

        self.status.title = title.to_string();
        self.status.playing = true;
        self.status.paused = false;
        self.status.eof_reached = false;

        Ok(())
    }

    pub fn toggle_pause(&mut self) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["cycle", "pause"])?;
            self.status.paused = !self.status.paused;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["stop"])?;
            self.status = PlaybackStatus::default();
        }
        Ok(())
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["seek", &seconds.to_string(), "relative"])?;
        }
        Ok(())
    }

    pub fn set_volume(&mut self, volume: i32) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["set_property", "volume", &volume.to_string()])?;
            self.status.volume = volume.clamp(0, 100);
        }
        Ok(())
    }

    pub fn update_status(&mut self) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            // Get time position
            if let Ok(val) = ipc.get_property("time-pos") {
                if let Some(time) = val.as_f64() {
                    self.status.time_pos = time;
                }
            }

            // Get duration
            if let Ok(val) = ipc.get_property("duration") {
                if let Some(dur) = val.as_f64() {
                    self.status.duration = dur;
                }
            }

            // Get pause state
            if let Ok(val) = ipc.get_property("pause") {
                if let Some(paused) = val.as_bool() {
                    self.status.paused = paused;
                }
            }

            // Get volume
            if let Ok(val) = ipc.get_property("volume") {
                if let Some(vol) = val.as_f64() {
                    self.status.volume = vol as i32;
                }
            }

            // Check EOF
            if let Ok(val) = ipc.get_property("eof-reached") {
                if let Some(eof) = val.as_bool() {
                    self.status.eof_reached = eof;
                }
            }
        }
        Ok(())
    }

    pub fn is_eof(&self) -> bool {
        self.status.eof_reached
    }
}

impl Drop for PlayerManager {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
```

**Step 2: Add mod declaration**

Add to `src/main.rs`:

```rust
mod player_manager;
```

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/player_manager.rs src/main.rs
git commit -m "feat: add PlayerManager with mpv IPC integration"
```

---

### Task 5: Add FocusedPanel and Fields to App

**Files:**
- Modify: `src/ui/app.rs:1-74`

**Step 1: Add FocusedPanel enum**

Add to `src/ui/app.rs` after InputMode:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusedPanel {
    SearchBar,
    Results,
    Queue,
}
```

**Step 2: Add fields to App struct**

Modify App struct to add:

```rust
use crate::player_manager::PlayerManager;
use crate::queue::Queue;

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
    pub playback_state: PlaybackState,
    pub pending_action: AppAction,
    pub should_quit: bool,
    // New fields:
    pub player_manager: Option<PlayerManager>,
    pub queue: Queue,
    pub queue_selected_index: usize,
    pub focused_panel: FocusedPanel,
}
```

**Step 3: Initialize new fields in new()**

Update `App::new()`:

```rust
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
        number_input: String::new(),
        playback_state: PlaybackState::Idle,
        pending_action: AppAction::None,
        should_quit: false,
        player_manager: None,
        queue: Queue::new(),
        queue_selected_index: 0,
        focused_panel: FocusedPanel::Results,
    }
}
```

**Step 4: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 5: Commit**

```bash
git add src/ui/app.rs
git commit -m "feat: add FocusedPanel, PlayerManager, Queue fields to App"
```

---

## Phase 2: UI Layout (3 tasks)

### Task 6: Add Persistent Search Bar at Top

**Files:**
- Modify: `src/ui/layout.rs:10-58`

**Step 1: Update render_ui layout**

Replace `render_ui` function in `src/ui/layout.rs`:

```rust
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
}
```

**Step 2: Add render_search_bar function**

Add after `render_ui`:

```rust
fn render_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == FocusedPanel::SearchBar;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let display_text = if app.focused_panel == FocusedPanel::SearchBar {
        format!("{}█", app.search_input)
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
```

**Step 3: Add render_main_content stub**

Add after `render_search_bar`:

```rust
fn render_main_content(f: &mut Frame, app: &App, area: Rect) {
    // Split into results (70%) and queue (30%)
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
            let prefix = if Some(i) == Some(0) && app.player_manager.is_some() {
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
```

**Step 4: Update render_results to respect focus**

Modify `render_results` to check `FocusedPanel::Results`:

```rust
fn render_results(f: &mut Frame, app: &App, area: Rect) {
    let results = app.current_page_results();
    let start_idx = app.page * app.page_size;

    let is_focused = app.focused_panel == FocusedPanel::Results;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

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
        format!("Page {}/{} • {} total", app.page + 1, total_pages, app.total_results)
    } else {
        format!("Page {} • {}+ loaded", app.page + 1, app.total_results)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Search Results ({}) ", page_info))
        .border_style(border_style);

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

    let mut state = ListState::default();
    state.select(Some(app.selected_index));

    f.render_stateful_widget(list, area, &mut state);
}
```

**Step 5: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 6: Commit**

```bash
git add src/ui/layout.rs
git commit -m "feat: add persistent search bar and queue panel to layout"
```

---

### Task 7: Add Status Bar with Progress

**Files:**
- Modify: `src/ui/layout.rs:133-150`

**Step 1: Rewrite render_footer**

Replace `render_footer` function:

```rust
fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let has_playback = app.player_manager.is_some();

    if has_playback {
        // Split into status line + controls line
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
        let progress_width = area.width.saturating_sub(60);
        let progress = if status.duration > 0.0 {
            (status.time_pos / status.duration * progress_width as f64) as u16
        } else {
            0
        };

        let filled = "█".repeat(progress as usize);
        let empty = "░".repeat((progress_width - progress) as usize);

        let icon = if status.paused { "⏸" } else { "▶" };
        let elapsed = format_duration(status.time_pos as u64);
        let duration = format_duration(status.duration as u64);

        let text = format!(
            "{} Now Playing: {} [{}{}] {} / {}  Vol:{}",
            icon, status.title, filled, empty, elapsed, duration, status.volume
        );

        let status_bar = Paragraph::new(text)
            .style(Style::default().bg(Color::DarkGray).fg(Color::White));

        f.render_widget(status_bar, area);
    }
}

fn render_controls_line(f: &mut Frame, app: &App, area: Rect) {
    let text = match app.focused_panel {
        FocusedPanel::SearchBar => {
            "Enter: Search • Esc: Cancel • Tab: Switch Panel".to_string()
        }
        FocusedPanel::Results => {
            "↑/↓: Navigate • Enter: Queue • Space: Pause • Tab: Switch • n/p: Next/Prev • q: Quit".to_string()
        }
        FocusedPanel::Queue => {
            "↑/↓: Navigate • Enter: Jump • Del: Remove • c: Clear • Tab: Switch Panel".to_string()
        }
    };

    let footer = Paragraph::new(text)
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::TOP | Borders::LEFT | Borders::RIGHT));

    f.render_widget(footer, area);
}
```

**Step 2: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src/ui/layout.rs
git commit -m "feat: add status bar with progress and dynamic controls"
```

---

### Task 8: Remove render_header (Replaced by Search Bar)

**Files:**
- Modify: `src/ui/layout.rs:48-58`

**Step 1: Delete render_header function**

Remove the old `render_header` function completely (lines ~48-58).

**Step 2: Remove render_status_bar (old version)**

Remove old `render_status_bar` function if it still exists (was for PlaybackState::Playing).

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ui/layout.rs
git commit -m "refactor: remove old header and status bar functions"
```

---

## Phase 3: Focus Management (2 tasks)

### Task 9: Add Tab Focus Cycling

**Files:**
- Modify: `src/ui/events.rs:10-16`

**Step 1: Add focus cycling handler**

Add to `handle_key_event` at the top, before mode dispatch:

```rust
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // Global Tab key for focus cycling
    if key.code == KeyCode::Tab {
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            cycle_focus_backward(app);
        } else {
            cycle_focus_forward(app);
        }
        return;
    }

    match app.input_mode {
        InputMode::Browse => handle_browse_keys(app, key),
        InputMode::Search => handle_search_keys(app, key),
        InputMode::Help => handle_help_keys(app, key),
    }
}

fn cycle_focus_forward(app: &mut App) {
    use crate::ui::app::FocusedPanel;
    app.focused_panel = match app.focused_panel {
        FocusedPanel::SearchBar => FocusedPanel::Results,
        FocusedPanel::Results => FocusedPanel::Queue,
        FocusedPanel::Queue => FocusedPanel::SearchBar,
    };
}

fn cycle_focus_backward(app: &mut App) {
    use crate::ui::app::FocusedPanel;
    app.focused_panel = match app.focused_panel {
        FocusedPanel::SearchBar => FocusedPanel::Queue,
        FocusedPanel::Queue => FocusedPanel::Results,
        FocusedPanel::Results => FocusedPanel::SearchBar,
    };
}
```

**Step 2: Add import**

Add to top of `src/ui/events.rs`:

```rust
use crate::ui::app::FocusedPanel;
```

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: add Tab key focus cycling between panels"
```

---

### Task 10: Route Keys Based on Focus

**Files:**
- Modify: `src/ui/events.rs:18-109`

**Step 1: Update handle_browse_keys to check focus**

Wrap the entire `handle_browse_keys` function body:

```rust
fn handle_browse_keys(app: &mut App, key: KeyEvent) {
    // Global quit keys work from any panel
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            app.should_quit = true;
            return;
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.should_quit = true;
            return;
        }
        _ => {}
    }

    match app.focused_panel {
        FocusedPanel::SearchBar => handle_search_bar_keys(app, key),
        FocusedPanel::Results => handle_results_keys(app, key),
        FocusedPanel::Queue => handle_queue_keys(app, key),
    }
}
```

**Step 2: Rename and extract handle_results_keys**

Create new function from old `handle_browse_keys` logic:

```rust
fn handle_results_keys(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
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

            let end = (app.page + 1) * app.page_size;
            if end > app.results.len() && !app.exhausted {
                app.pending_action = AppAction::FetchNextPage;
            }
        }
        (KeyCode::Char('p'), _) if app.has_prev_page() => {
            app.page -= 1;
            app.selected_index = 0;
        }
        (KeyCode::Char('h'), _) => {
            app.input_mode = InputMode::Help;
        }
        (KeyCode::Char('s'), _) => {
            app.focused_panel = FocusedPanel::SearchBar;
        }
        (KeyCode::Char(c), _) if c.is_ascii_digit() => {
            app.number_input.push(c);
        }
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
        (KeyCode::Backspace, _) => {
            app.number_input.pop();
        }
        _ => {}
    }
}
```

**Step 3: Add handle_search_bar_keys**

```rust
fn handle_search_bar_keys(app: &mut App, key: KeyEvent) {
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
                app.focused_panel = FocusedPanel::Results;
            }
        }
        KeyCode::Esc => {
            app.search_input.clear();
            app.focused_panel = FocusedPanel::Results;
        }
        _ => {}
    }
}
```

**Step 4: Add handle_queue_keys stub**

```rust
fn handle_queue_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Up => {
            if app.queue_selected_index > 0 {
                app.queue_selected_index -= 1;
            }
        }
        KeyCode::Down => {
            if app.queue_selected_index < app.queue.len().saturating_sub(1) {
                app.queue_selected_index += 1;
            }
        }
        KeyCode::Enter => {
            // Jump to selected track (implemented in Phase 5)
        }
        KeyCode::Delete | KeyCode::Backspace => {
            app.queue.remove(app.queue_selected_index);
            if app.queue_selected_index >= app.queue.len() && app.queue_selected_index > 0 {
                app.queue_selected_index -= 1;
            }
        }
        KeyCode::Char('c') => {
            app.queue.clear();
            app.queue_selected_index = 0;
        }
        _ => {}
    }
}
```

**Step 5: Remove old handle_search_keys**

Delete the old `handle_search_keys` function (now replaced by `handle_search_bar_keys`).

**Step 6: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 7: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: route keys based on focused panel"
```

---

## Phase 4: Playback Integration (4 tasks)

### Task 11: Connect Enter to Queue and Play

**Files:**
- Modify: `src/ui/events.rs:60-70` (handle_results_keys Enter case)
- Modify: `src/ui/runner.rs:30-80`

**Step 1: Update Play action to queue track**

Modify `handle_results_keys` Enter handler:

```rust
(KeyCode::Enter, _) => {
    let idx = if !app.number_input.is_empty() {
        if let Ok(num) = app.number_input.parse::<usize>() {
            app.number_input.clear();
            if num > 0 && num <= app.results.len() {
                Some(num - 1)
            } else {
                None
            }
        } else {
            app.number_input.clear();
            None
        }
    } else {
        Some(app.page * app.page_size + app.selected_index)
    };

    if let Some(idx) = idx {
        if idx < app.results.len() {
            app.pending_action = AppAction::Play(idx);
        }
    }
}
```

**Step 2: Update Play action handler in runner**

Modify `src/ui/runner.rs` Play action:

```rust
AppAction::Play(idx) => {
    if idx < app.results.len() {
        let result = &app.results[idx].clone();

        // Add to queue
        app.queue.push_back(result.clone());

        // If no player, create one and start playing
        if app.player_manager.is_none() {
            match PlayerManager::new(config) {
                Ok(mut pm) => {
                    let url = format!("https://www.youtube.com/watch?v={}", result.id);
                    if pm.play(&url, &result.title).is_ok() {
                        app.player_manager = Some(pm);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to create player: {}", e);
                }
            }
        }
    }
}
```

**Step 3: Add imports to runner**

Add to `src/ui/runner.rs`:

```rust
use crate::player_manager::PlayerManager;
```

**Step 4: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 5: Commit**

```bash
git add src/ui/events.rs src/ui/runner.rs
git commit -m "feat: connect Enter key to queue and start playback"
```

---

### Task 12: Add Status Polling in Runner

**Files:**
- Modify: `src/ui/runner.rs:8-35`

**Step 1: Add status polling in tick loop**

Add after handling actions, before event polling:

```rust
// Poll player status
if let Some(ref mut player) = app.player_manager {
    let _ = player.update_status();
}
```

**Step 2: Verify builds**

Run: `cargo check`
Expected: Compiles successfully

**Step 3: Test manually**

Run: `cargo run -- "rust tutorials"`
Expected: Can add tracks to queue, status updates

**Step 4: Commit**

```bash
git add src/ui/runner.rs
git commit -m "feat: poll player status every tick"
```

---

### Task 13: Implement Auto-Advance

**Files:**
- Modify: `src/ui/runner.rs:30-50`

**Step 1: Add auto-advance logic after status poll**

Add after `player.update_status()`:

```rust
// Poll player status and check for EOF
if let Some(ref mut player) = app.player_manager {
    let _ = player.update_status();

    // Check for track finished
    if player.is_eof() {
        if let Some(next_track) = app.queue.pop_front() {
            // Play next track
            let url = format!("https://www.youtube.com/watch?v={}", next_track.id);
            if player.play(&url, &next_track.title).is_err() {
                // Failed to play, stop player
                app.player_manager = None;
            }
        } else {
            // Queue empty, stop player
            app.player_manager = None;
        }
    }
}
```

**Step 2: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src/ui/runner.rs
git commit -m "feat: implement queue auto-advance on track finish"
```

---

### Task 14: Add Playback Controls

**Files:**
- Modify: `src/ui/events.rs:18-30`

**Step 1: Add global playback controls in handle_browse_keys**

Add after quit keys, before focus routing:

```rust
// Global playback controls (work from any panel)
if let Some(ref mut player) = &mut app.player_manager {
    match key.code {
        KeyCode::Char(' ') => {
            let _ = player.toggle_pause();
            return;
        }
        KeyCode::Char('<') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            let _ = player.seek(-10.0);
            return;
        }
        KeyCode::Char('>') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            let _ = player.seek(10.0);
            return;
        }
        KeyCode::Char('=') | KeyCode::Char('+') => {
            let new_vol = (player.status.volume + 5).min(100);
            let _ = player.set_volume(new_vol);
            return;
        }
        KeyCode::Char('-') => {
            let new_vol = (player.status.volume - 5).max(0);
            let _ = player.set_volume(new_vol);
            return;
        }
        KeyCode::Char('m') => {
            let new_vol = if player.status.volume > 0 { 0 } else { 100 };
            let _ = player.set_volume(new_vol);
            return;
        }
        KeyCode::Char('s') if !matches!(app.focused_panel, FocusedPanel::Results) => {
            let _ = player.stop();
            app.player_manager = None;
            return;
        }
        KeyCode::Char('n') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            // Next track
            if let Some(next_track) = app.queue.pop_front() {
                let url = format!("https://www.youtube.com/watch?v={}", next_track.id);
                let _ = player.play(&url, &next_track.title);
            }
            return;
        }
        _ => {}
    }
}
```

**Step 2: Make player_manager mutable in function signature**

Update function signature to take `app` mutably (already is).

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: add playback controls (space, seek, volume, next)"
```

---

## Phase 5: Queue Controls (2 tasks)

### Task 15: Implement Queue Jump

**Files:**
- Modify: `src/ui/events.rs:handle_queue_keys` (Enter case)

**Step 1: Implement Enter in handle_queue_keys**

Update the Enter case:

```rust
KeyCode::Enter => {
    if !app.queue.is_empty() && app.queue_selected_index < app.queue.len() {
        if app.queue_selected_index > 0 {
            // Move selected track to front
            app.queue.move_to_front(app.queue_selected_index);
        }

        // Start playing if player exists
        if let Some(ref mut player) = app.player_manager {
            if let Some(track) = app.queue.get(0) {
                let url = format!("https://www.youtube.com/watch?v={}", track.id);
                let _ = player.play(&url, &track.title);
            }
        }
    }
}
```

**Step 2: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src/ui/events.rs
git commit -m "feat: implement queue jump with Enter key"
```

---

### Task 16: Handle vlc/mplayer Fallback

**Files:**
- Modify: `src/ui/runner.rs:30-50`
- Modify: `src/player.rs:34-44`

**Step 1: Add supports_background_playback function**

Add to `src/player.rs`:

```rust
pub fn supports_background_playback(player: PlayerType) -> bool {
    matches!(player, PlayerType::Mpv)
}
```

**Step 2: Update Play action handler**

Wrap player creation in `src/ui/runner.rs`:

```rust
AppAction::Play(idx) => {
    if idx < app.results.len() {
        let result = &app.results[idx].clone();
        app.queue.push_back(result.clone());

        if crate::player::supports_background_playback(config.player) {
            // Background playback with mpv
            if app.player_manager.is_none() {
                match PlayerManager::new(config) {
                    Ok(mut pm) => {
                        let url = format!("https://www.youtube.com/watch?v={}", result.id);
                        if pm.play(&url, &result.title).is_ok() {
                            app.player_manager = Some(pm);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to create player: {}", e);
                    }
                }
            }
        } else {
            // Legacy: exit TUI, play externally, return
            crate::ui::terminal::restore_terminal(terminal)?;

            println!("{} {}", "Playing:".green(), result.title);
            crate::display::show_controls(config.player);

            let _ = crate::player::play_video(
                config,
                &result.id,
                &result.title,
                &result.safe_title(),
                temp_dir,
            );

            *terminal = crate::ui::terminal::init_terminal()?;
            println!("\nNote: Background playback requires mpv. Install mpv for best experience.");
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }
}
```

**Step 3: Verify build**

Run: `cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src/ui/runner.rs src/player.rs
git commit -m "feat: add vlc/mplayer fallback to legacy playback mode"
```

---

## Phase 6: Polish & Testing (4 tasks)

### Task 17: Update Help Overlay

**Files:**
- Modify: `src/ui/layout.rs:152-173`

**Step 1: Update help_text in render_help_overlay**

Replace help text:

```rust
let help_text = vec![
    Line::from(Span::styled("Keyboard Controls", Style::default().add_modifier(Modifier::BOLD))),
    Line::from(""),
    Line::from("Focus Navigation:"),
    Line::from("  Tab         - Cycle focus (Search → Results → Queue)"),
    Line::from("  Shift+Tab   - Reverse cycle"),
    Line::from(""),
    Line::from("Search Bar (when focused):"),
    Line::from("  Type        - Enter query"),
    Line::from("  Enter       - Submit search"),
    Line::from("  Esc         - Clear and return to Results"),
    Line::from(""),
    Line::from("Results (when focused):"),
    Line::from("  ↑/↓         - Move selection"),
    Line::from("  Enter       - Add to queue and play"),
    Line::from("  1-9         - Quick-pick by number"),
    Line::from("  n/p         - Next/Previous page"),
    Line::from("  s           - Focus search bar"),
    Line::from(""),
    Line::from("Queue (when focused):"),
    Line::from("  ↑/↓         - Navigate queue"),
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
    Line::from("  s           - Stop (from Queue/Search)"),
    Line::from(""),
    Line::from("Other:"),
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
git commit -m "docs: update help overlay with new controls"
```

---

### Task 18: Add Focus Tests

**Files:**
- Modify: `src/ui/events.rs:280-end`

**Step 1: Add focus cycling tests**

Add to test module in `src/ui/events.rs`:

```rust
#[test]
fn test_tab_cycles_focus_forward() {
    use crate::ui::app::FocusedPanel;

    let mut app = App::new("test".to_string(), 10);
    app.focused_panel = FocusedPanel::SearchBar;

    cycle_focus_forward(&mut app);
    assert_eq!(app.focused_panel, FocusedPanel::Results);

    cycle_focus_forward(&mut app);
    assert_eq!(app.focused_panel, FocusedPanel::Queue);

    cycle_focus_forward(&mut app);
    assert_eq!(app.focused_panel, FocusedPanel::SearchBar);
}

#[test]
fn test_shift_tab_cycles_focus_backward() {
    use crate::ui::app::FocusedPanel;

    let mut app = App::new("test".to_string(), 10);
    app.focused_panel = FocusedPanel::SearchBar;

    cycle_focus_backward(&mut app);
    assert_eq!(app.focused_panel, FocusedPanel::Queue);

    cycle_focus_backward(&mut app);
    assert_eq!(app.focused_panel, FocusedPanel::Results);

    cycle_focus_backward(&mut app);
    assert_eq!(app.focused_panel, FocusedPanel::SearchBar);
}
```

**Step 2: Run tests**

Run: `cargo test ui::events::tests`
Expected: All tests pass

**Step 3: Commit**

```bash
git add src/ui/events.rs
git commit -m "test: add focus cycling tests"
```

---

### Task 19: Fix Compilation Errors

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

### Task 20: End-to-End Testing

**Files:**
- None (testing only)

**Step 1: Test full workflow**

```bash
cargo run -- "rust programming"
```

Test checklist:
- [ ] TUI starts with search bar at top
- [ ] Tab cycles focus: SearchBar → Results → Queue
- [ ] Type in search bar (when focused), Enter searches
- [ ] Enter on result adds to queue and starts playing
- [ ] Progress bar updates in real-time
- [ ] Space pauses/resumes
- [ ] </> seeks backward/forward
- [ ] +/- adjusts volume
- [ ] n advances to next track in queue
- [ ] Queue panel shows all tracks
- [ ] Enter in queue jumps to track
- [ ] Delete removes track from queue
- [ ] c clears queue
- [ ] Track auto-advances when finished
- [ ] Terminal stays in TUI throughout
- [ ] q exits cleanly

**Step 2: Test edge cases**

- Add same track twice (should allow)
- Delete currently playing track from queue (keeps playing)
- Clear queue while playing (track continues)
- Seek past end of track (should auto-advance)
- Volume at 0 vs mute toggle
- Rapid Tab presses

**Step 3: Document any issues**

If bugs found, create follow-up tasks.

**Step 4: Final commit**

```bash
git add -A
git commit -m "test: verify end-to-end workflow"
```

---

## Implementation Complete

All 20 tasks completed. Background playback with queue management and persistent search bar is ready for use.

**Next steps:**
1. Build release: `cargo build --release`
2. Install: `cp target/release/yt-search-play ~/bin/`
3. Test with real usage
4. Gather feedback for future enhancements
