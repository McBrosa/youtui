mod cli;
mod cleanup;
mod config;
mod display;
mod ipc;
mod player;
mod queue;
mod search;
mod ui;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;

use cli::Cli;
use cleanup::{ManagedTempDir, setup_signal_handler};
use config::Config;
use player::detect_player;
use search::{PaginatedSearch, check_ytdlp};

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Check dependencies
    check_ytdlp()?;
    let player = detect_player()?;

    // Build config
    let config = Config::from_cli(&cli, player);

    // Create managed temp dir
    let temp_dir = ManagedTempDir::new(config.keep_temp)?;

    // Set up Ctrl-C handler
    setup_signal_handler();

    // Paginated search — fetches one page at a time, caches previous pages
    let page_size = config.num_results;
    let mut search = PaginatedSearch::new(&config.query, page_size, !config.include_shorts);

    // Fetch first page
    println!("{} {}", "Searching for:".blue(), config.query);
    search.ensure_page(0)?;

    if search.results.is_empty() {
        eprintln!("{}", "No results found.".red());
        if !config.include_shorts {
            eprintln!(
                "{}",
                "Try again with -i/--include-shorts option if you want to include shorter videos".yellow()
            );
        }
        std::process::exit(1);
    }

    // Initialize TUI
    let terminal = ui::init_terminal()?;
    let mut terminal_guard = ui::TerminalGuard::new(terminal);

    // Create app state
    let mut app = ui::App::new(config.query.clone(), page_size);
    app.results = search.results.clone();
    app.total_results = search.results.len();
    app.exhausted = search.exhausted;

    // Run TUI
    let result = ui::run_app(terminal_guard.get_mut(), app, &config, &mut search, temp_dir.path());

    // Terminal cleanup happens via Drop guard
    drop(terminal_guard);

    result
}
