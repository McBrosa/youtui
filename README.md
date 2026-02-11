# youtui

A rich terminal UI for searching and playing YouTube videos.

## Features

- 🔍 Interactive search with pagination
- ⌨️  Hybrid navigation (arrow keys + number quick-pick)
- 🎨 Rich TUI with colors, borders, and unicode symbols
- ▶️  Plays videos with mpv, vlc, or mplayer
- 📥 Download mode for permanent storage
- 🎵 Audio-only mode

## Built With

**Core Libraries:**
- [ratatui](https://github.com/ratatui/ratatui) - Terminal UI framework
- [crossterm](https://github.com/crossterm-rs/crossterm) - Cross-platform terminal manipulation
- [serde](https://github.com/serde-rs/serde) + [toml](https://github.com/toml-rs/toml) - Configuration serialization

**External Dependencies:**
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) - YouTube video fetching and metadata
- [mpv](https://mpv.io/) - Media player with IPC support for background playback

## Installation

**System Requirements:**
- macOS (with Homebrew) or Linux (Debian/Ubuntu, Fedora/RHEL, or Arch)
- Internet connection (for first-time dependency setup)

**First Launch:**
```bash
cargo install --path .
youtui
```

On first launch, youtui will automatically detect if `yt-dlp` and `mpv` are missing and offer to install them using your system's package manager (Homebrew on macOS, apt/dnf/pacman on Linux). Just accept the prompt and youtui will handle the setup.

**Manual Installation (optional):**

If you prefer to install dependencies yourself:

**macOS:**
```bash
brew install mpv yt-dlp
```

**Linux (Debian/Ubuntu):**
```bash
sudo apt install mpv yt-dlp
```

**Linux (Fedora/RHEL):**
```bash
sudo dnf install mpv yt-dlp
```

**Linux (Arch):**
```bash
sudo pacman -S mpv yt-dlp
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
