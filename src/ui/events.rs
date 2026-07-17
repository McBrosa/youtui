use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::clamp_results_per_page;
use crate::player_manager::PlayerManager;
use crate::ui::app::{App, AppAction, FocusedPanel, InputMode, SearchPhase, SettingsField};

pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // Some terminals report key releases in addition to presses. Handling both
    // makes text input and shortcuts fire twice.
    if key.kind == KeyEventKind::Release {
        return;
    }

    // Ctrl-C is the emergency exit even while a modal or text editor owns the
    // rest of the keyboard.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }

    // Status messages behave like a toast: they remain visible until the next
    // interaction, while any new failure from that interaction can replace it.
    app.status_message = None;

    // Modal input owns the keyboard. In particular, Tab must not move focus in
    // the obscured UI while Settings is open.
    if app.settings_open {
        handle_settings_keys(app, key);
        return;
    }

    // Global Tab key for focus cycling (works in any mode except Help).
    if app.input_mode != InputMode::Help {
        match key.code {
            KeyCode::BackTab => {
                cycle_focus_backward(app);
                return;
            }
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    cycle_focus_backward(app);
                } else {
                    cycle_focus_forward(app);
                }
                return;
            }
            _ => {}
        }
    }

    match app.input_mode {
        InputMode::Browse => handle_browse_keys(app, key),
        InputMode::Help => handle_help_keys(app, key),
    }
}

fn cycle_focus_forward(app: &mut App) {
    app.focused_panel = match app.focused_panel {
        FocusedPanel::SearchBar => FocusedPanel::Results,
        FocusedPanel::Results => FocusedPanel::Queue,
        FocusedPanel::Queue => FocusedPanel::SearchBar,
    };
}

fn cycle_focus_backward(app: &mut App) {
    app.focused_panel = match app.focused_panel {
        FocusedPanel::SearchBar => FocusedPanel::Queue,
        FocusedPanel::Queue => FocusedPanel::Results,
        FocusedPanel::Results => FocusedPanel::SearchBar,
    };
}

fn handle_browse_keys(app: &mut App, key: KeyEvent) {
    // If settings are open, handle settings navigation (including editing)
    if app.settings_open {
        handle_settings_keys(app, key);
        return;
    }

    // Settings has one consistent global shortcut. Lowercase `s` and `/`
    // remain dedicated to search focus.
    match key.code {
        KeyCode::Char('S') if app.focused_panel != FocusedPanel::SearchBar => {
            app.settings_open = true;
            return;
        }
        KeyCode::F(2) => {
            app.settings_open = true;
            return;
        }
        _ => {}
    }

    // Global quit keys work from any panel
    match (key.code, key.modifiers) {
        (KeyCode::Char('q' | 'Q'), _) if app.focused_panel != FocusedPanel::SearchBar => {
            app.should_quit = true;
            return;
        }
        (KeyCode::Esc, _) => {
            // Esc in the search editor cancels editing. While results are
            // loading it cancels that search without terminating the app.
            if app.focused_panel == FocusedPanel::SearchBar {
                app.search_input.clear();
                app.focused_panel = FocusedPanel::Results;
                return;
            }
            if app.loading {
                app.pending_action = AppAction::CancelSearch;
                return;
            }
            app.should_quit = true;
            return;
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.should_quit = true;
            return;
        }
        _ => {}
    }

    // Global playback controls (work from any panel, don't conflict with panel keys)
    if app.focused_panel != FocusedPanel::SearchBar {
        match key.code {
            KeyCode::Char(' ') => {
                if run_player_command(app, |player| player.toggle_pause()) {
                    return;
                }
            }
            KeyCode::Char('<') => {
                if run_player_command(app, |player| player.seek(-10.0)) {
                    return;
                }
            }
            KeyCode::Char('>') => {
                if run_player_command(app, |player| player.seek(10.0)) {
                    return;
                }
            }
            KeyCode::Char('=') | KeyCode::Char('+') => {
                if run_player_command(app, |player| {
                    player.set_volume((player.status.volume + 5).min(100))
                }) {
                    return;
                }
            }
            KeyCode::Char('-') if app.focused_panel != FocusedPanel::SearchBar => {
                if run_player_command(app, |player| {
                    player.set_volume((player.status.volume - 5).max(0))
                }) {
                    return;
                }
            }
            KeyCode::Char('m') if app.focused_panel != FocusedPanel::SearchBar => {
                if run_player_command(app, |player| {
                    let new_volume = if player.status.volume > 0 { 0 } else { 100 };
                    player.set_volume(new_volume)
                }) {
                    return;
                }
            }
            _ => {}
        }
    }

    match app.focused_panel {
        FocusedPanel::SearchBar => handle_search_bar_keys(app, key),
        FocusedPanel::Results => handle_results_keys(app, key),
        FocusedPanel::Queue => handle_queue_keys(app, key),
    }
}

