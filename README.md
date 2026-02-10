# yt-search-play

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

```bash
yt-search-play "search query"
```

### Keyboard Controls

**Navigation:**
- `↑/↓` - Move selection
- `Enter` - Play selected video
- `1-9` - Quick-pick video by number
- `n/p` - Next/Previous page

**Commands:**
- `s` - New search
- `h` - Toggle help
- `q` - Quit

### Options

- `-n, --num <NUM>` - Number of results per page (default: 10)
- `-a, --audio-only` - Audio only mode
- `-f, --format <FMT>` - Video format (default: best)
- `-d, --download` - Download permanently
- `--download-dir <DIR>` - Download directory
- `-i, --include-shorts` - Include YouTube Shorts
- `-k, --keep` - Keep temp files

## License

MIT
