mod cleanup;
mod config;
mod display;
mod ipc;
mod player;
mod player_manager;
mod queue;
mod search;
mod ui;

use anyhow::Result;

use cleanup::{ManagedTempDir, setup_signal_handler};
use config::Config;
use player::detect_player;
use search::{PaginatedSearch, check_ytdlp};
use ui::FocusedPanel;

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
    let mut app = ui::App::new(String::new(), config.results_per_page, config.clone());
    app.focused_panel = FocusedPanel::SearchBar;

    // Search manager with no initial query
    let mut search = PaginatedSearch::new("", config.results_per_page, !config.include_shorts);

    // Run TUI loop
    let result = ui::run_app(terminal_guard.get_mut(), app, &config, &mut search, temp_dir.path());

    drop(terminal_guard);
    result
}
