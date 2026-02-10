# Background Playback & Persistent Search Bar Design

## Overview

Add background audio playback with queue management and a persistent search bar to the yt-search-play TUI. Users can browse results, manage a queue, and control playback without leaving the interface.

## Goals

- Play audio in background while staying in TUI
- Show live playback progress (title, elapsed time, duration)
- Queue system - add multiple tracks, auto-advance when finished
- Persistent search bar at top (always visible, Tab to focus)
- Extended playback controls (play/pause, seek, volume, next/prev)
- mpv IPC for real-time status, fallback to legacy mode for vlc/mplayer

## Architecture Overview

### Core Components

**1. Player Manager** (`src/player_manager.rs`)
- Spawns mpv with `--idle --input-ipc-server=/tmp/yt-search-play-<pid>.sock`
- Maintains Unix socket connection
- Sends JSON commands: play, pause, seek, volume, stop
- Polls status every tick (250ms): time-pos, duration, pause state, volume
- Handles IPC errors: reconnect on socket failure, auto-advance on crash

**2. Queue System** (`src/queue.rs`)
- `VecDeque<SearchResult>` for track queue
- Operations: push_back (add), pop_front (next), remove (by index), clear
- Track currently playing with index
- Auto-advance: when track finishes, pop next and play

**3. App State Extensions** (`src/ui/app.rs`)
```rust
pub struct App {
    // Existing fields...
    pub player_manager: Option<PlayerManager>,
    pub queue: Queue,
    pub current_track_idx: Option<usize>,
    pub focused_panel: FocusedPanel,
}

pub enum FocusedPanel {
    SearchBar,
    Results,
    Queue,
}
```

**4. IPC Module** (`src/ipc.rs`)
- JSON command formatting for mpv protocol
- UnixStream socket wrapper
- Response parsing: extract time-pos, duration, etc.
- Error handling: broken pipe, parse errors, timeouts

## UI Layout

### 4-Panel Design

```
┌─────────────────────────────────────────────────────────────┐
│ Search: [rust tutorials____________] (focused: cyan border) │
├──────────────────────────────┬──────────────────────────────┤
│ Search Results (70%)         │ Queue (30%)                  │
│                              │                              │
│   1. Video Title             │  ▶ 1. Current Track         │
│   2. Selected Video (yellow) │    2. Next Track            │
│   3. Another Video           │    3. Queued Track          │
│   ...                        │                              │
│                              │  [Empty - Add tracks]        │
│                              │                              │
├──────────────────────────────┴──────────────────────────────┤
│ ▶ Now Playing: Track Title [████████░░] 2:34 / 4:12  Vol:80│
│ Tab: Switch Focus • Enter: Play/Queue • Space: Pause • </>: │
└─────────────────────────────────────────────────────────────┘
```

### Panel Descriptions

**Top: Search Bar** (1 line)
- Always visible, not modal
- Focused: cyan border, cursor visible
- Unfocused: dark gray border, shows last query
- Tab to focus, type directly, Enter submits, Esc clears and returns to Results

**Left: Search Results** (70% width, flexible height)
- Current behavior: paginated list with arrow navigation
- Enter adds selected track to queue (starts if empty)
- Number quick-pick still works
- Focused: cyan border, yellow selection highlight
- Unfocused: dark gray border, selection still visible

**Right: Queue Panel** (30% width, flexible height)
- Shows all queued tracks
- Current track has ▶ prefix and green highlight
- Arrow keys navigate, Enter jumps to track, Delete removes
- Empty state: "Queue is empty. Press Enter on results to add tracks."
- Focused: cyan border with selected track highlighted
- Unfocused: dark gray border, current track still green

**Bottom: Status Bar** (2 lines)
- Line 1: Now playing info with progress bar
  - Format: `▶ Now Playing: {title} [████████░░] {elapsed} / {duration}  Vol:{volume}`
  - Paused: Change ▶ to ⏸
  - Idle: "No track playing. Select from results to start."
- Line 2: Control hints
  - Updates based on focused panel

## Focus Management

### Navigation Flow

**Tab Key**: Cycle forward
- SearchBar → Results → Queue → SearchBar

**Shift+Tab**: Cycle backward
- Queue → Results → SearchBar → Queue

### Panel-Specific Behavior

**When SearchBar Focused:**
- All character input goes to search buffer
- Backspace deletes characters
- Enter: Submit search, move focus to Results
- Esc: Clear buffer, move focus to Results
- Arrow keys: No effect (stay in search bar)

**When Results Focused:**
- Arrow up/down: Navigate list
- Enter: Add selected to queue (start if idle)
- Number keys: Quick-pick buffer
- 's': Focus search bar
- 'h': Help overlay (blocks other input)
- 'q': Quit

**When Queue Focused:**
- Arrow up/down: Navigate queue
- Enter: Jump to selected track (moves to current, starts playing)
- Delete/Backspace: Remove selected track
- 'c': Clear entire queue
- Arrow left: Focus Results

