# Terminal Video View — Design Spec

**Date:** 2026-08-11
**Status:** Approved

## Summary

Render the currently playing video inside the terminal, in the main content
area where the search/results/queue panels live. The user toggles between the
video view and the normal search/queue view with a single key. Video renders
as Unicode half-block cell art (works in every terminal). mpv remains the
audio/playback master; a separate ffmpeg process supplies frames.

## Goals

- Toggle key `v` switches the main content area between video view and the
  normal search/queue layout.
- Video renders as `▀` half-block cells (fg = top pixel, bg = bottom pixel),
  giving 2x vertical resolution and full RGB color via ratatui cells.
- Playback controls (pause, seek, next/prev, volume) keep working while the
  video view is active.
- Video failures never affect audio playback or the rest of the TUI.

## Non-Goals (YAGNI)

- Pixel graphics protocols (kitty/sixel/iTerm2).
- Braille or ASCII-luminance art modes.
- Configurable fps or resolution (fixed: 12 fps, 480p source cap).
- VLC/mplayer fallback support — mpv only (fallback players run in the
  foreground and own the terminal anyway).

## Architecture

```
mpv (audio master, existing)          ffmpeg (frame source, new)
  --no-video NOT forced; existing       -ss <mpv time-pos>
  audio_only config unchanged           -i <direct stream URL from yt-dlp -g>
  IPC time-pos polling (existing) ──►   -vf scale=W:H,fps=12
                                        -f rawvideo -pix_fmt rgb24 -
        sync decisions                        │ stdout pipe
              │                               ▼
              └──────────────► reader thread ──channel──► UI render
```

- **mpv stays the single source of truth** for position, pause state, track
  changes. It keeps playing exactly as today (own OS window rules unchanged
  when video view is off; see "mpv window while video view is active" below).
- **ffmpeg decodes the stream a second time** starting at mpv's current
  position, scaled directly to the pane's cell grid, piped as raw RGB24.
- **A reader thread** reads exactly `W*H*3` bytes per frame and sends frames
  over a bounded channel; stale frames are dropped (only latest matters).

### mpv window while video view is active

When the terminal video view is toggled on, mpv's own OS video window is
redundant. Set the mpv property `vid` to `no` (`{"command": ["set_property",
"vid", "no"]}`) when the view activates, and restore `vid=auto` when the view
deactivates. This keeps audio playing, drops mpv's video decode, and closes
its window without restarting playback. If the property set fails, log to the
status message and continue — coexisting windows are acceptable degradation.

## Components

### 1. `App` state (`src/ui/app.rs`)

- `pub video_view: bool` — whether the video view is active.
- Toggle rules (key `v`, `InputMode::Browse`, any focused panel):
  - A track is playing (or paused) AND `config.audio_only == false` →
    toggle `video_view`.
  - Otherwise → set the existing status/flash message ("nothing playing" /
    "audio-only mode — no video") and do not toggle.

### 2. New module: `src/video.rs`

Owns the frame pipeline. Public surface:

```rust
pub struct VideoSession { /* child, reader handle, rx, dims, start_pos, frames_shown */ }

pub struct Frame { pub width: u16, pub height_px: u16, pub rgb: Vec<u8> }

impl VideoSession {
    /// Spawn ffmpeg at `position` seconds for `stream_url`, scaled to
    /// `cols` x `rows*2` pixels. Non-blocking; returns Err on spawn failure.
    pub fn start(stream_url: &str, position: f64, cols: u16, rows: u16) -> Result<VideoSession>;

    /// Latest frame if a new one arrived; updates frames_shown.
    pub fn poll_frame(&mut self) -> Option<Frame>;

    /// Position ffmpeg is expected to be at: start_pos + frames_shown / 12.0
    pub fn expected_position(&self) -> f64;

    /// Kill ffmpeg and join the reader thread.
    pub fn stop(&mut self);
}

/// Pure function: should the session restart? True when
/// |mpv_pos - expected| > 2.0.
pub fn drift_exceeded(mpv_pos: f64, expected: f64) -> bool;

/// Pure function: build the ffmpeg argv (testable without spawning).
pub fn ffmpeg_args(url: &str, position: f64, w_px: u16, h_px: u16) -> Vec<String>;
```

Implementation notes:

- `ffmpeg_args` produces:
  `-ss <position> -i <url> -vf scale=<w>:<h>,fps=12 -f rawvideo -pix_fmt rgb24 -loglevel error -`
  (output to stdout). `w = cols`, `h = rows * 2`.
