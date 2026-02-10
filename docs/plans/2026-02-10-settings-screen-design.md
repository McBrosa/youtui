# Settings Screen Design

## Overview

Remove all command-line arguments and replace with an in-app settings modal. Launch with no parameters opens empty TUI with search bar focused. Settings persist to `~/.config/yt-search-play/config.toml` and save immediately on change.

## Goals

- Launch with no arguments: `yt-search-play` (no query required)
- Open to empty search bar, user types first query
- Press 'S' or F2 to open settings modal overlay
- All settings (audio-only, bandwidth, downloads, etc.) configurable in-app
- Changes save immediately to config file
- Config persists across sessions

## Architecture

### Config File

**Location:** `~/.config/yt-search-play/config.toml`

**Structure:**
```toml
[playback]
audio_only = false
bandwidth_limit = false
keep_temp = false
include_shorts = false

[downloads]
download_mode = false
download_dir = "~/Downloads"

[display]
results_per_page = 20

[format]
custom_format = ""  # empty = auto-detect
```

**First Run:**
- Check if config file exists
- If missing: Create directory + file with defaults
- If exists: Load and parse TOML
- Continue to TUI launch

**Player Detection:**
- Player (mpv/vlc/mplayer) auto-detected on each launch (not saved)
- Ensures player availability without stale config

### App State Extensions

Add to `src/ui/app.rs`:
```rust
pub struct App {
    // Existing fields...
    pub settings_open: bool,
    pub settings_selected_index: usize,
    pub settings_editing: Option<SettingsField>,
    pub config: Config,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsField {
    DownloadDir,
    ResultsPerPage,
    CustomFormat,
}
```

**Settings Item Mapping:**
- Index 0-1: Section header (skip)
- Index 2: Audio Only (checkbox)
- Index 3: Bandwidth Limit (checkbox)
- Index 4: Keep Temp (checkbox)
- Index 5: Include Shorts (checkbox)
- Index 6-8: Section header (skip)
- Index 9: Download Mode (checkbox)
- Index 10: Download Dir (text field)
- Index 11-13: Section header (skip)
- Index 14: Results Per Page (text field)
- Index 15-17: Section header (skip)
- Index 18: Custom Format (text field)

### CLI Module Removal

**Before:**
```rust
// src/cli.rs with clap Parser
pub struct Cli {
    pub num: usize,
    pub audio_only: bool,
    // ... many flags
    pub query: Vec<String>,
}
```

**After:**
```rust
// src/cli.rs - empty or removed entirely
// main.rs just launches TUI with no args
```

## UI Design

### Settings Modal Layout

Centered overlay (60% width, 70% height):

```
┌────────────────── Settings ──────────────────┐
│                                              │
│  Playback                                    │
│  ────────                                    │
│  [✓] Audio Only                              │
│  [ ] Bandwidth Limit (360p video, 128k audio)│
│  [ ] Keep Temporary Files                    │
│  [ ] Include YouTube Shorts                  │
│                                              │
│  Downloads                                   │
│  ─────────                                   │
│  [ ] Download Mode (save permanently)        │
│  Download Directory: [~/Downloads_______]    │
│                                              │
│  Display                                     │
│  ───────                                     │
│  Results Per Page: [20__]                    │
│                                              │
│  Advanced                                    │
│  ────────                                    │
│  Custom Format: [___________________]        │
│  (leave empty for auto)                      │
│                                              │
│  Press S/F2/Esc to close                     │
└──────────────────────────────────────────────┘
```

**Visual Indicators:**
- Selected item: Yellow background, black text
- Checkboxes: `[✓]` checked, `[ ]` unchecked
- Text fields: Value with cursor `█` when editing
- Section headers: Bold, non-selectable
- Help text: Dark gray

**Rendering Helper Functions:**

```rust
fn checkbox_item(idx: usize, label: &str, checked: bool, selected: usize) -> ListItem {
    let checkbox = if checked { "[✓]" } else { "[ ]" };
    let text = format!("  {} {}", checkbox, label);
    let style = if idx == selected {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default().fg(Color::White)
    };
    ListItem::new(text).style(style)
}

fn text_field_item(idx: usize, label: &str, value: &str, selected: usize,
                   editing: &Option<SettingsField>) -> ListItem {
    let is_editing = editing.is_some() && idx == selected;
    let cursor = if is_editing { "█" } else { "" };
    let text = format!("  {}: [{}{}]", label, value, cursor);
    let style = if idx == selected {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default().fg(Color::White)
    };
    ListItem::new(text).style(style)
}
```

## Event Handling

### Opening/Closing Modal

**Global key (works from any panel):**
- `S` or `F2`: Toggle settings modal
- When modal opens: Save current state, pause other input
- When modal closes: Resume normal TUI interaction

### Navigation Mode

**Arrow keys:**
- Up/Down: Move selection, skip section headers
- Wrap at top/bottom

**Activation:**
- Space or Enter: Toggle checkbox OR enter edit mode for text
- Esc: Close modal

### Edit Mode

**When editing text field:**
- All character input appends to field value
- Backspace: Delete last character
- Enter or Esc: Exit edit mode, save value
- Arrow keys: Disabled (or Esc first to exit edit)