### Global Keys (Work From Any Panel)

**Playback Controls:**
- Space: Toggle play/pause
- 's' (when not Results): Stop and clear current (queue remains)
- 'n': Next track
- 'p': Previous track (restart if >3s, otherwise go back)
- '<': Seek backward 10 seconds
- '>': Seek forward 10 seconds
- '=': Volume up 5%
- '-': Volume down 5%
- 'm': Mute toggle

## mpv IPC Protocol

### Socket Setup

1. Spawn mpv: `mpv --idle --input-ipc-server=/tmp/yt-search-play-<pid>.sock --no-video`
2. Wait for socket file to exist (100ms poll, 2s timeout)
3. Connect: `UnixStream::connect(socket_path)`
4. Keep connection open for session

### Command Format

All commands are newline-delimited JSON:

```json
{ "command": ["loadfile", "https://youtube.com/watch?v=..."] }
{ "command": ["get_property", "time-pos"] }
{ "command": ["set_property", "pause", false] }
{ "command": ["cycle", "pause"] }
{ "command": ["seek", 10, "relative"] }
{ "command": ["set_property", "volume", 75] }
{ "command": ["stop"] }
```

### Response Format

```json
{ "data": 123.45, "error": "success" }
{ "error": "property unavailable" }
```

### Status Polling (Every Tick)

Query these properties:
- `time-pos`: Current position in seconds (f64)
- `duration`: Total duration in seconds (f64)
- `pause`: Is paused (bool)
- `volume`: Volume 0-100 (i32)
- `eof-reached`: Track finished (bool)

Batch queries in single tick to minimize latency.

### Error Handling

**Socket connection fails:**
- Log error to status bar: "Failed to connect to mpv"
- Fall back to legacy external playback for that track
- Don't crash - allow user to continue browsing

**Socket breaks during playback:**
- Detect via `UnixStream` read/write errors
- Show in status bar: "Playback interrupted"
- Auto-advance to next in queue
- Attempt to reconnect socket for next play

**JSON parse errors:**
- Log warning (debug mode only)
- Skip that status update
- Continue polling next tick
- Don't accumulate errors - treat as transient

**mpv process dies:**
- Detect via `process.try_wait()` showing exit
- Show "Player crashed" in status bar
- Clear current track, idle state
- Allow user to play next track (respawn mpv)

## Queue Behavior

### Adding Tracks

**From Results Panel:**
- Enter on selected track: `queue.push_back(result.clone())`
- If `player_manager.is_none()` (idle): Start playing immediately
- If playing: Add to back of queue, continue current

**Number Quick-Pick:**
- Type digits, Enter: Same as arrow select + Enter
- Adds to queue by number

### Auto-Advance Logic

In runner tick, after polling IPC status:

```rust
if player_manager.is_track_finished() {
    if let Some(next_track) = app.queue.pop_front() {
        player_manager.play(&next_track)?;
        app.current_track_idx = Some(0);
    } else {
        // Queue empty
        player_manager.stop();
        app.player_manager = None;
        app.current_track_idx = None;
    }
}
```

### Queue Manipulation

**Jump to Track (Queue Panel focused, Enter):**
```rust
let selected_idx = app.queue_selected_idx;
if selected_idx != app.current_track_idx {
    // Stop current
    player_manager.stop();
    // Remove tracks before selected
    app.queue.drain(0..selected_idx);
    // Play new current (now at index 0)
    player_manager.play(&app.queue[0])?;
    app.current_track_idx = Some(0);
}
```

**Remove Track:**
```rust
app.queue.remove(selected_idx);
if selected_idx == app.current_track_idx {
    // Removed currently playing - it keeps playing
    // But won't show in queue panel anymore
    // When finished, advances to new queue[0]
}
```

**Clear Queue:**
```rust
app.queue.clear();
// Current track keeps playing
// When finished, no auto-advance (goes idle)
```

## Player Compatibility

### mpv: Full Background Playback

- Supports IPC socket
- Real-time status updates
- All controls work
- Smooth queue transitions

### vlc/mplayer: Legacy Fallback

```rust
pub fn supports_background_playback(player: PlayerType) -> bool {
    matches!(player, PlayerType::Mpv)
}

// In play action handler:
if supports_background_playback(config.player) {
    // New: PlayerManager + IPC
    if app.player_manager.is_none() {
        app.player_manager = Some(PlayerManager::new(config)?);
    }
    app.player_manager.play(&track)?;
    app.queue.push_back(track);
} else {
    // Legacy: Exit TUI, play externally, return
    restore_terminal(terminal)?;
    let _ = play_video_blocking(config, &track)?;
    *terminal = init_terminal()?;
}
```

User sees notification on first run:
```
Note: Background playback requires mpv.
Using legacy mode with vlc - will exit TUI during playback.
Install mpv for best experience: brew install mpv
```

## Edge Cases

### Search While Playing

