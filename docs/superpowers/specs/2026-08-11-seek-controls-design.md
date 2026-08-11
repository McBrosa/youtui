# Seek Controls Design — YouTube-style skip and jump

Date: 2026-08-11
Status: Approved (Approach A)

## Goal

Add YouTube-player-style seeking to youtui playback:

1. `←` / `→` seek backward/forward by a configurable step (default 5s).
2. `Shift+←` / `Shift+→` seek by a configurable large step (default 60s).
3. `t` opens a timestamp prompt in the player bar; typing `1:23:45`, `12:34`,
   or `90` and pressing Enter jumps to that absolute position.

Existing `<` / `>` (±10s) keys remain unchanged. Digit-to-percent jumping was
considered and explicitly dropped (conflicts with result-number selection).

## Non-goals

- No keybinding remap system or generic binding table.
- No digit 0-9 percent jump.
- No chapter metadata support.
- Non-mpv players (VLC/mplayer fallback) unaffected — seeks only work via mpv
  IPC, same as existing `<`/`>`.

## Changes by file

### `src/config.rs`

- Two new persisted `Config` fields:
  - `seek_step: u64` — default `5` (seconds).
  - `seek_step_large: u64` — default `60` (seconds).
- Load/save alongside existing fields with the same atomic-write path.
  Missing keys in an existing config file fall back to defaults.
- Values clamped to `1..=3600` on load and on settings save.

### `src/player_manager.rs`

- New method mirroring `seek()`:

  ```rust
  pub fn seek_absolute(&mut self, seconds: f64) -> Result<()>
  ```

  Sends `["seek", &seconds.to_string(), "absolute"]` over IPC with the same
  active-playback guard as `seek()`.

### `src/ui/app.rs`

- New field: `timestamp_input: Option<String>` — `Some` while the `t` prompt
  is open; holds the raw typed text.
- Two new `SettingsField` variants: `SeekStep`, `SeekStepLarge`. Both edited
  like `ResultsPerPage` (digit-only text input, committed on Enter).

### `src/ui/events.rs`

- **Timestamp prompt intercept** — at top of `handle_browse_keys`, after the
  settings intercept: when `app.timestamp_input.is_some()`:
  - ASCII digits and `:` append (max length 8).
  - `Backspace` deletes last char.
  - `Enter` parses; on success calls `seek_absolute(secs)` clamped to
    `[0, duration]`, closes prompt. On parse failure sets a status message
    ("Invalid timestamp") and keeps the prompt open.
  - `Esc` closes the prompt without seeking.
  - All other keys ignored while prompt open.
- **Global playback block** (the existing `Space`/`<`/`>` match, non-SearchBar
  panels only):
  - `Left` with `SHIFT` → `player.seek(-(seek_step_large as f64))`.
  - `Right` with `SHIFT` → `player.seek(seek_step_large as f64)`.
  - `Left` plain → `player.seek(-(seek_step as f64))`.
  - `Right` plain → `player.seek(seek_step as f64)`.
  - `t` → open prompt (`app.timestamp_input = Some(String::new())`) only when
    a player is active; no-op otherwise.
  - All use existing `run_player_command`.
- **Parse helper**:

  ```rust
  fn parse_timestamp(input: &str) -> Option<f64>
  ```

  Accepts `ss`, `mm:ss`, `hh:mm:ss`. Rules: 1-3 colon-separated segments, all
  digits, non-leading segments must be `< 60` and exactly 1-2 digits; empty
  segments rejected. Returns total seconds.

- **Settings editing**: extend the `ResultsPerPage` digit-only guard to the
  two new fields; commit parses `u64`, clamps `1..=3600`, writes config.

### `src/ui/layout.rs`

- Player bar: while `timestamp_input` is `Some`, render `Jump to: <text>▌` in
  the player status line (replacing the progress/time segment for that frame).
- Settings panel: two new rows ("Seek step (s)", "Large seek step (s)")
  following existing field row pattern.
- Help panel rows:
  - `←/→` — "Seek ±<seek_step>s"
  - `Shift+←/→` — "Seek ±<seek_step_large>s"
  - `t` — "Jump to timestamp"

## Error handling

- No active player: arrow/`t` keys fall through as no-ops (existing
  `run_player_command` returns false → key continues to panel handler; arrows
  currently unbound in panels so effectively no-op).
- IPC failure: existing `run_player_command` path — player cleared, status
  message shown.
- Invalid timestamp text: status message, prompt stays open.
- Timestamp beyond duration: clamp to duration (mpv also tolerates overshoot;
  clamp keeps UI progress sane). If duration unknown (0), pass value through
  unclamped.

## Testing

Unit tests in existing test modules, matching current style:

- `parse_timestamp`: `"90"`→90, `"12:34"`→754, `"1:02:03"`→3723, rejects
  `""`, `"1:99"`, `":30"`, `"1:2:3:4"`, `"ab"`, `"12:"`.
- Events: `Left`/`Right`/Shift variants dispatch seek with configured steps;
  `t` opens prompt only with active player; prompt Enter/Esc/append/backspace
  behavior; digits still select results when prompt closed.
- Config: new fields round-trip save/load; defaults applied when keys absent;
  clamping.
- player_manager: `seek_absolute` emits `["seek", "<n>", "absolute"]` (mirror
  of existing seek command tests).
