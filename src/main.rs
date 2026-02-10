mod cli;
mod cleanup;
mod config;
mod display;
mod player;
mod search;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;

use cli::Cli;
use cleanup::{ManagedTempDir, is_interrupted, setup_signal_handler};
use config::Config;
use display::{UserAction, get_selection, show_controls, show_results};
use player::{PlaybackResult, detect_player, play_video};
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

    let mut page: usize = 0;

    // Main loop
    loop {
        if is_interrupted() {
            break;
        }

        let total = search.results.len();
        let start = page * page_size;
        let end = (start + page_size).min(total);
        let page_slice = &search.results[start..end];

        // "has next" is true if there are more cached results OR yt-dlp hasn't been exhausted
        let has_next = end < total || !search.exhausted;
        let has_prev = page > 0;

        show_results(page_slice, page, page_size, total, search.exhausted);

        match get_selection(start, page_slice.len(), has_next, has_prev) {
            UserAction::Quit => break,
            UserAction::NextPage => {
                page += 1;
                // Fetch from yt-dlp only if we don't already have this page cached
                search.ensure_page(page)?;
                // If the fetch didn't produce enough results, clamp back
                let max_page = if search.results.is_empty() {
                    0
                } else {
                    (search.results.len() - 1) / page_size
                };
                if page > max_page {
                    page = max_page;
                    eprintln!("{}", "No more results available.".yellow());
                }
            }
            UserAction::PrevPage => {
                page = page.saturating_sub(1);
                // Previous pages are always cached — no fetch needed
            }
            UserAction::NewSearch(new_query) => {
                if is_interrupted() {
                    break;
                }
                println!("{} {}", "Searching for:".blue(), new_query);
                search.reset(&new_query);
                search.ensure_page(0)?;
                page = 0;
                if search.results.is_empty() {
                    eprintln!("{}", "No results found.".red());
                    println!("{}", "Press Enter to continue...".green());
                    let mut buf = String::new();
                    let _ = std::io::stdin().read_line(&mut buf);
                }
            }
            UserAction::Play(index) => {
                if is_interrupted() {
                    break;
                }
                let result = &search.results[index];
                println!("{} {} (ID: {})", "Selected:".green(), result.title, result.id);

                show_controls(config.player);

                match play_video(&config, &result.id, &result.title, &result.safe_title(), temp_dir.path())? {
                    PlaybackResult::Finished => break,
                    PlaybackResult::ReturnToMenu => continue,
                    PlaybackResult::Error(msg) => {
                        eprintln!("{} {}", "Playback error:".red(), msg);
                    }
                }
            }
        }
    }

    // temp_dir dropped here -> cleanup runs
    Ok(())
}