- Search bar is always accessible
- New search doesn't affect playback
- Queue is preserved
- Results panel refreshes, playback continues

### Queue Same Track Twice

- Allowed - adds duplicate
- Each entry is independent
- Useful for repeat-one without loop

### Delete Currently Playing Track

- Track continues playing
- Removed from queue display
- When finished, advances to queue[0] (which is now the "next" track)

### Rapid Commands

- Debounce IPC writes: max 1 command per 100ms
- Queue commands internally if too fast
- Prevents socket buffer overflow
- User won't notice 100ms delay

### Terminal Resize

- ratatui handles automatically
- Panels scale proportionally (70/30 split maintained)
- Progress bar width adjusts
- Text truncates with ellipsis if needed

### Empty Queue Display

- Show helpful text:
  ```
  Queue is empty

  Press Enter on results to add tracks
  ```
- Center in panel
- Gray color for hint text

## Implementation Plan

### Phase 1: Foundation (5 tasks)

1. Add dependencies: `serde_json`
2. Create `src/ipc.rs` with socket wrapper and JSON helpers
3. Create `src/player_manager.rs` stub with mpv spawn
4. Create `src/queue.rs` with VecDeque wrapper
5. Add fields to `App`: player_manager, queue, focused_panel

### Phase 2: IPC & Status Polling (4 tasks)

6. Implement mpv socket connection with timeout
7. Implement IPC command sending: loadfile, stop, pause, seek, volume
8. Implement status polling: get time-pos, duration, pause, volume, eof
9. Add error handling: reconnect, parse errors, broken pipe

### Phase 3: UI Layout (3 tasks)

10. Add persistent search bar at top (1 line, always visible)
11. Split main area into Results (70%) + Queue (30%) panels
12. Update status bar to show progress bar, volume, controls hints

### Phase 4: Focus Management (2 tasks)

13. Add Tab/Shift+Tab focus cycling
14. Update event handlers: route keys based on focused_panel

### Phase 5: Playback Integration (3 tasks)

15. Connect Enter key to queue.push_back + player_manager.play
16. Add tick handler: poll IPC status, update app.playback_state
17. Implement auto-advance: detect eof, pop queue, play next

### Phase 6: Controls & Polish (3 tasks)

18. Add playback controls: Space, n/p, </>, =/-/m keys
19. Add queue controls: Delete, 'c', Enter (jump to track)
20. Update help overlay with new controls

## Testing Strategy

### Unit Tests

- `queue.rs`: push, pop, remove, clear, reorder operations
- `ipc.rs`: JSON command formatting, response parsing
- `app.rs`: focus cycling logic (Tab wraps correctly)
- `player.rs`: `supports_background_playback()` detection

### Integration Tests (Manual)

- Start playback, verify IPC connection succeeds
- Poll status while playing, verify time-pos increments
- Add 3 tracks to queue, verify auto-advance works
- Seek forward/back, verify position changes
- Crash mpv manually, verify TUI doesn't crash

### Edge Case Tests (Manual)

- Play track while another is playing (replaces, queue works)
- Seek past end of track (should trigger auto-advance)
- Volume at 0 vs mute (different states, both work)
- Rapid Tab presses (focus cycles, no crash)
- Delete track 0 while playing track 0 (keeps playing, advances correctly)

## Dependencies

**New:**
- `serde_json` = "1.0" (JSON parsing for IPC)

**Existing:**
- `ratatui` = "0.26" (TUI framework)
- `crossterm` = "0.27" (Terminal control)
- Standard library `std::os::unix::net::UnixStream` (IPC socket)

## Files to Create

- `src/player_manager.rs` (~200 lines)
- `src/queue.rs` (~100 lines)
- `src/ipc.rs` (~150 lines)

## Files to Modify

- `src/ui/app.rs` - Add player_manager, queue, focused_panel fields
- `src/ui/events.rs` - Add Tab, Space, seek, volume key handlers, route by focus
- `src/ui/layout.rs` - New 4-panel layout: search bar, results, queue, status
- `src/ui/runner.rs` - Poll IPC each tick, auto-advance queue
- `src/main.rs` - Initialize empty queue, show mpv requirement notice
- `Cargo.toml` - Add serde_json dependency

## Success Criteria

- User can press Enter to add tracks to queue while browsing
- Audio plays in background, progress bar updates in real-time
- Space pauses/resumes, </> seeks, =/- adjusts volume
- Queue auto-advances to next track when current finishes
- Tab cycles focus between search bar, results, and queue
- Search bar is always visible, can type and submit without 's' key
- Queue panel shows all queued tracks with current playing highlighted
- No crashes on mpv disconnect, socket errors, or rapid commands
- vlc/mplayer users see notice and fall back to legacy mode

## Future Enhancements (Out of Scope)

- Shuffle/repeat modes
- Save/load playlists
- Lyrics display integration
- Album art (if fetchable)
- Audio visualizer
- Crossfade between tracks
- Search history dropdown
