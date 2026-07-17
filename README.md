# youtui

A responsive terminal interface for searching, queueing, streaming, and downloading YouTube videos.

## Highlights

- Streaming, cancellable search with incremental results, rolling page-ahead
  prefetch (viewing page N warms N+1), a short-lived repeated-query cache, and
  adaptive Shorts filtering
- Keyboard-first results and queue navigation
- Integrated mpv playback controls, progress, and automatic queue playback
- VLC and mplayer fallback support
- Audio-only, bandwidth-limited, and permanent-download modes
- Responsive layouts for wide, narrow, and small terminals
- Persistent settings with safe, atomic configuration writes

## Requirements

- Rust 1.88 or newer
- [`yt-dlp`](https://github.com/yt-dlp/yt-dlp)
- One supported player: [`mpv`](https://mpv.io/) (recommended), VLC, or mplayer
- `ffmpeg` when merging downloaded video/audio streams or extracting audio

mpv provides the full in-TUI queue and playback experience. VLC and mplayer use a foreground fallback that downloads media to a temporary directory before playback.

## Install

From this repository:

```bash
cargo install --path . --locked
youtui
```

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

## Settings

Press `F2` (or uppercase `S` outside the search bar) to configure:

- Audio-only playback
- Bandwidth limiting
- Temporary-file retention
- Shorts inclusion
- Automatic queue playback
- Permanent-download mode and destination
- Results per page (default: 20, valid range: 1–500)
- An advanced custom yt-dlp format selector

Changes are persisted automatically. Configuration is stored at:

- macOS: `~/Library/Application Support/youtui/config.toml`
- Linux: `$XDG_CONFIG_HOME/youtui/config.toml`, or `~/.config/youtui/config.toml` when unset

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
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
