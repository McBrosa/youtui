# youtui

A responsive terminal interface for searching, queueing, streaming, and downloading YouTube videos.

## Highlights

- Streaming, cancellable search with incremental results, rolling page-ahead
  prefetch (viewing page N warms N+1), a short-lived repeated-query cache, and
  Shorts filtering that preserves regular short videos when identified by YouTube
- Keyboard-first results and queue navigation
- Integrated mpv playback controls, progress, and automatic queue playback
- Independent in-terminal video on Kitty-protocol terminals (including
  Ghostty), without delaying or interrupting the audio player
- VLC and mplayer fallback support
- Audio-only, bandwidth-limited, and permanent-download modes
- Responsive layouts for wide, narrow, and small terminals
- Persistent settings with safe, atomic configuration writes

## Requirements

- Rust 1.90 or newer
- [`yt-dlp`](https://github.com/yt-dlp/yt-dlp)
- One supported player: [`mpv`](https://mpv.io/) (recommended), VLC, or mplayer
- `ffmpeg` when merging/extracting downloads or using the portable block/non-Kitty video renderer

mpv provides the full in-TUI queue and playback experience. VLC and mplayer use a foreground fallback that downloads media to a temporary directory before playback.

With mpv, leaving a result highlighted for about a second starts buffering its
audio in the background. During playback, youtui buffers the next queued track.
One standby player targets 10 seconds of audio with an 8 MiB in-memory packet cache.
Pressing Enter can use that player even while its buffer is filling.

## Install

From this repository:

```bash
cargo install --path . --locked
youtui
```

### Browser interface

The browser interface uses a local Phoenix LiveView application.
Videos play in the browser. Rust supplies search, queues, settings, and media preparation.
Web mode requires yt-dlp, ffmpeg, and ffprobe.
Full YouTube extraction also requires a supported JavaScript runtime, such as Deno, and the yt-dlp EJS components.

For source development, install Elixir and Erlang. Then prepare the Phoenix application:

```bash
cd web
mix setup
cd ..
cargo build --locked
cargo run --locked -- --web
```

After installation with Cargo, start the interface with:

```bash
youtui --web
youtui --web --port 4010
youtui --web --no-open
```

The launcher opens a local browser session and prints its URL.
Press Ctrl+C in the terminal to stop the server and its background processes.
The server binds to `127.0.0.1`.
Web mode does not require mpv or a terminal with graphics support.

The interface includes search, cancellation, pagination, video playback, queue controls, and shared settings.
The browser supplies playback controls for pause, seeking, volume, and fullscreen.
When you select a video, youtui resolves its available streams. The browser can
start playing a combined H.264/AAC MP4 as it downloads. Browsers with native
HLS support can also buffer separate audio and video tracks through the local
server, one segment at a time. Video is capped at 720p. If a compatible stream
is unavailable or playback fails, youtui prepares a local MP4 and reuses it for
subsequent plays during that session.
Temporary files are removed when the background worker exits.

One browser tab owns playback at a time.
Searches remain independent between tabs.
Starting playback in another tab transfers playback ownership.
Queues remain in memory for the running web session.
The web queue holds up to 100 upcoming videos.

For a packaged Phoenix companion, build the release:

```bash
cd web
MIX_ENV=prod mix assets.deploy
MIX_ENV=prod mix release
cd ..
YOUTUI_WEB_RELEASE="$PWD/web/_build/prod/rel/youtui_web" youtui --web
```

The release includes the Erlang runtime and compiled web assets.
Build a separate release for each supported operating system and architecture.
The launcher also discovers a companion at `../lib/youtui/web` relative to the Rust executable directory.
`YOUTUI_WEB_DIR` selects a Phoenix source project instead of the repository default.
`YOUTUI_CONFIG_PATH` selects an alternate settings file for development or isolated tests.

If starting `yt-dlp` itself is slow, install its Python package in a virtual
environment and link the environment’s `yt-dlp` executable at `bin/yt-dlp`
beside the settings file. youtui and its mpv player use that copy for search
and playback. By default they use the executable on your PATH.

On startup, youtui offers to install a missing `yt-dlp` or media player with Homebrew, apt, dnf, or pacman when the platform is recognized. It never treats non-interactive input as permission to run a package manager.

To install dependencies yourself:

```bash
# macOS
brew install yt-dlp mpv ffmpeg

# Debian / Ubuntu
sudo apt install yt-dlp mpv ffmpeg

# Fedora / RHEL
sudo dnf install yt-dlp mpv ffmpeg

# Arch Linux
sudo pacman -S yt-dlp mpv ffmpeg
```

## Usage

```bash
youtui
youtui --help
youtui --version
```

Type a query in the search bar and press Enter. The footer always shows the shortcuts relevant to the focused panel; press `?` for the complete in-app guide.

Search uses YouTube’s relevance ranking by default. Turn off **Sort by newest**
in Settings to include older matches. The first matching video is selectable
while the rest of the page loads.

### Controls

| Context | Keys | Action |
| --- | --- | --- |
| Global | `Tab` / `Shift+Tab` | Cycle panel focus forward / backward |
| Global | `F2` | Open settings |
| Global | `?` or `h` | Open help |
| Global | `q`, `Esc`, or `Ctrl+C` | Quit when not editing text, showing a modal, or searching |
| Active search | `Esc` | Cancel search and keep results already received |
| Search | `Enter` / `Esc` | Submit / cancel editing |
| Results | `↑` `↓` or `j` `k` | Move selection |
| Results | `g` / `G` | Jump to first / last result on the page |
| Results | `Enter` | Play; with mpv, also add to the queue |
| Results | digits, then `Enter` | Quick-pick the displayed result number |
| Results | `n` / `p` | Next / previous page |
| Results | `/` or `s` | Focus search |
| Queue | `Enter` | Move the selected track to the front and play it |
| Queue | `Delete` or `Backspace` | Remove the selected track |
| Queue | `n` / `c` | Next track / clear queue |
| Playback | `Space` | Pause or resume |
| Playback | `<` / `>` | Seek backward / forward 10 seconds |
| Playback | `+` / `-` | Raise / lower volume |
| Playback | `m` | Mute or restore volume |
| Playback | `v` | Toggle the in-terminal video pane |

## Settings

Press `F2` (or uppercase `S` outside the search bar) to configure:

- Audio-only playback
- Bandwidth limiting
- Temporary-file retention
- Shorts inclusion
- Automatic queue playback
- Permanent-download mode and destination
- Results per page (default: 20, valid range: 1–500)
- Terminal video renderer and pixel-video source quality (144p–720p). Video
  resolves and decodes independently from the audio-only queue player, so a
  slow or unavailable video stream cannot block audio. Kitty uses shared
  memory when available; other transports use bounded direct or block frames.
- An advanced custom yt-dlp format selector

Changes are persisted automatically. Configuration is stored at:

- macOS: `~/Library/Application Support/youtui/config.toml`
- Linux: `$XDG_CONFIG_HOME/youtui/config.toml`, or `~/.config/youtui/config.toml` when unset

## Development

```bash
cargo +1.90.0 check --all-targets --all-features --locked
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

`Cargo.toml` defines the Rust and Clippy rules that CI runs. Handle `Result` and
other `must_use` values. For an unsafe block, document the specific invariant
that makes the operation safe. Scope any lint exception to the affected item
and include `reason = "…"` in the lint attribute.

Keep queue selection in `Queue` and player ownership in `App`. Keyboard actions
and automatic transitions share the same queue-front loading code. When
changing streaming search, check how much data each progress update copies as
the result set grows. Borrow rows for rendering and move owned data across
worker channels where possible.

Run the web checks from `web/`:

```bash
mix precommit
MIX_ENV=prod mix assets.deploy
MIX_ENV=prod mix release
```

The test suite includes hermetic audio-only playback and streaming-search smoke
tests. They drive the compiled TUI through a pseudo-terminal and use local
`yt-dlp`/`mpv` protocol fakes, so they require neither network access nor an
audio device. Run them alone with
`cargo test --test audio_only_playback_smoke --locked` and
`cargo test --test search_streaming_smoke --locked`.

CI checks the declared minimum Rust version, formatting, strict Clippy, tests, the release build, and RustSec advisories.

## License

[MIT](LICENSE)
