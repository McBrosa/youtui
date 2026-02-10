use std::time::{Duration, Instant};
use std::path::Path;
use crossterm::event::{self, Event};
use anyhow::Result;
use colored::Colorize;
use crate::ui::{App, AppEvent, handle_key_event, layout::render_ui, terminal::Tui};
use crate::ui::app::AppAction;
use crate::config::Config;
use crate::search::PaginatedSearch;

const TICK_RATE: Duration = Duration::from_millis(250);

pub fn run_app(
    terminal: &mut Tui,
    mut app: App,
    config: &Config,
    search: &mut PaginatedSearch,
    temp_dir: &Path,
) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| render_ui(f, &app))?;

        // Handle pending actions
        match std::mem::replace(&mut app.pending_action, AppAction::None) {
            AppAction::Play(idx) => {
                if idx < app.results.len() {
                    let result = &app.results[idx];
                    // Exit TUI temporarily for playback
                    crate::ui::terminal::restore_terminal(terminal)?;

                    println!("{} {}", "Playing:".green(), result.title);
                    crate::display::show_controls(config.player);

                    let _ = crate::player::play_video(
                        config,
                        &result.id,
                        &result.title,
                        &result.safe_title(),
                        temp_dir,
                    );

                    // Re-enter TUI
                    *terminal = crate::ui::terminal::init_terminal()?;
                    println!("\nReturning to search results...");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            AppAction::NewSearch(query) => {
                app.query = query;
                search.reset(&app.query);
                search.ensure_page(0)?;
                app.results = search.results.clone();
                app.total_results = search.results.len();
                app.exhausted = search.exhausted;
                app.page = 0;
                app.selected_index = 0;
            }
            AppAction::FetchNextPage => {
                search.ensure_page(app.page)?;
                app.results = search.results.clone();
                app.total_results = search.results.len();
                app.exhausted = search.exhausted;
            }
            AppAction::None => {}
        }

        let timeout = TICK_RATE
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(&mut app, key);
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