fn run_player_command(
    app: &mut App,
    command: impl FnOnce(&mut PlayerManager) -> anyhow::Result<()>,
) -> bool {
    let Some(player) = app.player_manager.as_mut() else {
        return false;
    };

    if let Err(error) = command(player) {
        app.player_manager = None;
        app.status_message = Some(format!("Playback stopped: {error}"));
    }
    true
}

fn handle_results_keys(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Up | KeyCode::Char('k'), _) => {
            if app.selected_index > 0 {
                app.selected_index -= 1;
            }
        }
        (KeyCode::Down | KeyCode::Char('j'), _) => {
            let page_results = app.current_page_results();
            if app.selected_index < page_results.len().saturating_sub(1) {
                app.selected_index += 1;
            }
        }
        (KeyCode::Home | KeyCode::Char('g'), _) => {
            app.selected_index = 0;
        }
        (KeyCode::End | KeyCode::Char('G'), _) => {
            app.selected_index = app.current_page_results().len().saturating_sub(1);
        }
        (KeyCode::Char('n') | KeyCode::PageDown, _) if app.has_next_page() => {
            let target_page = app.page.saturating_add(1);
            let target_start = target_page.saturating_mul(app.page_size.max(1));
            if target_start < app.results.len() {
                app.page = target_page;
                app.selected_index = 0;
                app.schedule_page_prefetch();
            } else if !app.loading && !app.exhausted {
                app.loading = true;
                app.search_phase = Some(SearchPhase::RequestedPage { target_page });
                app.pending_action = AppAction::FetchNextPage(target_page);
            } else if matches!(
                app.search_phase,
                Some(SearchPhase::Prefetch {
                    target_page: prefetched_page
                }) if prefetched_page == target_page
            ) {
                // The requested page is already being prefetched. Remember the
                // user's intent and switch as soon as its snapshot arrives.
                app.search_phase = Some(SearchPhase::RequestedPage { target_page });
            }
        }
        (KeyCode::Char('p') | KeyCode::PageUp, _) if app.has_prev_page() => {
            app.page -= 1;
            app.selected_index = 0;
            if let Some(SearchPhase::RequestedPage { target_page }) = app.search_phase {
                // The user explicitly moved away from a partially loaded page.
                // Keep fetching it, but do not force focus back on later updates.
                app.search_phase = Some(SearchPhase::Prefetch { target_page });
            }
            app.schedule_page_prefetch();
        }
        (KeyCode::Char('h' | '?'), _) => {
            app.input_mode = InputMode::Help;
        }
        (KeyCode::Char('s' | '/'), _) => {
            app.focused_panel = FocusedPanel::SearchBar;
        }
        (KeyCode::Char(c), _) if c.is_ascii_digit() => {
            if app.number_input.len() < 6 {
                app.number_input.push(c);
            }
        }
        (KeyCode::Enter, _) => {
            let idx = if !app.number_input.is_empty() {
                let page_len = app.current_page_results().len();
                let page_start = app.page.saturating_mul(app.page_size.max(1));
                let result = app.number_input.parse::<usize>().ok().and_then(|num| {
                    let global_start = page_start.saturating_add(1);
                    let global_end = page_start.saturating_add(page_len);
                    if (global_start..=global_end).contains(&num) {
                        // Match the number displayed beside the result.
                        Some(num - 1)
                    } else if num > 0 && num <= page_len {
                        // Keep page-local quick picks for muscle memory.
                        Some(page_start.saturating_add(num - 1))
                    } else {
                        None
                    }
                });
                app.number_input.clear();
                result
            } else {
                Some(
                    app.page
                        .saturating_mul(app.page_size.max(1))
                        .saturating_add(app.selected_index),
                )
            };

            if let Some(idx) = idx
                && idx < app.results.len()
            {
                app.pending_action = AppAction::Play(idx);
            }
        }
        (KeyCode::Backspace, _) => {
            app.number_input.pop();
        }
        _ => {}
    }
}

fn handle_search_bar_keys(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Char(c), modifiers)
            if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if app.search_input.len() < 4096 {
                app.search_input.push(c);
            }
        }
        (KeyCode::Backspace, _) => {
            app.search_input.pop();
        }
        (KeyCode::Enter, _) => {
            let query = app.search_input.trim().to_string();
            if !query.is_empty() {
                // Update query immediately so search bar shows new query
                app.query.clone_from(&query);
                app.loading = true;
                app.search_phase = Some(SearchPhase::Initial);
                app.pending_action = AppAction::NewSearch(query);
                app.search_input.clear();
                app.number_input.clear();
                app.focused_panel = FocusedPanel::Results;
            }
        }
        (KeyCode::Esc, _) => {
            app.search_input.clear();
            app.focused_panel = FocusedPanel::Results;
        }
        _ => {}
    }
}

