# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Current use is local. The user wants public availability later.

The current workflow is to search YouTube, choose videos, manage a queue, and watch in a browser on the same machine.

The specific public audience and release model remain open decisions. Public availability does not yet mean a hosted website or remote access.

## Product Purpose

youtui provides terminal and browser interfaces for YouTube search and playback.

The browser interface starts with `youtui --web`. The launcher stays running in the terminal while the browser handles video playback.

Success means users can search, queue, and watch videos locally without duplicate preparation jobs or a separate desktop player.

## Operating Context

- The existing Rust terminal interface remains available.
- The web interface uses Phoenix LiveView with a Rust backend.
- The web server binds to the loopback address. The launcher provides a signed browser session through a launch token.
- Web playback uses the browser's native video controls. It does not require mpv or a terminal with video support.
- Search and preparation of new videos require internet access.
- Web media preparation uses `yt-dlp`, `ffmpeg`, and `ffprobe`. YouTube extraction also requires a supported JavaScript runtime and EJS support.
- Settings persist and are shared between terminal and web modes.
- Pressing `Ctrl+C` in the launching terminal stops the web services and media jobs.

## Capabilities and Constraints

- Search supports cancellation and pagination.
- Users can add videos to a queue, select a video, skip, and clear the queue.
- Native browser controls provide pause, seek, volume, and fullscreen.
- Only one media preparation job can run at a time. Additional playback requests cannot start another job while preparation is active.
- Prepared videos are reused during the current session. This cache does not persist between runs.
- Browsers can start playback while buffering a progressive MP4 or a native HLS presentation with one video rendition up to 720p. A prepared local MP4 supplies the fallback.
- One browser tab owns playback at a time. Playing from another tab transfers ownership. Tabs can search independently.
- The upcoming queue is stored in memory and is limited to 100 entries. Saved playlists are not implemented.
- HLS playback uses a fixed rendition for the selected video. The app is not an offline mirror of YouTube.

Public release requirements remain undecided. These include distribution, hosting, remote access, supported browsers, and an accessibility target.

## Brand Commitments

The product name is `youtui`.

The browser interface follows the user's Positivus reference: light surfaces,
dark outlines, and gold highlights in place of green. This is a reference-led
adaptation for a video workspace, not a copy of the landing page.

## Evidence on Hand

- `README.md` describes installation, dependencies, controls, and web mode.
- `docs/plans/2026-09-30-web-interface.md` records the web architecture and protocol.
- `tests/web_backend_smoke.rs` and `tests/web_launcher_smoke.rs` cover backend and launcher behavior.
- `web/test/youtui_web/live/player_live_test.exs` covers browser interface behavior, including duplicate playback requests.
- The local implementation has been tested with browser playback and repeated Play clicks.

No public launch date, user research, testimonials, or adoption claims have been established.

## Product Principles

1. Keep access local until public release requirements are agreed.
2. Preserve terminal mode while adding browser playback.
3. Prevent overlapping preparation jobs and reuse prepared videos within a session.
4. Keep settings shared between interfaces.
5. Distinguish current capabilities from future plans.