- Reader thread: `read_exact` into a `w*h*3` buffer in a loop; send via
  `std::sync::mpsc::sync_channel(1)` with `try_send` (drop frame when full).
  Thread exits when the pipe closes (ffmpeg killed or stream ended).
- `stop()` must be idempotent and called on Drop.
- ffmpeg stderr → `Stdio::null()` (loglevel error already set; do not let it
  write to the tty under ratatui).

### 3. Stream URL resolution

- Command: `yt-dlp -g -f "bestvideo[height<=480]/best[height<=480]" <watch-url>`
  where watch-url is `https://www.youtube.com/watch?v=<id>` (same construction
  as `src/ui/runner.rs:578`).
- First line of stdout = direct stream URL. Video-only stream is fine — audio
  comes from mpv.
- Runs on a background thread (never block the UI tick). While resolving,
  the video pane shows "loading video…".
- Cache: `HashMap<String /* video id */, String /* url */>` for the session.
  On ffmpeg failing immediately after start with a cached URL (URLs expire),
  evict the cache entry and re-resolve once before reporting an error.

### 4. Sync logic (in the existing status-poll path)

The UI already polls mpv `STATUS_PROPERTIES` including `time-pos`
(`src/player_manager.rs:13`). Each poll while `video_view` is on:

- **Track changed** (video id differs from session's) → stop session, start
  for the new track at mpv position (resolve URL if not cached).
- **Paused** → stop session (kill ffmpeg), keep last frame, render a `⏸`
  overlay centered on the frame. On resume, start a new session at the
  current position.
- **Seek / drift**: if `drift_exceeded(mpv_pos, session.expected_position())`
  → restart the session at `mpv_pos`. This one rule covers both user seeks
  and gradual decode drift; no separate seek hook needed.
- **Stopped / queue ended** → stop session, show the placeholder.
- **Pane resized** (cols/rows differ from session dims) → restart at the
  current position with new dimensions.

### 5. Rendering (`src/ui/layout.rs`)

- When `app.video_view` is true, the main content area (everything currently
  occupied by the search bar + results + queue panels) is replaced by a single
  video widget. The status bar and controls line at the bottom render as
  usual; the controls line shows video-view keys (`v` back, space pause,
  seek keys, `n`/`p`).
- Widget draws the latest frame: for each cell `(x, y)` set char `▀`,
  fg = RGB of pixel `(x, 2y)`, bg = RGB of pixel `(x, 2y+1)`. Use
  `ratatui::style::Color::Rgb`.
- Letterboxing: ffmpeg's `scale=W:H` stretches to the pane. Acceptable —
  simplicity over aspect ratio. (Ceiling: add
  `force_original_aspect_ratio=decrease,pad=…` to the filter later if the
  stretch bothers anyone.)
- States, in priority order: error message → "loading video…" →
  last frame (+ `⏸` overlay when paused) → "no video playing" placeholder.

### 6. Key handling (`src/ui/events.rs`)

- `v` in Browse mode → toggle per the rules in Component 1.
- While `video_view` is on: playback keys work as normal; keys that only make
  sense in search/queue view (panel navigation, result selection, `/`) either
  toggle the view off first or are ignored — implementer's choice, but `Esc`
  and `v` MUST return to the search/queue view. `q` quits as usual.
- `?` help overlay gains a line documenting `v`.

## Error Handling

All failures render a message inside the video pane and never touch audio:

- `yt-dlp -g` fails → "video unavailable: could not resolve stream".
- ffmpeg not installed / spawn fails → "video unavailable: ffmpeg not found"
  (reuse the dependency-detection pattern in `src/deps.rs` if it fits).
- ffmpeg exits mid-stream → keep last frame; on next sync poll the drift rule
  restarts it once; if it dies again within ~5s, show the error message and
  stop retrying until track change or manual re-toggle.
- Toggling back to search/queue always works regardless of video state.
- Toggling off stops any session and resolution work (kill ffmpeg immediately;
  a stale URL resolution result is discarded, not applied).

## Testing

Unit tests only, no network, no spawned ffmpeg:

- `ffmpeg_args` builds the exact expected argv for given url/pos/dims.
- `drift_exceeded` boundaries (1.9 → false, 2.1 → true; also negative drift).
- Frame→cell mapping: synthetic 2x4-pixel frame renders the expected `▀`
  cells with correct fg/bg RGB.
- Toggle state transitions: playing → toggles; nothing playing → no toggle +
  message; audio_only → no toggle + message.
- Follow the existing test style in `player_manager.rs` (in-module `#[cfg(test)]`).

## Dependencies

No new crates. New runtime dependency use: ffmpeg (already documented in the
README as required for downloads) and yt-dlp `-g` mode (already required).
