use std::time::{Duration, Instant};
use std::path::Path;
use crossterm::event::{self, Event};
use anyhow::Result;
use colored::Colorize;
use crate::ui::{App, handle_key_event, layout::render_ui, terminal::Tui};
use crate::ui::app::AppAction;
use crate::config::Config;
use crate::player_manager::PlayerManager;
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
                    let result = app.results[idx].clone();

                    // Add to queue
                    app.queue.push_back(result.clone());

                    if crate::player::supports_background_playback(config.player) {
                        // Background playback with mpv
                        if app.player_manager.is_none() {
                            match PlayerManager::new() {
                                Ok(mut pm) => {
                                    let url = format!("https://www.youtube.com/watch?v={}", result.id);
                                    if pm.play(&url, &result.title).is_ok() {
                                        app.player_manager = Some(pm);
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Failed to create player: {}", e);
                                }
                            }
                        }
                    } else {
                        // Legacy: exit TUI, play externally, return
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

                        *terminal = crate::ui::terminal::init_terminal()?;

                        // Remove from queue since it was played inline
                        app.queue.pop_front();
                    }
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

        // Poll player status and check for EOF
        let mut player_finished = false;
        if let Some(ref mut player) = app.player_manager {
            let _ = player.update_status();

            if player.is_eof() {
                player_finished = true;
            }
        }

        if player_finished {
            // Remove the finished track from queue front
            app.queue.pop_front();

            if !app.queue.is_empty() {
                // Play next track
                if let Some(track) = app.queue.get(0) {
                    let url = format!("https://www.youtube.com/watch?v={}", track.id);
                    let title = track.title.clone();
                    if let Some(ref mut player) = app.player_manager {
                        if player.play(&url, &title).is_err() {
                            app.player_manager = None;
                        }
                    }
                }
            } else {
                // Queue empty, stop player
                app.player_manager = None;
            }
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