**Validation:**
- Results Per Page: Only accept digits, clamp to 1-100
- Download Dir: Accept any characters (expand ~ on save)
- Custom Format: Accept any characters

### Immediate Save

Every change triggers `config.save()`:
```rust
impl Config {
    pub fn toggle_audio_only(&mut self) -> Result<()> {
        self.audio_only = !self.audio_only;
        self.save()
    }

    pub fn set_download_dir(&mut self, dir: String) -> Result<()> {
        self.download_dir = dir;
        self.save()
    }

    fn save(&self) -> Result<()> {
        let path = dirs::config_dir()
            .ok_or("No config directory")?
            .join("yt-search-play/config.toml");
        fs::create_dir_all(path.parent().unwrap())?;
        let toml = toml::to_string_pretty(self)?;
        fs::write(path, toml)?;
        Ok(())
    }
}
```

## Main.rs Integration

### New Launch Flow

```rust
fn main() -> Result<()> {
    // Load or create config (no CLI parsing)
    let mut config = Config::load_or_create()?;

    // Check dependencies
    check_ytdlp()?;
    let player = detect_player()?;
    config.player = player;

    // Create managed temp dir
    let temp_dir = ManagedTempDir::new(config.keep_temp)?;
    setup_signal_handler();

    // Initialize TUI with empty query
    let terminal = ui::init_terminal()?;
    let mut terminal_guard = ui::TerminalGuard::new(terminal);

    // App starts with empty search, focus on search bar
    let mut app = ui::App::new(String::new(), config.results_per_page);
    app.focused_panel = FocusedPanel::SearchBar;
    app.config = config.clone();

    // Search manager with no initial query
    let mut search = PaginatedSearch::new("", config.results_per_page, !config.include_shorts);

    // Run TUI loop
    let result = ui::run_app(terminal_guard.get_mut(), app, &mut config, &mut search, temp_dir.path());

    drop(terminal_guard);
    result
}
```

**Key differences from current:**
- No `Cli::parse()` call
- No required query argument
- Empty initial search
- SearchBar focused by default
- Config loaded from file, not CLI

### Config Module Rewrite

New `src/config.rs`:
```rust
use serde::{Deserialize, Serialize};
use std::fs;
use anyhow::{Context, Result};
use crate::player::PlayerType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip)]
    pub player: PlayerType,  // Auto-detected, not saved

    pub audio_only: bool,
    pub bandwidth_limit: bool,
    pub keep_temp: bool,
    pub include_shorts: bool,
    pub download_mode: bool,
    pub download_dir: String,
    pub results_per_page: usize,
    pub custom_format: String,
}

impl Config {
    pub fn load_or_create() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let contents = fs::read_to_string(&config_path)
                .context("Failed to read config file")?;
            let mut config: Config = toml::from_str(&contents)
                .context("Failed to parse config file")?;
            config.player = PlayerType::Mpv; // Placeholder, set in main
            Ok(config)
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        fs::create_dir_all(config_path.parent().unwrap())?;
        let toml_string = toml::to_string_pretty(self)?;
        fs::write(config_path, toml_string)?;
        Ok(())
    }

    fn config_path() -> Result<std::path::PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or(anyhow::anyhow!("No config directory found"))?;
        Ok(config_dir.join("yt-search-play/config.toml"))
    }

    pub fn toggle_audio_only(&mut self) -> Result<()> {
        self.audio_only = !self.audio_only;
        self.save()
    }

    pub fn toggle_bandwidth_limit(&mut self) -> Result<()> {
        self.bandwidth_limit = !self.bandwidth_limit;
        self.save()
    }

    pub fn toggle_keep_temp(&mut self) -> Result<()> {
        self.keep_temp = !self.keep_temp;
        self.save()
    }

    pub fn toggle_include_shorts(&mut self) -> Result<()> {
        self.include_shorts = !self.include_shorts;
        self.save()
    }

    pub fn toggle_download_mode(&mut self) -> Result<()> {
        self.download_mode = !self.download_mode;
        self.save()
    }

    pub fn format(&self) -> String {
        if !self.custom_format.is_empty() {
            self.custom_format.clone()
        } else {
            resolve_format(self.audio_only, self.bandwidth_limit)
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            player: PlayerType::Mpv,
            audio_only: false,
            bandwidth_limit: false,
            keep_temp: false,
            include_shorts: false,
            download_mode: false,
            download_dir: dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("Downloads")
                .to_string_lossy()
                .to_string(),
            results_per_page: 20,
            custom_format: String::new(),
        }
    }
}

fn resolve_format(audio_only: bool, limit: bool) -> String {
    match (audio_only, limit) {
        (true, true) => "bestaudio[abr<=128]/bestaudio/best".to_string(),
        (true, false) => "bestaudio/best".to_string(),
        (false, true) => "bestvideo[height<=360]+bestaudio/best[height<=360]/best".to_string(),
        (false, false) => "bestvideo+bestaudio/best".to_string(),
    }
}
```

**Does this complete the design? Any concerns?**