use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, TryRecvError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use colored::Colorize;
use crossterm::event::{self, Event};

use crate::cleanup::{INTERRUPTED, ManagedTempDir};
use crate::config::clamp_results_per_page;
use crate::player_manager::PlayerManager;
use crate::search::PaginatedSearch;
use crate::ui::app::{AppAction, SearchPhase};
use crate::ui::{App, handle_key_event, layout::render_ui, terminal::Tui};

const TICK_RATE: Duration = Duration::from_millis(250);
const SEARCH_POLL_RATE: Duration = Duration::from_millis(50);
const SEARCH_CACHE_CAPACITY: usize = 8;
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchCacheKey {
    query: String,
    page_size: usize,
    filter_shorts: bool,
}

impl SearchCacheKey {
    fn new(query: &str, page_size: usize, filter_shorts: bool) -> Self {
        Self {
            // Preserve case: a search can be a case-sensitive video/channel ID.
            query: query.trim().to_string(),
            page_size: clamp_results_per_page(page_size),
            filter_shorts,
        }
    }
}

struct SearchCacheEntry {
    key: SearchCacheKey,
    state: PaginatedSearch,
    fetched_at: Instant,
}

#[derive(Default)]
struct SearchCache {
    entries: VecDeque<SearchCacheEntry>,
}

impl SearchCache {
    fn insert(&mut self, state: &PaginatedSearch) {
        // A partial first page is not useful as an instant cache hit. It is
        // still retained by the active search state and can be cancelled safely.
        if state.results.len() < state.page_size && !state.exhausted {
            return;
        }

        self.remove_expired();
        let key = SearchCacheKey::new(state.query(), state.page_size, state.filter_shorts);
        let now = Instant::now();
        let fetched_at = self
            .entries
            .iter()
            .position(|entry| entry.key == key)
            .and_then(|index| self.entries.remove(index))
            .map_or(now, |previous| {
                if state.results.len() > previous.state.results.len()
                    || state.exhausted != previous.state.exhausted
                {
                    now
                } else {
                    previous.fetched_at
                }
            });
        self.entries.push_front(SearchCacheEntry {
            key,
            state: state.clone(),
            fetched_at,
        });
        self.entries.truncate(SEARCH_CACHE_CAPACITY);
    }

    fn get(
        &mut self,
        query: &str,
        page_size: usize,
        filter_shorts: bool,
    ) -> Option<PaginatedSearch> {
        self.remove_expired();
        let key = SearchCacheKey::new(query, page_size, filter_shorts);
        let index = self.entries.iter().position(|entry| entry.key == key)?;
        let entry = self.entries.remove(index)?;
        let state = entry.state.clone();
        self.entries.push_front(entry);
        Some(state)
    }