fn handle_queue_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.queue_selected_index > 0 {
                app.queue_selected_index -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.queue_selected_index < app.queue.len().saturating_sub(1) {
                app.queue_selected_index += 1;
            }
        }
        KeyCode::Home | KeyCode::Char('g') => {
            app.queue_selected_index = 0;
        }
        KeyCode::End | KeyCode::Char('G') => {
            app.queue_selected_index = app.queue.len().saturating_sub(1);
        }
        KeyCode::Enter => {
            if promote_selected_queue_item(app) {
                play_queue_front(app);
            }
        }
        KeyCode::Delete | KeyCode::Backspace => {
            if app.queue_selected_index < app.queue.len() {
                let was_playing = removed_queue_item_was_playing(
                    app.queue_selected_index,
                    app.player_manager
                        .as_ref()
                        .and_then(|pm| pm.current_video_id.as_deref()),
                );
                app.queue.remove(app.queue_selected_index);

                if app.queue_selected_index >= app.queue.len() && app.queue_selected_index > 0 {
                    app.queue_selected_index -= 1;
                }

                if was_playing {
                    // The currently-playing track was removed; play whatever is now at the
                    // front WITHOUT popping again (calling handle_next_video would double-pop).
                    if app.queue.is_empty() {
                        if let Some(mut pm) = app.player_manager.take()
                            && let Err(error) = pm.clear()
                        {
                            app.status_message = Some(format!("Could not stop playback: {error}"));
                        }
                    } else {
                        play_queue_front(app);
                    }
                }
            }
        }
        KeyCode::Char('c') => {
            app.queue.clear();
            app.queue_selected_index = 0;
            // Clear player when queue is cleared
            if let Some(mut player) = app.player_manager.take()
                && let Err(error) = player.clear()
            {
                app.status_message = Some(format!("Could not stop playback: {error}"));
            }
        }
        KeyCode::Char('n') => {
            // Next track - manual action, always auto-plays
            app.handle_next_video(true);
        }
        KeyCode::Char('s' | '/') => {
            app.focused_panel = FocusedPanel::SearchBar;
        }
        KeyCode::Char('h' | '?') => {
            app.input_mode = InputMode::Help;
        }
        _ => {}
    }
}

fn promote_selected_queue_item(app: &mut App) -> bool {
    if app.queue.is_empty() || app.queue_selected_index >= app.queue.len() {
        return false;
    }
    if app.queue_selected_index > 0 {
        app.queue.move_to_front(app.queue_selected_index);
        app.queue_selected_index = 0;
    }
    true
}

fn play_queue_front(app: &mut App) {
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

fn removed_queue_item_was_playing(selected_index: usize, current_video_id: Option<&str>) -> bool {
    // Queue position, not video ID, identifies the active entry. Duplicate
    // videos are valid queue entries and a later duplicate is not playing.
    selected_index == 0 && current_video_id.is_some()
}

fn handle_help_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('h' | '?' | 'q') => {
            app.input_mode = InputMode::Browse;
        }
        _ => {}
    }
}

