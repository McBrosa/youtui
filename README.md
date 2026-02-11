# youtui

A rich terminal UI for searching and playing YouTube videos.

## Features

- 🔍 Interactive search with pagination
- ⌨️  Hybrid navigation (arrow keys + number quick-pick)
- 🎨 Rich TUI with colors, borders, and unicode symbols
- ▶️  Plays videos with mpv, vlc, or mplayer
- 📥 Download mode for permanent storage
- 🎵 Audio-only mode

## Installation

Requires: `yt-dlp` and one of `mpv`, `vlc`, or `mplayer`

```bash
cargo install --path .
```

## Usage

Launch with no arguments:

```bash
youtui
```

The app will open with an empty search bar. Type your search query and press Enter to search.

## Settings

Press `S` or `F2` to open the settings modal. All settings are saved immediately to `~/.config/youtui/config.toml` and persist across sessions.

Available settings:
- **Audio Only**: Play audio only (no video)
- **Bandwidth Limit**: Limit video quality to 360p and audio to 128kbps
- **Keep Temporary Files**: Don't delete downloaded files after playback
- **Include YouTube Shorts**: Include Shorts in search results
- **Download Mode**: Save files permanently instead of streaming
- **Download Directory**: Where to save downloaded files
- **Results Per Page**: Number of results to show per page (default: 10)
- **Custom Format**: Override auto-detected format string (for advanced users)

## Controls

**Navigation:**
- `↑/↓` - Move selection
- `Enter` - Play selected video
- `1-9` - Quick-pick video by number
- `n/p` - Next/Previous page

**Commands:**
- `s` - New search
- `S` / `F2` - Open settings
- `h` - Toggle help
- `q` - Quit

## License

MIT
