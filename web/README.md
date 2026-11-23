# Youtui web companion

This Phoenix LiveView application supplies the browser interface for youtui.
The Rust launcher supplies the worker executable, HTTP port, and browser session token.
The browser streams compatible MP4 or HLS sources as they arrive. If streaming fails, the Rust worker prepares a local MP4 with yt-dlp and ffmpeg.

## Development

Install Elixir, Erlang, yt-dlp, ffmpeg, and ffprobe.
Install the YouTube extraction dependencies listed in the root README.
From this directory, prepare the web application:

```bash
mix setup
cd ..
cargo run --locked -- --web
```

Use the launch URL that youtui prints.
The `/start` route creates the browser session and redirects to the interface.
Direct requests to the interface or media routes require that session.
Press Ctrl+C in the launch terminal to stop the application.

## Verification

From this directory, run the application checks:

```bash
mix precommit
MIX_ENV=prod mix assets.deploy
MIX_ENV=prod mix release
```

The production release includes the Erlang runtime and web assets.
Set `YOUTUI_WEB_RELEASE` to its absolute directory before `youtui --web`.
The root README describes the launcher environment and installation paths.

## Architecture

`Youtui.Backend` supervises the Rust worker through an Elixir Port.
The connection uses JSON messages with four-byte length prefixes.
`Youtui.Media` registers completed files under the worker cache directory.
The media controller supports byte ranges for seeking.
The LiveView keeps the video element mounted during search and queue updates.

The protocol contract is in `docs/plans/2026-09-30-web-interface.md` at the repository root.