fn handle_settings_keys(app: &mut App, key: KeyEvent) {
    // If editing a text field, handle text input
    if let Some(field) = app.settings_editing {
        match (key.code, key.modifiers) {
            (KeyCode::Char(c), modifiers)
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let accepts_character =
                    field != SettingsField::ResultsPerPage || c.is_ascii_digit();
                if accepts_character
                    && let Some(input) = app.settings_text_input.as_mut()
                    && input.len() < 4096
                {
                    input.push(c);
                }
            }
            (KeyCode::Backspace, _) => {
                if let Some(input) = app.settings_text_input.as_mut() {
                    input.pop();
                }
            }
            (KeyCode::Enter | KeyCode::Esc, _) => {
                finish_settings_edit(app, field);
            }
            _ => {}
        }
        return;
    }

    // Define selectable indices (skip section headers)
    const SELECTABLE_INDICES: &[usize] = &[2, 3, 4, 5, 6, 10, 11, 15, 19];

    match key.code {
        KeyCode::Esc => {
            app.settings_open = false;
            app.settings_editing = None;
            app.settings_text_input = None;
        }
        KeyCode::Up | KeyCode::BackTab => {
            // Find the previous selectable index
            let current = app.settings_selected_index;
            let pos = SELECTABLE_INDICES.iter().position(|&x| x == current);

            if let Some(pos) = pos
                && pos > 0
            {
                app.settings_selected_index = SELECTABLE_INDICES[pos - 1];
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            // Find the next selectable index
            let current = app.settings_selected_index;
            let pos = SELECTABLE_INDICES.iter().position(|&x| x == current);

            if let Some(pos) = pos
                && pos < SELECTABLE_INDICES.len() - 1
            {
                app.settings_selected_index = SELECTABLE_INDICES[pos + 1];
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            // Handle action based on the selected index
            match app.settings_selected_index {
                2 => {
                    // Audio Only checkbox
                    let result = app.config.toggle_audio_only();
                    record_settings_save_result(app, result);
                }
                3 => {
                    // Bandwidth Limit checkbox
                    let result = app.config.toggle_bandwidth_limit();
                    record_settings_save_result(app, result);
                }
                4 => {
                    // Keep Temp checkbox
                    let result = app.config.toggle_keep_temp();
                    record_settings_save_result(app, result);
                }
                5 => {
                    // Include Shorts checkbox
                    let result = app.config.toggle_include_shorts();
                    record_settings_save_result(app, result);
                }
                6 => {
                    // Auto Play Queue checkbox
                    let result = app.config.toggle_auto_play_queue();
                    record_settings_save_result(app, result);
                }
                10 => {
                    // Download Mode checkbox
                    let result = app.config.toggle_download_mode();
                    record_settings_save_result(app, result);
                }
                11 => {
                    // Download Dir text field - enter edit mode
                    app.settings_editing = Some(SettingsField::DownloadDir);
                    app.settings_text_input = Some(app.config.download_dir.clone());
                }
                15 => {
                    // Results Per Page text field - enter edit mode
                    app.settings_editing = Some(SettingsField::ResultsPerPage);
                    app.settings_text_input = Some(app.config.results_per_page.to_string());
                }
                19 => {
                    // Custom Format text field - enter edit mode
                    app.settings_editing = Some(SettingsField::CustomFormat);
                    app.settings_text_input = Some(app.config.custom_format.clone());
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn record_settings_save_result(app: &mut App, result: anyhow::Result<()>) {
    if let Err(error) = result {
        app.status_message = Some(format!("Could not save settings: {error}"));
    }
}

fn finish_settings_edit(app: &mut App, field: SettingsField) {
    let input = app.settings_text_input.take().unwrap_or_default();
    match field {
        SettingsField::DownloadDir => {
            if input.trim().is_empty() {
                app.status_message = Some("Download directory cannot be empty".to_string());
                app.settings_editing = None;
                return;
            }
            app.config.download_dir = input;
        }
        SettingsField::ResultsPerPage => {
            let value = input
                .parse::<usize>()
                .unwrap_or(app.config.results_per_page);
            app.config.results_per_page = clamp_results_per_page(value);
        }
        SettingsField::CustomFormat => app.config.custom_format = input,
    }

    if let Err(error) = app.config.save() {
        app.status_message = Some(format!("Could not save settings: {error}"));
    }
    app.settings_editing = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::search::SearchResult;

    fn create_test_results(count: usize) -> Vec<SearchResult> {
        (0..count)
            .map(|i| SearchResult {
                title: format!("Video {}", i + 1),
                duration: "5:00".to_string(),
                channel: "Test Channel".to_string(),
                views: "1K".to_string(),
                id: format!("id{}", i + 1),
            })
            .collect()
    }

    #[test]
    fn test_arrow_up_navigation() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(5);
        app.total_results = 5;
        app.selected_index = 2;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Up);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.selected_index, 1);
    }

    #[test]
    fn test_arrow_up_at_top() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(5);
        app.total_results = 5;
        app.selected_index = 0;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Up);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn test_arrow_down_navigation() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(5);
        app.total_results = 5;
        app.selected_index = 2;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Down);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.selected_index, 3);
    }

    #[test]
    fn test_arrow_down_at_bottom() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(5);
        app.total_results = 5;
        app.selected_index = 4;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Down);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.selected_index, 4);
    }

    #[test]
    fn test_next_page_navigation() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(25);
        app.total_results = 25;
        app.page = 0;
        app.selected_index = 5;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('n'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.page, 1);
        assert_eq!(app.selected_index, 0);
        assert_eq!(app.pending_action, AppAction::PrefetchNextPage(1));
    }

    #[test]
    fn test_cached_partial_next_page_is_completed_before_prefetching() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(15);
        app.total_results = 15;
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Char('n')));

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::FetchNextPage(1));
    }

    #[test]
    fn test_cached_navigation_does_not_refetch_fully_cached_following_page() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(30);
        app.total_results = 30;
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Char('n')));

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::None);
    }

    #[test]
    fn test_previous_page_schedules_missing_following_page_prefetch() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(25);
        app.total_results = 25;
        app.page = 2;
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Char('p')));

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::PrefetchNextPage(1));
    }

    #[test]
    fn test_cached_navigation_does_not_schedule_while_search_is_loading() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(20);
        app.total_results = 20;
        app.loading = true;
        app.search_phase = Some(SearchPhase::Prefetch { target_page: 1 });
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Char('n')));

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::None);
    }

    #[test]
    fn test_cached_navigation_does_not_schedule_after_search_exhaustion() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(20);
        app.total_results = 20;
        app.exhausted = true;
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Char('n')));

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::None);
    }

    #[test]
    fn test_cached_navigation_preserves_an_existing_pending_action() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(20);
        app.total_results = 20;
        app.pending_action = AppAction::Play(3);
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Char('n')));

        assert_eq!(app.page, 1);
        assert_eq!(app.pending_action, AppAction::Play(3));
    }

    #[test]
    fn test_uncached_next_page_keeps_current_page_visible_while_loading() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(10);
        app.total_results = 10;
        app.page = 0;
        app.selected_index = 5;
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Char('n')));

        assert_eq!(app.page, 0);
        assert_eq!(app.selected_index, 5);
        assert!(app.loading);
        assert_eq!(
            app.search_phase,
            Some(SearchPhase::RequestedPage { target_page: 1 })
        );
        assert_eq!(app.pending_action, AppAction::FetchNextPage(1));
    }

    #[test]
    fn test_results_remain_navigable_while_search_prefetches() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(5);
        app.total_results = 5;
        app.loading = true;
        app.search_phase = Some(SearchPhase::Prefetch { target_page: 1 });
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Down));

        assert_eq!(app.selected_index, 1);
    }

    #[test]
    fn test_esc_cancels_loading_search_without_quitting() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.loading = true;
        app.search_phase = Some(SearchPhase::Initial);
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Esc));

        assert_eq!(app.pending_action, AppAction::CancelSearch);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_next_page_when_not_available() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(10);
        app.total_results = 10;
        app.exhausted = true;
        app.page = 0;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('n'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.page, 0);
    }

    #[test]
    fn test_prev_page_navigation() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(25);
        app.total_results = 25;
        app.page = 2;
        app.selected_index = 5;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('p'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.page, 1);
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn test_previous_page_during_loading_prevents_forced_forward_navigation() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(15);
        app.total_results = 15;
        app.page = 1;
        app.loading = true;
        app.search_phase = Some(SearchPhase::RequestedPage { target_page: 1 });
        app.focused_panel = FocusedPanel::Results;

        handle_browse_keys(&mut app, KeyEvent::from(KeyCode::Char('p')));

        assert_eq!(app.page, 0);
        assert_eq!(
            app.search_phase,
            Some(SearchPhase::Prefetch { target_page: 1 })
        );
    }

    #[test]
    fn test_prev_page_at_first_page() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.results = create_test_results(25);
        app.total_results = 25;
        app.page = 0;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('p'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.page, 0);
    }

    #[test]
    fn test_help_key_switches_mode() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.input_mode = InputMode::Browse;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('h'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.input_mode, InputMode::Help);
    }

    #[test]
    fn test_quit_key() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.should_quit = false;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('q'));
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }

    #[test]
    fn test_esc_key_quits_from_results() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.should_quit = false;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }

    #[test]
    fn test_ctrl_c_quits() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.should_quit = false;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }

    #[test]
    fn test_tab_cycles_focus_forward() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;

        cycle_focus_forward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::Results);

        cycle_focus_forward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::Queue);

        cycle_focus_forward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::SearchBar);
    }

    #[test]
    fn test_shift_tab_cycles_focus_backward() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;

        cycle_focus_backward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::Queue);

        cycle_focus_backward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::Results);

        cycle_focus_backward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::SearchBar);
    }

    #[test]
    fn test_esc_in_search_bar_returns_to_results() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;
        app.search_input = "some text".to_string();
        app.should_quit = false;

        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);

        assert!(!app.should_quit);
        assert_eq!(app.focused_panel, FocusedPanel::Results);
        assert!(app.search_input.is_empty());
    }

    #[test]
    fn test_search_bar_enter_triggers_new_search() {
        let mut app = App::new("old query".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;
        app.search_input = "new query".to_string();

        let key = KeyEvent::from(KeyCode::Enter);
        handle_browse_keys(&mut app, key);

        assert_eq!(
            app.pending_action,
            AppAction::NewSearch("new query".to_string())
        );
        assert_eq!(app.focused_panel, FocusedPanel::Results);
        assert!(app.search_input.is_empty());
    }

    #[test]
    fn test_settings_open_with_uppercase_s() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = false;
        app.focused_panel = FocusedPanel::Queue;

        let key = KeyEvent::from(KeyCode::Char('S'));
        handle_browse_keys(&mut app, key);

        assert!(app.settings_open);
    }

    #[test]
    fn test_s_key_from_results_focuses_search_bar() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = false;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('s'));
        handle_browse_keys(&mut app, key);

        assert!(!app.settings_open);
        assert_eq!(app.focused_panel, FocusedPanel::SearchBar);
    }

    #[test]
    fn test_settings_open_with_f2() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = false;

        let key = KeyEvent::from(KeyCode::F(2));
        handle_browse_keys(&mut app, key);

        assert!(app.settings_open);
    }

    #[test]
    fn test_settings_s_key_blocked_in_search_bar() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;
        app.settings_open = false;

        let key = KeyEvent::from(KeyCode::Char('s'));
        handle_browse_keys(&mut app, key);

        assert!(!app.settings_open);
        assert_eq!(app.search_input, "s");
    }

    #[test]
    fn test_settings_navigation_down() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = true;
        app.settings_selected_index = 2;

        let key = KeyEvent::from(KeyCode::Down);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.settings_selected_index, 3);
    }

    #[test]
    fn test_settings_navigation_up() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = true;
        app.settings_selected_index = 3;

        let key = KeyEvent::from(KeyCode::Up);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.settings_selected_index, 2);
    }

    #[test]
    fn test_settings_toggle_checkbox() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = true;
        app.settings_selected_index = 2; // Audio Only
        app.config.audio_only = false;

        let key = KeyEvent::from(KeyCode::Enter);
        handle_browse_keys(&mut app, key);

        assert!(app.config.audio_only);
    }

    #[test]
    fn test_settings_enter_edit_mode() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = true;
        app.settings_selected_index = 11; // Download Dir (index 10 is Download Mode checkbox)

        let key = KeyEvent::from(KeyCode::Enter);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.settings_editing, Some(SettingsField::DownloadDir));
    }

    #[test]
    fn test_settings_esc_closes_modal() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = true;

        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);

        assert!(!app.settings_open);
    }

    #[test]
    fn test_settings_end_to_end_workflow() {
        // Create app with default settings
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::Queue;

        // Open settings with the global uppercase-S shortcut.
        let key = KeyEvent::from(KeyCode::Char('S'));
        handle_browse_keys(&mut app, key);
        assert!(app.settings_open);

        // Navigate down to bandwidth limit (index 3)
        let key = KeyEvent::from(KeyCode::Down);
        handle_browse_keys(&mut app, key);
        assert_eq!(app.settings_selected_index, 3);

        // Toggle bandwidth limit
        let key = KeyEvent::from(KeyCode::Enter);
        handle_browse_keys(&mut app, key);
        assert!(app.config.bandwidth_limit);

        // Navigate to download dir (index 11)
        // From index 3: 3→4→5→6→10→11 (5 down presses; index 10 is Download Mode checkbox)
        for _ in 0..5 {
            let key = KeyEvent::from(KeyCode::Down);
            handle_browse_keys(&mut app, key);
        }
        assert_eq!(app.settings_selected_index, 11);

        // Enter edit mode
        let key = KeyEvent::from(KeyCode::Enter);
        handle_browse_keys(&mut app, key);
        assert_eq!(app.settings_editing, Some(SettingsField::DownloadDir));

        // Store initial directory to verify text was appended
        let initial_dir = app.config.download_dir.clone();

        // Type some text
        for c in "test".chars() {
            let key = KeyEvent::from(KeyCode::Char(c));
            handle_browse_keys(&mut app, key);
        }

        // Editing is buffered, so no filesystem write occurs per keystroke.
        assert_eq!(app.config.download_dir, initial_dir);
        let expected_dir = format!("{}test", initial_dir);
        assert_eq!(
            app.settings_text_input.as_deref(),
            Some(expected_dir.as_str())
        );

        // Exit edit mode
        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);
        assert_eq!(app.settings_editing, None);
        assert_eq!(app.config.download_dir, format!("{}test", initial_dir));

        // Close settings
        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);
        assert!(!app.settings_open);

        // Verify changes persisted
        assert!(app.config.bandwidth_limit);
        assert!(app.config.download_dir.ends_with("test"));
    }

    // --- Helper ---

    fn create_test_track(id: &str, title: &str) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: title.to_string(),
            duration: "5:00".to_string(),
            channel: "Test Channel".to_string(),
            views: "1K".to_string(),
        }
    }

    // --- Video selection & play action ---

    #[test]
    fn test_enter_sets_play_action_for_selected_result() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.results = create_test_results(10);
        app.total_results = 10;
        app.page = 0;
        app.selected_index = 3;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Enter);
        handle_results_keys(&mut app, key);

        assert_eq!(app.pending_action, AppAction::Play(3));
    }

    #[test]
    fn test_enter_computes_correct_global_index_on_second_page() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.results = create_test_results(30);
        app.total_results = 30;
        app.page = 2;
        app.selected_index = 4;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Enter);
        handle_results_keys(&mut app, key);

        // page=2, page_size=10, selected=4 → global index = 24
        assert_eq!(app.pending_action, AppAction::Play(24));
    }

    #[test]
    fn test_number_quickpick_respects_current_page() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.results = create_test_results(30);
        app.total_results = 30;
        app.page = 1;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('3'));
        handle_results_keys(&mut app, key);
        let key = KeyEvent::from(KeyCode::Enter);
        handle_results_keys(&mut app, key);

        // Expected: page=1, page_size=10, number=3 → global index 12
        // Actual (bug): Play(2)
        assert_eq!(app.pending_action, AppAction::Play(12));
    }

    #[test]
    fn test_number_quickpick_accepts_displayed_global_number() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.results = create_test_results(30);
        app.total_results = 30;
        app.page = 1;
        app.focused_panel = FocusedPanel::Results;

        handle_results_keys(&mut app, KeyEvent::from(KeyCode::Char('1')));
        handle_results_keys(&mut app, KeyEvent::from(KeyCode::Char('3')));
        handle_results_keys(&mut app, KeyEvent::from(KeyCode::Enter));

        assert_eq!(app.pending_action, AppAction::Play(12));
    }

    #[test]
    fn test_number_quickpick_on_page_zero_is_correct() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.results = create_test_results(20);
        app.total_results = 20;
        app.page = 0;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('5'));
        handle_results_keys(&mut app, key);
        let key = KeyEvent::from(KeyCode::Enter);
        handle_results_keys(&mut app, key);

        // On page 0 the bug doesn't manifest: num-1 = 4, page*size+num-1 = 4
        assert_eq!(app.pending_action, AppAction::Play(4));
    }

    #[test]
    fn test_number_quickpick_out_of_range_is_ignored() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.results = create_test_results(5);
        app.total_results = 5;
        app.page = 0;
        app.focused_panel = FocusedPanel::Results;

        // Type "9" (only 5 results exist)
        let key = KeyEvent::from(KeyCode::Char('9'));
        handle_results_keys(&mut app, key);
        let key = KeyEvent::from(KeyCode::Enter);
        handle_results_keys(&mut app, key);

        assert_eq!(app.pending_action, AppAction::None);
    }

    #[test]
    fn test_down_arrow_does_not_scroll_past_current_page() {
        let mut app = App::new("test".to_string(), 5, Config::default());
        app.results = create_test_results(20);
        app.total_results = 20;
        app.page = 0;
        app.selected_index = 4; // last item on page 0
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Down);
        handle_results_keys(&mut app, key);

        // Should stay at 4, not advance to page-1 territory
        assert_eq!(app.selected_index, 4);
        assert_eq!(app.page, 0);
    }

    // --- Queue navigation ---

    #[test]
    fn test_queue_down_navigation() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue.push_back(create_test_track("2", "Track 2"));
        app.queue.push_back(create_test_track("3", "Track 3"));
        app.queue_selected_index = 0;

        let key = KeyEvent::from(KeyCode::Down);
        handle_queue_keys(&mut app, key);

        assert_eq!(app.queue_selected_index, 1);
    }

    #[test]
    fn test_queue_up_navigation() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue.push_back(create_test_track("2", "Track 2"));
        app.queue_selected_index = 1;

        let key = KeyEvent::from(KeyCode::Up);
        handle_queue_keys(&mut app, key);

        assert_eq!(app.queue_selected_index, 0);
    }

    #[test]
    fn test_queue_up_at_top_stays() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue_selected_index = 0;

        let key = KeyEvent::from(KeyCode::Up);
        handle_queue_keys(&mut app, key);

        assert_eq!(app.queue_selected_index, 0);
    }

    #[test]
    fn test_queue_down_at_bottom_stays() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue.push_back(create_test_track("2", "Track 2"));
        app.queue_selected_index = 1; // last

        let key = KeyEvent::from(KeyCode::Down);
        handle_queue_keys(&mut app, key);

        assert_eq!(app.queue_selected_index, 1);
    }

    // --- Queue delete ---

    #[test]
    fn test_queue_delete_removes_selected_item() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue.push_back(create_test_track("2", "Track 2"));
        app.queue.push_back(create_test_track("3", "Track 3"));
        app.queue_selected_index = 1;

        let key = KeyEvent::from(KeyCode::Delete);
        handle_queue_keys(&mut app, key);

        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.get(0).unwrap().id, "1");
        assert_eq!(app.queue.get(1).unwrap().id, "3");
        assert_eq!(app.queue_selected_index, 1);
    }

    #[test]
    fn test_queue_delete_clamps_index_when_last_item_removed() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue.push_back(create_test_track("2", "Track 2"));
        app.queue_selected_index = 1; // last item

        let key = KeyEvent::from(KeyCode::Delete);
        handle_queue_keys(&mut app, key);

        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.queue_selected_index, 0);
    }

    #[test]
    fn test_queue_delete_on_empty_queue_is_safe() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue_selected_index = 0;

        let key = KeyEvent::from(KeyCode::Delete);
        handle_queue_keys(&mut app, key); // Should not panic

        assert!(app.queue.is_empty());
    }

    // --- Queue clear ---

    #[test]
    fn test_queue_clear_empties_queue_and_resets_index() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue.push_back(create_test_track("2", "Track 2"));
        app.queue.push_back(create_test_track("3", "Track 3"));
        app.queue_selected_index = 2;

        let key = KeyEvent::from(KeyCode::Char('c'));
        handle_queue_keys(&mut app, key);

        assert!(app.queue.is_empty());
        assert_eq!(app.queue_selected_index, 0);
    }

    // --- Queue Enter (play / reorder) ---

    #[test]
    fn test_queue_enter_on_non_first_item_moves_it_to_front() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue.push_back(create_test_track("2", "Track 2"));
        app.queue.push_back(create_test_track("3", "Track 3"));
        app.queue_selected_index = 2; // Track 3

        assert!(promote_selected_queue_item(&mut app));

        // Track 3 moved to front, selection reset to 0
        assert_eq!(app.queue.get(0).unwrap().id, "3");
        assert_eq!(app.queue_selected_index, 0);
    }

    #[test]
    fn test_queue_enter_on_first_item_preserves_order() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(create_test_track("1", "Track 1"));
        app.queue.push_back(create_test_track("2", "Track 2"));
        app.queue_selected_index = 0;

        assert!(promote_selected_queue_item(&mut app));

        // Order unchanged
        assert_eq!(app.queue.get(0).unwrap().id, "1");
        assert_eq!(app.queue.get(1).unwrap().id, "2");
    }

    #[test]
    fn test_later_duplicate_is_not_treated_as_playing() {
        assert!(!removed_queue_item_was_playing(1, Some("duplicate-id")));
        assert!(removed_queue_item_was_playing(0, Some("duplicate-id")));
    }

    // --- Focus cycling ---

    #[test]
    fn test_tab_cycles_through_all_panels() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;

        cycle_focus_forward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::Results);
        cycle_focus_forward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::Queue);
        cycle_focus_forward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::SearchBar);
    }

    #[test]
    fn test_shift_tab_cycles_backward_through_all_panels() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::Results;

        cycle_focus_backward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::SearchBar);
        cycle_focus_backward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::Queue);
        cycle_focus_backward(&mut app);
        assert_eq!(app.focused_panel, FocusedPanel::Results);
    }

    // --- Search flow ---

    #[test]
    fn test_new_search_clears_number_input() {
        let mut app = App::new("old".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;
        app.search_input = "new query".to_string();
        app.number_input = "3".to_string();

        let key = KeyEvent::from(KeyCode::Enter);
        handle_search_bar_keys(&mut app, key);

        assert!(app.number_input.is_empty());
        assert_eq!(app.focused_panel, FocusedPanel::Results);
    }

    #[test]
    fn test_empty_search_input_does_not_trigger_search() {
        let mut app = App::new("old".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;
        app.search_input = String::new();

        let key = KeyEvent::from(KeyCode::Enter);
        handle_search_bar_keys(&mut app, key);

        assert_eq!(app.pending_action, AppAction::None);
    }

    #[test]
    fn test_search_ignores_whitespace_only_input_and_trims_queries() {
        let mut app = App::new("old".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;
        app.search_input = "   ".to_string();

        handle_search_bar_keys(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.pending_action, AppAction::None);

        app.search_input = "  new query  ".to_string();
        handle_search_bar_keys(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            app.pending_action,
            AppAction::NewSearch("new query".to_string())
        );
        assert_eq!(app.query, "new query");
    }

    #[test]
    fn test_search_bar_backspace_removes_last_char() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;
        app.search_input = "hello".to_string();

        let key = KeyEvent::from(KeyCode::Backspace);
        handle_search_bar_keys(&mut app, key);

        assert_eq!(app.search_input, "hell");
    }

    #[test]
    fn test_search_bar_accepts_q_instead_of_quitting() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;

        handle_key_event(&mut app, KeyEvent::from(KeyCode::Char('q')));

        assert_eq!(app.search_input, "q");
        assert!(!app.should_quit);
    }

    #[test]
    fn test_key_release_is_ignored() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::SearchBar;
        let key = KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );

        handle_key_event(&mut app, key);

        assert!(app.search_input.is_empty());
    }

    #[test]
    fn test_backtab_cycles_focus_backward() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::Results;

        handle_key_event(&mut app, KeyEvent::from(KeyCode::BackTab));

        assert_eq!(app.focused_panel, FocusedPanel::SearchBar);
    }

    #[test]
    fn test_question_mark_opens_help() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.focused_panel = FocusedPanel::Results;

        handle_key_event(&mut app, KeyEvent::from(KeyCode::Char('?')));

        assert_eq!(app.input_mode, InputMode::Help);
    }

    #[test]
    fn test_zero_results_per_page_is_not_applied_while_editing() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.settings_open = true;
        app.settings_selected_index = 15;
        app.settings_editing = Some(SettingsField::ResultsPerPage);
        app.settings_text_input = Some(String::new());
        let original = app.config.results_per_page;

        handle_key_event(&mut app, KeyEvent::from(KeyCode::Char('0')));

        assert_eq!(app.config.results_per_page, original);
        assert_eq!(app.settings_text_input.as_deref(), Some("0"));
    }

    #[test]
    fn test_empty_download_directory_is_rejected() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        let original = app.config.download_dir.clone();
        app.settings_editing = Some(SettingsField::DownloadDir);
        app.settings_text_input = Some("   ".to_string());

        finish_settings_edit(&mut app, SettingsField::DownloadDir);

        assert_eq!(app.config.download_dir, original);
        assert_eq!(
            app.status_message.as_deref(),
            Some("Download directory cannot be empty")
        );
    }
}
