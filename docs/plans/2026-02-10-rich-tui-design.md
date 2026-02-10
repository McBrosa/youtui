# Rich TUI Design for yt-search-play

**Date:** 2026-02-10
**Goal:** Transform the YouTube search and play tool into a rich, interactive TUI experience using ratatui

## Overview

Transition from simple print-and-read-line interface to a full-featured TUI with rich formatting, hybrid navigation, and live status updates.

## Architecture Changes

### Current Flow
- Print results → Read input → Execute action → Repeat

### New Flow
- Enter TUI mode → Render widgets → Handle events → Update state → Re-render

### Module Structure

Add new `ui/` module:
- `ui/app.rs` - Application state (results, selected index, input mode, playback status)
- `ui/layout.rs` - Widget rendering (results panel, search box, status bar, help overlay)
- `ui/events.rs` - Event handling (keyboard input, player events)

Existing modules (`search`, `player`, `config`) remain unchanged - TUI is a view layer over existing business logic.

### Terminal Management

- Use `crossterm::terminal::{enable_raw_mode, disable_raw_mode}` for raw input
- Implement Drop guard to ensure terminal restoration on panic/Ctrl-C
- `PaginatedSearch` still manages results, `play_video` still launches players
- UI layer polls for events and updates display reactively

## UI Layout

### Three-Panel Structure

```
┌─────────────────────────────────────────────────┐
│ yt-search-play │ Query: "rust tutorials"        │ <- Header (1 line)
├─────────────────────────────────────────────────┤
│                                                  │
│  1. Advanced Rust Tutorial - 45:23              │
│     Channel: RustDev | 1.2M views | ID: abc123  │
│                                                  │
│  2. Learn Rust in 2024 - 1:02:15                │ <- Results (scrollable)
│     Channel: CodeMaster | 856K views | ID: xyz  │
│                                                  │
│  [Selected: highlighted with different colors]  │
│                                                  │
│  Page 1/3 • 25 results loaded                   │
├─────────────────────────────────────────────────┤
│ ▶ Playing: "Learn Rust..." [12:34/1:02:15] ... │ <- Status bar (1-2 lines)
├─────────────────────────────────────────────────┤
│ ↑/↓: Navigate • Enter: Play • 1-9: Quick pick   │ <- Footer (1 line)
│ s: Search • n/p: Next/Prev • h: Help • q: Quit  │
└─────────────────────────────────────────────────┘
```

### Widget Components

- **Header:** `Paragraph` widget with app title and current query
- **Results panel:** `List` widget with custom multi-line items (title + metadata)
- **Status bar:** Conditional `Paragraph` - only visible during playback, updates every second
- **Footer:** Context-aware `Paragraph` with keyboard hints

State-driven rendering: `App` struct holds state, widgets render based on current state each frame.

## Event Handling

### Event Types

```rust
enum AppEvent {
    Key(KeyEvent),                    // From crossterm
    PlayerUpdate(PlayerStatus),       // Periodic status from player
    Tick,                            // For animations/status refresh
}
```

### Hybrid Navigation Model

**Arrow key navigation:**
- Up/Down: Move selected_index within current page
- Enter: Play video at selected_index
- Auto-scroll List widget to keep selection visible

**Number quick-pick:**
- Detect 1-9 keypresses → immediately play that numbered result
- For 10+: type full number + Enter
- Store partial input in `App.number_input: Option<String>`

**Command keys:**
- `s`: Enter search mode (input box at bottom)
- `n`/`p`: Change page
- `h`: Toggle help overlay
- `q`/`Esc`: Quit (or exit search mode)

### Search Input Mode

When `s` pressed:
- Footer transforms into text input box using `Paragraph`
- Capture characters until Enter/Esc
- Display cursor with `|` character
- On Enter: call `search.reset()` and return to browse mode

### Player Status Updates

- Spawn background thread when `play_video()` called
- Thread periodically reads player status:
  - mpv: use `--input-ipc-server` socket
  - vlc/mplayer: poll process status
- Send updates via `mpsc::channel` to main thread
- Event loop merges with keyboard events

## Visual Styling

### Color Scheme

- **Header bar:** Bold cyan background, black text
- **Selected result:** Yellow background, black text
- **Unselected results:** White title on black
- **Metadata rows:** Dim gray text
- **Status bar:** Green (playing), blue (buffering), red (error)
- **Footer hints:** Cyan for keys, white for descriptions
- **Borders:** Dark gray using `Block::default().borders()`

### Box Drawing

- Main panels: `Block` with single-line borders (`─│┌┐└┘├┤`)
- Header/footer: Horizontal separators only
- Use `Block::title()` for panel labels

### Typography

- Result titles: Bold + bright color
- Selected item: Full block background color
- Metadata: Unicode symbols `⏱ 45:23 │ 👁 1.2M views │ 📺 RustDev`
- Progress bar: Unicode blocks `▰▰▰▰▰▱▱▱▱▱ 50%`

### Responsive Design

- Calculate available height: `terminal_height - header(1) - status(1-2) - footer(1) - borders(2)`
- Dynamic page size based on terminal height
- Truncate long titles with `...` if narrow

### Help Overlay

- Press `h` for centered popup
- Use `Layout` with percentage-based constraints
- Gray background for semi-transparent effect
- Organized categories of keyboard shortcuts

## Implementation Phases

### Phase 1: Foundation
- Add `ratatui` and `crossterm` dependencies
- Create `ui/` module structure with stubs
- Add terminal setup/teardown with cleanup guards
- Test raw mode enable/disable and Ctrl-C handling

### Phase 2: Core TUI Loop
- Implement `App` state struct
- Build event loop: poll → update → render
- Render simple results `List` widget
- Get arrow navigation + Enter working

### Phase 3: Rich Rendering
- Add styling: colors, borders, highlighting
- Implement three-panel layout
- Add unicode symbols and polish
- Make responsive to terminal size

### Phase 4: Hybrid Navigation
- Add number quick-pick (1-9 direct, 10+ buffered)
- Implement search input mode
- Add help overlay with `h` key

### Phase 5: Player Integration
- Add status bar during playback
- mpv: IPC socket for real-time status (optional)
- vlc/mplayer: static "Playing..." message
- Keep status visible while player runs

## Backward Compatibility

- All existing CLI args continue working (`--audio-only`, `--format`, `--download`, etc.)
- `Config`, `PaginatedSearch`, `play_video` logic unchanged
- Only UI layer changes

## Testing Considerations

- Different terminal sizes (test resize while running)
- All three players (mpv, vlc, mplayer)
- Edge cases: empty results, network failures, Ctrl-C during playback
- Terminal restoration on crash/panic

## Dependencies to Add

```toml
ratatui = "0.26"
crossterm = "0.27"
```

Existing dependencies remain: `clap`, `colored`, `tempfile`, `ctrlc`, `anyhow`, `which`

## Success Criteria

- Rich visual layout with boxes, colors, and structure
- Smooth arrow key navigation with visual selection
- Number quick-pick still works (1-9 direct)
- Live status bar during video playback
- Clean terminal restoration in all exit scenarios
- All existing features continue to work