    fn remove_expired(&mut self) {
        self.entries
            .retain(|entry| entry.fetched_at.elapsed() <= SEARCH_CACHE_TTL);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchRequest {
    New,
    NextPage { target_page: usize },
    BackgroundPrefetch,
}

enum SearchMessage {
    Progress {
        generation: u64,
        request: SearchRequest,
        state: PaginatedSearch,
    },
    Finished(SearchOutcome),
}

struct SearchOutcome {
    generation: u64,
    state: PaginatedSearch,
    result: Result<()>,
}

struct SearchWorker {
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct SearchRuntime {
    generation: u64,
    worker: Option<SearchWorker>,
    cache: SearchCache,
}

impl SearchWorker {
    fn cancel_and_join(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SearchWorker {
    fn drop(&mut self) {
        // Also cover early `?` returns from terminal/event processing.
        self.stop();
    }
}

pub fn run_app(
    terminal: &mut Tui,
    mut app: App,
    search: &mut PaginatedSearch,
    temp_dir: &mut ManagedTempDir,
) -> Result<()> {
    let (search_tx, search_rx) = mpsc::channel();
    let mut search_runtime = SearchRuntime::default();
    let mut last_tick = Instant::now();
    let mut dirty = true;

    loop {
        if app.should_quit || INTERRUPTED.load(Ordering::SeqCst) {
            break;
        }

        if sync_runtime_settings(&mut app, search, temp_dir) {
            dirty = true;
        }

        if process_pending_action(
            terminal,
            &mut app,
            search,
            temp_dir,
            &search_tx,
            &mut search_runtime,
        )? {
            dirty = true;
        }

        if drain_search_results(
            &search_rx,
            search_runtime.generation,
            &mut app,
            search,
            &mut search_runtime.cache,
        )? {
            dirty = true;
        }

        if dirty {
            terminal.draw(|frame| render_ui(frame, &app))?;
            dirty = false;
        }

        // The video view repaints on the tick, so 250ms would cap it at 4fps;
        // tighten the tick to keep frame delivery close to the pipeline rate.
        let tick_rate = if app.video_view {
            app.video.tick_rate()
        } else {
            TICK_RATE
        };
        let timeout =
            event_poll_timeout(tick_rate.saturating_sub(last_tick.elapsed()), app.loading);
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    handle_key_event(&mut app, key);
                    dirty = true;
                }
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            let terminal_size = terminal.size()?;
            if poll_player(&mut app, (terminal_size.width, terminal_size.height)) {
                dirty = true;
            }
            last_tick = Instant::now();
        }
    }

    if let Some(worker) = search_runtime.worker {
        worker.cancel_and_join();
    }

    Ok(())
}

fn event_poll_timeout(until_tick: Duration, search_active: bool) -> Duration {
    if search_active {
        until_tick.min(SEARCH_POLL_RATE)
    } else {
        until_tick
    }
}

fn process_pending_action(
    terminal: &mut Tui,
    app: &mut App,
    search: &mut PaginatedSearch,
    temp_dir: &ManagedTempDir,
    search_tx: &Sender<SearchMessage>,
    search_runtime: &mut SearchRuntime,
) -> Result<bool> {
    match std::mem::replace(&mut app.pending_action, AppAction::None) {
        AppAction::Play(index) => play_result(terminal, app, index, temp_dir),
        AppAction::NewSearch(query) => {
            search_runtime.cache.insert(search);
            app.results.clear();
            app.total_results = 0;
            app.exhausted = false;
            app.page = 0;
            app.selected_index = 0;
            app.loading = true;
            app.search_phase = Some(SearchPhase::Initial);
            app.status_message = None;

            if let Some(cached) =
                search_runtime
                    .cache
                    .get(&query, app.page_size, !app.config.include_shorts)
            {
                search_runtime.generation = search_runtime.generation.wrapping_add(1);
                if let Some(worker) = search_runtime.worker.take() {
                    worker.cancel_and_join();
                }
                apply_cached_search(cached, app, search);
                return Ok(true);
            }

            *search = PaginatedSearch::new(&query, app.page_size, !app.config.include_shorts);
            spawn_search(
                search.clone(),
                0,
                SearchRequest::New,
                search_tx,
                &mut search_runtime.generation,
                &mut search_runtime.worker,
                app,
            );
            Ok(true)
        }
        AppAction::FetchNextPage(target_page) => {
            app.loading = true;
            app.search_phase = Some(SearchPhase::RequestedPage { target_page });
            app.status_message = None;
            let request = SearchRequest::NextPage { target_page };
            spawn_search(
                search.clone(),
                target_page,
                request,
                search_tx,
                &mut search_runtime.generation,
                &mut search_runtime.worker,
                app,
            );
            Ok(true)
        }
        AppAction::PrefetchNextPage(from_page) => {
            let target_page = from_page.saturating_add(1);
            app.loading = true;
            app.search_phase = Some(SearchPhase::Prefetch { target_page });
            app.status_message = None;
            spawn_search(
                search.clone(),
                from_page,
                SearchRequest::BackgroundPrefetch,
                search_tx,
                &mut search_runtime.generation,
                &mut search_runtime.worker,
                app,
            );
            Ok(true)
        }
        AppAction::CancelSearch => {
            search_runtime.generation = search_runtime.generation.wrapping_add(1);
            if let Some(worker) = search_runtime.worker.take() {
                worker.cancel_and_join();
            }
            app.loading = false;
            app.search_phase = None;
            search_runtime.cache.insert(search);
            Ok(true)
        }
        AppAction::None => Ok(false),
    }
}

fn spawn_search(
    mut state: PaginatedSearch,
    page: usize,
    request: SearchRequest,
    tx: &Sender<SearchMessage>,
    generation: &mut u64,
    active_worker: &mut Option<SearchWorker>,
    app: &mut App,
) {
    *generation = generation.wrapping_add(1);
    let current_generation = *generation;
    if let Some(worker) = active_worker.take() {
        worker.cancel_and_join();
    }

    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let tx = tx.clone();
    let spawn_result = thread::Builder::new()
        .name(format!("youtui-search-{current_generation}"))
        .spawn(move || {
            let result = state
                .ensure_page_with_cancel_and_progress(page, &worker_cancelled, |progress| {
                    let _ = tx.send(SearchMessage::Progress {
                        generation: current_generation,
                        request,
                        state: progress.clone(),
                    });
                })
                .map(|_| ());
            let _ = tx.send(SearchMessage::Finished(SearchOutcome {
                generation: current_generation,
                state,
                result,
            }));
        });

    match spawn_result {
        Ok(handle) => {
            *active_worker = Some(SearchWorker {
                cancelled,
                handle: Some(handle),
            });
        }
        Err(error) => {
            app.loading = false;
            app.search_phase = None;
            app.status_message = Some(format!("Could not start search: {error}"));
        }
    }
}

fn drain_search_results(
    rx: &Receiver<SearchMessage>,
    active_generation: u64,
    app: &mut App,
    search: &mut PaginatedSearch,
    search_cache: &mut SearchCache,
) -> Result<bool> {
    let mut changed = false;
    loop {
        match rx.try_recv() {
            Ok(SearchMessage::Progress {
                generation,
                request,
                state,
            }) => {
                if generation == active_generation {
                    apply_search_progress(request, state, app, search);
                    changed = true;
                }
            }
            Ok(SearchMessage::Finished(outcome)) => {
                if outcome.generation == active_generation {
                    let cacheable = outcome.result.is_ok() || !outcome.state.results.is_empty();
                    apply_search_outcome(outcome, app, search);
                    if cacheable {
                        search_cache.insert(search);
                    }
                    changed = true;
                }
            }
            Err(TryRecvError::Empty) => return Ok(changed),
            Err(TryRecvError::Disconnected) => return Ok(changed),
        }
    }
}

fn apply_cached_search(state: PaginatedSearch, app: &mut App, search: &mut PaginatedSearch) {
    *search = state;
    app.results.clone_from(&search.results);
    app.total_results = search.results.len();
    app.exhausted = search.exhausted;
    app.page = 0;
    app.selected_index = 0;
    app.loading = false;
    app.search_phase = None;
    app.status_message = None;
    app.schedule_page_prefetch();
}

fn apply_search_progress(
    request: SearchRequest,
    state: PaginatedSearch,
    app: &mut App,
    search: &mut PaginatedSearch,
) {
    *search = state;
    app.results.clone_from(&search.results);
    app.total_results = search.results.len();
    app.exhausted = search.exhausted;

    let requested_page = match app.search_phase {
        Some(SearchPhase::RequestedPage { target_page }) => Some(target_page),
        Some(SearchPhase::Initial) => match request {
            SearchRequest::New => Some(0),
            SearchRequest::NextPage { target_page } => Some(target_page),
            SearchRequest::BackgroundPrefetch => None,
        },
        // A prefetch is intentionally invisible to page selection. Keep its
        // declared target stable even once that page is complete: the worker
        // was only asked to fill this target, so advertising another page here
        // would briefly claim work that is not in flight.
        Some(SearchPhase::Prefetch { .. }) => None,
        None => None,
    };

    let Some(target_page) = requested_page else {
        return;
    };
    let page_size = app.page_size.max(1);
    let target_start = target_page.saturating_mul(page_size);
    let target_end = target_page.saturating_add(1).saturating_mul(page_size);
    let target_has_results = target_start < app.results.len();
    let target_is_complete = target_end <= app.results.len() || app.exhausted;

    if target_has_results && app.page != target_page {
        app.page = target_page;
        app.selected_index = 0;
    }

    app.search_phase = if target_is_complete && !app.exhausted {
        Some(SearchPhase::Prefetch {
            target_page: target_page.saturating_add(1),
        })
    } else if target_page == 0 {
        Some(SearchPhase::Initial)
    } else {
        Some(SearchPhase::RequestedPage { target_page })
    };
}

fn apply_search_outcome(outcome: SearchOutcome, app: &mut App, search: &mut PaginatedSearch) {
    let requested_page = match app.search_phase {
        Some(SearchPhase::RequestedPage { target_page }) => Some(target_page),
        _ => None,
    };
    app.loading = false;
    app.search_phase = None;
    *search = outcome.state;
    app.results.clone_from(&search.results);
    app.total_results = search.results.len();
    app.exhausted = search.exhausted;

    match outcome.result {
        Ok(()) => {
            app.status_message = None;

            if let Some(target_page) = requested_page {
                let target_start = target_page.saturating_mul(app.page_size.max(1));
                if target_start < app.results.len() {
                    app.page = target_page;
                }
            }
            app.selected_index = app
                .selected_index
                .min(app.current_page_results().len().saturating_sub(1));
            app.schedule_page_prefetch();
        }
        Err(error) => {
            app.status_message = Some(format!("Search failed: {error}"));
        }
    }
}

fn play_result(
    terminal: &mut Tui,
    app: &mut App,
    index: usize,
    temp_dir: &ManagedTempDir,
) -> Result<bool> {
    let Some(result) = app.results.get(index).cloned() else {
        return Ok(false);
    };

    let background_playback =
        crate::player::supports_background_playback(app.config.player) && !app.config.download_mode;
    if background_playback {
        app.queue.push_back(result);
        start_queue_if_idle(app);
        return Ok(true);
    }

    crate::ui::terminal::restore_terminal(terminal)?;
    let action = if app.config.download_mode {
        "Downloading:"
    } else {
        "Playing:"
    };
    println!("{} {}", action.green(), result.title);
    if !app.config.download_mode {
        crate::display::show_controls(app.config.player);
    }

    let playback_result = crate::player::play_video(
        &app.config,
        &result.id,
        &result.title,
        &result.safe_title(),
        temp_dir.path(),
    );
    *terminal = crate::ui::terminal::init_terminal()?;

    if let Err(error) = playback_result {
        app.status_message = Some(format!("Playback failed: {error}"));
    }
    Ok(true)
}

fn start_queue_if_idle(app: &mut App) {
    let player_is_idle = app
        .player_manager
        .as_ref()
        .is_none_or(|player| player.current_video_id.is_none());
    if !player_is_idle {
        return;
    }

    let Some(track) = app.queue.get(0) else {
        return;
    };
    let url = format!("https://www.youtube.com/watch?v={}", track.id);
    let title = track.title.clone();
    let video_id = track.id.clone();

    let result = if let Some(player) = app.player_manager.as_mut() {
        player.play(&app.config, &url, &title, &video_id)
    } else {
        match PlayerManager::new(&app.config) {
            Ok(mut player) => match player.play(&app.config, &url, &title, &video_id) {
                Ok(()) => {
                    app.player_manager = Some(player);
                    Ok(())
                }
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        }
    };

    match result {
        Ok(()) => app.status_message = None,
        Err(error) => {
            app.player_manager = None;
            app.status_message = Some(format!("Could not start playback: {error}"));
        }
    }
}

fn poll_player(app: &mut App, terminal_size: (u16, u16)) -> bool {
    let Some(player) = app.player_manager.as_mut() else {
        return false;
    };

    let update_error = player.update_status().err();
    let finished = player.is_eof();

    if let Some(error) = update_error {
        // Consume EOF before dropping the broken manager so the current queue
        // item advances exactly once. If core did not mark EOF, retain the item
        // so Enter can retry it.
        app.player_manager = None;
        app.status_message = Some(format!("Playback connection lost: {error}"));
    }

    if finished {
        app.handle_next_video(false);
    }

    if app.video_view {
        sync_video(app, terminal_size);
    }

    // Warm the stream-URL cache for the playing track so the first toggle
    // into the video view doesn't wait on yt-dlp. No-op once cached/pending.
    if !app.config.audio_only
        && let Some(video_id) = app
            .player_manager
            .as_ref()
            .and_then(|player| player.current_video_id.clone())
    {
        app.video.prefetch(&video_id);
    }

    true
}

/// Drive the terminal video pipeline from the latest mpv status. Runs on the
/// same tick as `poll_player` so it always sees fresh position/pause state.
/// Video failures never touch `app.player_manager` or `app.status_message`;
/// they are surfaced only inside the video pane (see `video::VideoState`).
fn sync_video(app: &mut App, (width, height): (u16, u16)) {
    let (video_id, playing, paused, time_pos) = match app.player_manager.as_ref() {
        Some(player) => (
            player.current_video_id.clone(),
            player.status.playing,
            player.status.paused,
            player.status.time_pos,
        ),
        None => (None, false, false, 0.0),
    };
    let has_player = app.player_manager.is_some();
    let (cols, rows) = crate::ui::layout::video_pane_size(has_player, width, height);
    app.video.sync(
        playing,
        paused,
        video_id.as_deref(),
        time_pos,
        cols,
        rows,
        app.config.video_render,
    );
}

fn sync_runtime_settings(
    app: &mut App,
    search: &mut PaginatedSearch,
    temp_dir: &mut ManagedTempDir,
) -> bool {
    let page_size = clamp_results_per_page(app.config.results_per_page);
    app.config.results_per_page = page_size;
    temp_dir.set_keep(app.config.keep_temp);

    let page_size_changed = app.page_size != page_size || search.page_size != page_size;
    let shorts_filter = !app.config.include_shorts;
    let filter_changed = search.filter_shorts != shorts_filter;
    if !page_size_changed && !filter_changed {
        return false;
    }

    app.page_size = page_size;
    app.page = 0;
    app.selected_index = 0;

    if !app.query.trim().is_empty() {
        // Keep the existing state's settings intact until NewSearch caches it.
        // Relabelling old results here would allow them to hit a cache lookup
        // for a new page size or Shorts policy.
        app.loading = true;
        app.search_phase = Some(SearchPhase::Initial);
        app.pending_action = AppAction::NewSearch(app.query.clone());
    } else {
        search.page_size = page_size;
        search.filter_shorts = shorts_filter;
    }
    true
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;
    use crate::config::Config;
    use crate::search::SearchResult;

    fn result(id: &str) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: format!("Track {id}"),
            duration: "1:00".to_string(),
            channel: "Channel".to_string(),
            views: "1".to_string(),
            published: String::new(),
        }
    }

    #[test]
    fn successful_search_completion_clears_loading_and_updates_results() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.loading = true;
        app.status_message = Some("old error".to_string());
        let mut search = PaginatedSearch::new("query", 10, false);
        let mut completed = search.clone();
        completed.results.push(result("1"));

        apply_search_outcome(
            SearchOutcome {
                generation: 1,
                state: completed,
                result: Ok(()),
            },
            &mut app,
            &mut search,
        );

        assert!(!app.loading);
        assert_eq!(app.results.len(), 1);
        assert_eq!(app.total_results, 1);
        assert!(app.status_message.is_none());
    }

    #[test]
    fn failed_next_page_keeps_the_visible_page_and_partial_state() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.results = (0..10).map(|index| result(&index.to_string())).collect();
        app.total_results = 10;
        app.page = 0;
        app.loading = true;
        app.search_phase = Some(SearchPhase::RequestedPage { target_page: 1 });
        let mut search = PaginatedSearch::new("query", 10, false);
        search.results.clone_from(&app.results);

        apply_search_outcome(
            SearchOutcome {
                generation: 1,
                state: search.clone(),
                result: Err(anyhow!("network unavailable")),
            },
            &mut app,
            &mut search,
        );

        assert!(!app.loading);
        assert_eq!(app.page, 0);
        assert_eq!(app.results.len(), 10);
        assert_eq!(app.pending_action, AppAction::None);
        assert!(app.status_message.as_deref().unwrap().contains("network"));
    }

