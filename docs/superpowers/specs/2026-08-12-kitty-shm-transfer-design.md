# Kitty Shared-Memory Frame Transfer — Design Spec

**Date:** 2026-08-12
**Status:** Implemented — Phase 0 answered from Ghostty source (readSharedMemory: shm_open + unlink-after-read) instead of a live bench; the standalone bench example was skipped, youtui itself is the manual test.

## Summary

The pixel renderer currently ships every frame as base64 RGBA through the
PTY (`ratatui-image`, kitty `t=d`). That costs ~4/3x encode overhead plus a
full trip through the terminal's escape-sequence parser — the practical
ceiling that pinned `VIDEO_FPS` at 24. The kitty graphics protocol also
accepts frames via POSIX shared memory (`t=s`): the escape sequence carries
only a base64 shm object *name*; the terminal mmaps, reads, and unlinks the
object itself. Payload through the PTY drops from megabytes to ~100 bytes
per frame, opening the way to 60 fps.

`ratatui-image` only implements `t=d`, so this needs a small hand-rolled
kitty encoder for the transmit side. Placement (the Unicode-placeholder cell
grid) can copy the crate's proven approach.

## Goals

- 60 fps pixel rendering in kitty-protocol terminals on the local machine
  (Ghostty, kitty).
- Zero regression for every other path: t=d fallback, iTerm2, blocks, SSH.
- Fallback is automatic and silent; no new user-facing configuration.

## Non-goals

- Sixel or iTerm2 fast paths (no shm equivalent exists).
- Kitty animation frames (`a=f`) — per-frame retransmit is simpler and shm
  makes it cheap enough.
- Matching source fps exactly; fixed 60 cap.

## Protocol facts (verified against kitty docs)

- `t=s`: payload = base64 shm object name. Terminal reads then unlinks and
  closes it (POSIX). Client creates via `shm_open` + `ftruncate` + `mmap`.
- `f=24` sends raw RGB — 25% smaller than the RGBA (`f=32`) the crate uses,
  and matches our ffmpeg output exactly (no per-frame RGB→RGBA copy).
- `U=1` virtual placement: image is displayed wherever placeholder cells
  (U+10EEEE + row/col diacritics, fg color = image id) appear — this is what
  makes kitty graphics compose with ratatui's cell diffing.
- `a=d,d=I,i=<id>` deletes an image and frees its data.
- `q=2` suppresses terminal responses — mandatory; we must never read stdin
  (see the query-race regression, 2026-08-11).
- tmux requires escape passthrough and breaks shm locality assumptions —
  shm path disabled under `$TMUX`.

## Phases

### Phase 0 — Spike (gate for everything else)

Standalone `examples/kitty_shm_bench.rs`: opens the tty raw, transmits a
synthetic animation via `t=s`, then via `t=d`, printing achieved fps and CPU
for each. Run manually in Ghostty and kitty.

Answers required before Phase 1:
1. Does Ghostty support `t=s` at all? (kitty does; Ghostty needs proof.)
2. Does the terminal unlink promptly, or do objects accumulate? Decide
   between one shm object per frame (rotating names) vs one persistent
   buffer rewritten per frame.
3. Real fps ceiling at 720p/60 — confirms the effort is worth it.

If Ghostty lacks `t=s`, stop: file upstream issue, keep 24 fps t=d.

### Phase 1 — Encoder module `src/kitty_shm.rs`

~250 lines, all escape-string builders pure and unit-tested:

```rust
pub struct ShmKittyTransport { /* seq counter, current/previous image id, pane cols/rows */ }
impl ShmKittyTransport {
    /// mmap-copy `rgb` into a fresh shm object and return the escape
    /// sequences to transmit it and drop the previous frame's image.
    pub fn frame_sequences(&mut self, frame: &Frame) -> Result<String>;
    /// Placeholder cells for the pane, fg-colored with the current image id.
    pub fn write_placeholders(&self, area: Rect, buf: &mut Buffer);
}
fn transmit_escape(id: u32, shm_name: &str, w: u16, h: u16) -> String;  // _Gq=2,i=,a=T,U=1,f=24,t=s,s=,v=;b64(name)
fn delete_escape(id: u32) -> String;                                    // _Gq=2,a=d,d=I,i=
fn shm_write(name: &str, rgb: &[u8]) -> Result<()>;                     // shm_open O_CREAT|O_EXCL, ftruncate, mmap, copy
```

Constraints:
- shm names ≤ 31 chars (macOS `PSHMNAMLEN`): `/yt-<pid%0xffff>-<seq%8>`.
- Rotate 2 image ids (transmit new → repaint placeholders → delete old) so
  a frame is never deleted while still displayed (no flicker).
- On any shm/escape error: return Err once; caller downgrades to the
  `ratatui-image` t=d path for the rest of the session.
- `// SAFETY:` comments on the `shm_open`/`mmap` unsafe blocks; unmap+close
  on every exit path (terminal owns the unlink).

### Phase 2 — Integration in `src/video.rs` + `src/ui/layout.rs`

```rust
enum PixelTransport {
    Shm(ShmKittyTransport),   // kitty && local && !tmux
    Crate,                    // everything else (existing Protocol path)
}
```

- Selection at `set_picker` time: `ProtocolType::Kitty` && `SSH_CONNECTION`/
  `SSH_TTY` unset && `TMUX` unset → `Shm`, else `Crate`.
- `store_frame`: Shm path skips `RgbImage`/`DynamicImage`/`new_protocol`
  entirely — `frame_sequences(&frame)` + remember pane area. `VideoDisplay`
  gains a `ShmFrame` variant; layout writes the escape string into the first
  buffer cell (same trick `ratatui-image` uses) and placeholder cells over
  the pane.
- Downgrade on first error: replace transport with `Crate`, log nothing,
  keep playing.

### Phase 3 — Frame rate per transport

- `VideoFps` becomes a per-session value: 60 for `Shm`, 24 for `Crate`
  (`VideoSession` already carries its own clock; pass fps at `start`).
- `VIDEO_TICK_RATE` in the runner derives from the active session's fps.
- ffmpeg `fps=` filter gets the session value.

### Phase 4 — Verification

- Unit: escape builders byte-exact against kitty-doc examples; shm
  roundtrip (write → open/read back → unlink); placeholder cell encoding
  (char, diacritics, fg id) for a 2x2 pane; transport selection matrix
  (kitty/ssh/tmux/iterm2).
- Smoke: existing PTY tests must stay green (video view never activates in
  them; detection reads env only).
- Manual: A/B `youtui` in Ghostty — smoothness at 60, CPU vs t=d, seek and
  resize behavior, fallback by faking `SSH_CONNECTION`.

## Risks

| Risk | Mitigation |
|---|---|
| Ghostty lacks/limits `t=s` | Phase 0 gates; keep t=d at 24 fps |
| shm objects leak if terminal never reads | rotate a bounded name set (8), `O_EXCL` + unlink-before-create on reuse |
| Flicker between delete/transmit | rotate ids, delete old only after new placeholders painted |
| tmux/SSH silently broken | env-gated off; t=d fallback |
| 60 fps ffmpeg decode CPU | 720p cap already in place; decode cost scales linearly and was fine at 24 |

## Effort

Spike ~½ day. Phases 1–3 ~1 day. Phase 4 ~½ day.