    #[test]
    fn progress_publishes_partial_initial_results_without_ending_loading() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.loading = true;
        app.search_phase = Some(SearchPhase::Initial);
        let mut search = PaginatedSearch::new("query", 10, false);
        let mut progress = search.clone();
        progress.results = (0..5).map(|index| result(&index.to_string())).collect();

        apply_search_progress(SearchRequest::New, progress, &mut app, &mut search);

        assert!(app.loading);
        assert_eq!(app.results.len(), 5);
        assert_eq!(app.page, 0);
        assert_eq!(app.search_phase, Some(SearchPhase::Initial));
    }

    #[test]
    fn requested_page_stays_visible_until_partial_results_arrive() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.results = (0..10).map(|index| result(&index.to_string())).collect();
        app.total_results = 10;
        app.loading = true;
        app.search_phase = Some(SearchPhase::RequestedPage { target_page: 1 });
        let mut search = PaginatedSearch::new("query", 10, false);
        search.results.clone_from(&app.results);

        assert_eq!(app.page, 0);
        assert_eq!(app.current_page_results().len(), 10);

        let mut progress = search.clone();
        progress
            .results
            .extend((10..15).map(|index| result(&index.to_string())));
        apply_search_progress(
            SearchRequest::NextPage { target_page: 1 },
            progress,
            &mut app,
            &mut search,
        );

        assert_eq!(app.page, 1);
        assert_eq!(app.current_page_results().len(), 5);
        assert_eq!(
            app.search_phase,
            Some(SearchPhase::RequestedPage { target_page: 1 })
        );
    }

    #[test]
    fn completed_requested_page_transitions_to_prefetch_progress() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.loading = true;
        app.search_phase = Some(SearchPhase::RequestedPage { target_page: 1 });
        let mut search = PaginatedSearch::new("query", 10, false);
        let mut progress = search.clone();
        progress.results = (0..20).map(|index| result(&index.to_string())).collect();

        apply_search_progress(
            SearchRequest::NextPage { target_page: 1 },
            progress,
            &mut app,
            &mut search,
        );

        assert_eq!(app.page, 1);
        assert_eq!(
            app.search_phase,
            Some(SearchPhase::Prefetch { target_page: 2 })
        );
    }

    #[test]
    fn prefetch_progress_does_not_override_manual_page_navigation() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.page = 0;
        app.loading = true;
        app.search_phase = Some(SearchPhase::Prefetch { target_page: 1 });
        let mut search = PaginatedSearch::new("query", 10, false);
        let mut progress = search.clone();
        progress.results = (0..20).map(|index| result(&index.to_string())).collect();

        apply_search_progress(
            SearchRequest::BackgroundPrefetch,
            progress,
            &mut app,
            &mut search,
        );

        assert_eq!(app.page, 0);
        assert_eq!(
            app.search_phase,
            Some(SearchPhase::Prefetch { target_page: 1 })
        );
    }

    #[test]
    fn successful_outcome_schedules_prefetch_for_the_viewed_pages_missing_successor() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.page = 1;
        app.loading = true;
        app.search_phase = Some(SearchPhase::RequestedPage { target_page: 1 });
        let mut search = PaginatedSearch::new("query", 10, false);
        let mut completed = search.clone();
        completed.results = (0..20).map(|index| result(&index.to_string())).collect();

        apply_search_outcome(
            SearchOutcome {
                generation: 1,
                state: completed,
                result: Ok(()),
            },
            &mut app,
            &mut search,
        );

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::PrefetchNextPage(1));
    }

    #[test]
    fn successful_outcome_finishes_a_partial_viewed_page_before_prefetching() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.page = 1;
        app.loading = true;
        app.search_phase = Some(SearchPhase::RequestedPage { target_page: 1 });
        let mut search = PaginatedSearch::new("query", 10, false);
        let mut completed = search.clone();
        completed.results = (0..15).map(|index| result(&index.to_string())).collect();

        apply_search_outcome(
            SearchOutcome {
                generation: 1,
                state: completed,
                result: Ok(()),
            },
            &mut app,
            &mut search,
        );

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::FetchNextPage(1));
    }

    #[test]
    fn completed_background_prefetch_keeps_the_viewed_page_and_does_not_run_ahead() {
        let mut app = App::new("query".to_string(), 10, Config::default());
        app.results = (0..20).map(|index| result(&index.to_string())).collect();
        app.total_results = 20;
        app.page = 1;
        app.loading = true;
        app.search_phase = Some(SearchPhase::Prefetch { target_page: 2 });
        let mut search = PaginatedSearch::new("query", 10, false);
        let mut completed = search.clone();
        completed.results = (0..30).map(|index| result(&index.to_string())).collect();

        apply_search_outcome(
            SearchOutcome {
                generation: 1,
                state: completed,
                result: Ok(()),
            },
            &mut app,
            &mut search,
        );

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::None);
        assert!(!app.loading);
        assert!(app.search_phase.is_none());
    }

    #[test]
    fn active_search_caps_event_polling_at_fifty_milliseconds() {
        assert_eq!(
            event_poll_timeout(Duration::from_millis(200), true),
            SEARCH_POLL_RATE
        );
        assert_eq!(
            event_poll_timeout(Duration::from_millis(20), true),
            Duration::from_millis(20)
        );
        assert_eq!(
            event_poll_timeout(Duration::from_millis(200), false),
            Duration::from_millis(200)
        );
    }

    #[test]
    fn search_cache_trims_queries_preserves_case_and_separates_filter_settings() {
        let mut cache = SearchCache::default();
        let mut state = PaginatedSearch::new("  Jubal SHOW ", 2, true);
        state.results = vec![result("1"), result("2")];
        cache.insert(&state);

        let cached = cache.get("Jubal SHOW", 2, true).unwrap();
        assert_eq!(cached.results.len(), 2);
        assert_eq!(cached.query(), "  Jubal SHOW ");
        assert!(cache.get("jubal show", 2, true).is_none());
        assert!(cache.get("Jubal SHOW", 2, false).is_none());
        assert!(cache.get("Jubal SHOW", 3, true).is_none());
    }

    #[test]
    fn search_cache_is_bounded_expires_entries_and_ignores_unusable_partials() {
        let mut cache = SearchCache::default();
        let mut partial = PaginatedSearch::new("partial", 2, false);
        partial.results.push(result("partial"));
        cache.insert(&partial);
        assert!(cache.entries.is_empty());

        for index in 0..=SEARCH_CACHE_CAPACITY {
            let query = format!("query {index}");
            let mut state = PaginatedSearch::new(&query, 1, false);
            state.results.push(result(&index.to_string()));
            cache.insert(&state);
        }

        assert_eq!(cache.entries.len(), SEARCH_CACHE_CAPACITY);
        assert!(cache.get("query 0", 1, false).is_none());
        let cached = cache.get("query 8", 1, false).unwrap();

        let original_fetch = Instant::now() - Duration::from_secs(60);
        cache.entries.front_mut().unwrap().fetched_at = original_fetch;
        cache.insert(&cached);
        assert_eq!(cache.entries.front().unwrap().fetched_at, original_fetch);

        cache.entries.front_mut().unwrap().fetched_at =
            Instant::now() - SEARCH_CACHE_TTL - Duration::from_secs(1);
        assert!(cache.get("query 8", 1, false).is_none());
    }

    #[test]
    fn applying_cached_search_restores_results_without_a_loading_state() {
        let mut app = App::new("cached".to_string(), 2, Config::default());
        app.loading = true;
        app.search_phase = Some(SearchPhase::Initial);
        let mut search = PaginatedSearch::new("old", 2, false);
        let mut cached = PaginatedSearch::new("cached", 2, false);
        cached.results = vec![result("1"), result("2")];

        apply_cached_search(cached, &mut app, &mut search);

        assert_eq!(search.query(), "cached");
        assert_eq!(app.results.len(), 2);
        assert!(!app.loading);
        assert!(app.search_phase.is_none());
        assert_eq!(app.pending_action, AppAction::PrefetchNextPage(0));
    }

    #[test]
    fn settings_sync_does_not_relabel_old_results_before_they_are_cached() {
        let config = Config {
            include_shorts: true,
            results_per_page: 20,
            ..Config::default()
        };
        let mut app = App::new("query".to_string(), 10, config);
        let mut search = PaginatedSearch::new("query", 10, true);
        search.results = (0..10).map(|index| result(&index.to_string())).collect();
        let mut temp_dir = ManagedTempDir::new(false).unwrap();

        assert!(sync_runtime_settings(&mut app, &mut search, &mut temp_dir));

        assert_eq!(app.page_size, 20);
        assert!(app.config.include_shorts);
        assert_eq!(search.page_size, 10);
        assert!(search.filter_shorts);
        assert_eq!(search.results.len(), 10);
        assert_eq!(
            app.pending_action,
            AppAction::NewSearch("query".to_string())
        );
    }
}
