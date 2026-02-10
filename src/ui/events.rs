use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::ui::app::{App, AppAction, FocusedPanel, InputMode, SettingsField};

pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // Global Tab key for focus cycling (works in any mode except Help)
    if app.input_mode != InputMode::Help && key.code == KeyCode::Tab {
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            cycle_focus_backward(app);
        } else {
            cycle_focus_forward(app);
        }
        return;
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

    // Global settings toggle (works from any panel except Help, but not while editing)
    if app.input_mode != InputMode::Help {
        match key.code {
            KeyCode::Char('s') | KeyCode::Char('S') if app.focused_panel != FocusedPanel::SearchBar => {
                app.settings_open = !app.settings_open;
                return;
            }
            KeyCode::F(2) => {
                app.settings_open = !app.settings_open;
                return;
            }
            _ => {}
        }
    }

    // Global quit keys work from any panel
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) => {
            app.should_quit = true;
            return;
        }
        (KeyCode::Esc, _) => {
            // Esc in search bar returns to results, otherwise quit
            if app.focused_panel == FocusedPanel::SearchBar {
                app.search_input.clear();
                app.focused_panel = FocusedPanel::Results;
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
    if app.player_manager.is_some() {
        match key.code {
            KeyCode::Char(' ') => {
                if let Some(pm) = app.player_manager.as_mut() {
                    let _ = pm.toggle_pause();
                }
                return;
            }
            KeyCode::Char('<') => {
                if let Some(pm) = app.player_manager.as_mut() {
                    let _ = pm.seek(-10.0);
                }
                return;
            }
            KeyCode::Char('>') => {
                if let Some(pm) = app.player_manager.as_mut() {
                    let _ = pm.seek(10.0);
                }
                return;
            }
            KeyCode::Char('=') | KeyCode::Char('+') => {
                let new_vol = app.player_manager.as_ref().unwrap().status.volume + 5;
                let new_vol = new_vol.min(100);
                if let Some(pm) = app.player_manager.as_mut() {
                    let _ = pm.set_volume(new_vol);
                }
                return;
            }
            KeyCode::Char('-') if app.focused_panel != FocusedPanel::SearchBar => {
                let new_vol = app.player_manager.as_ref().unwrap().status.volume - 5;
                let new_vol = new_vol.max(0);
                if let Some(pm) = app.player_manager.as_mut() {
                    let _ = pm.set_volume(new_vol);
                }
                return;
            }
            KeyCode::Char('m') if app.focused_panel != FocusedPanel::SearchBar => {
                let current_vol = app.player_manager.as_ref().unwrap().status.volume;
                let new_vol = if current_vol > 0 { 0 } else { 100 };
                if let Some(pm) = app.player_manager.as_mut() {
                    let _ = pm.set_volume(new_vol);
                }
                return;
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

fn handle_results_keys(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            if app.selected_index > 0 {
                app.selected_index -= 1;
            }
        }
        (KeyCode::Down, _) => {
            let page_results = app.current_page_results();
            if app.selected_index < page_results.len().saturating_sub(1) {
                app.selected_index += 1;
            }
        }
        (KeyCode::Char('n'), _) if app.has_next_page() => {
            app.page += 1;
            app.selected_index = 0;

            let end = (app.page + 1) * app.page_size;
            if end > app.results.len() && !app.exhausted {
                app.pending_action = AppAction::FetchNextPage;
            }
        }
        (KeyCode::Char('p'), _) if app.has_prev_page() => {
            app.page -= 1;
            app.selected_index = 0;
        }
        (KeyCode::Char('h'), _) => {
            app.input_mode = InputMode::Help;
        }
        (KeyCode::Char('s'), _) => {
            app.focused_panel = FocusedPanel::SearchBar;
        }
        (KeyCode::Char(c), _) if c.is_ascii_digit() => {
            app.number_input.push(c);
        }
        (KeyCode::Enter, _) => {
            let idx = if !app.number_input.is_empty() {
                let result = app.number_input.parse::<usize>().ok().and_then(|num| {
                    if num > 0 && num <= app.results.len() {
                        Some(num - 1)
                    } else {
                        None
                    }
                });
                app.number_input.clear();
                result
            } else {
                Some(app.page * app.page_size + app.selected_index)
            };

            if let Some(idx) = idx {
                if idx < app.results.len() {
                    app.pending_action = AppAction::Play(idx);
                }
            }
        }
        (KeyCode::Backspace, _) => {
            app.number_input.pop();
        }
        _ => {}
    }
}

fn handle_search_bar_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => {
            app.search_input.push(c);
        }
        KeyCode::Backspace => {
            app.search_input.pop();
        }
        KeyCode::Enter => {
            if !app.search_input.is_empty() {
                // Update query immediately so search bar shows new query
                app.query = app.search_input.clone();
                app.loading = true;
                app.pending_action = AppAction::NewSearch(app.search_input.clone());
                app.search_input.clear();
                app.focused_panel = FocusedPanel::Results;
            }
        }
        KeyCode::Esc => {
            app.search_input.clear();
            app.focused_panel = FocusedPanel::Results;
        }
        _ => {}
    }
}

fn handle_queue_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Up => {
            if app.queue_selected_index > 0 {
                app.queue_selected_index -= 1;
            }
        }
        KeyCode::Down => {
            if app.queue_selected_index < app.queue.len().saturating_sub(1) {
                app.queue_selected_index += 1;
            }
        }
        KeyCode::Enter => {
            if !app.queue.is_empty() && app.queue_selected_index < app.queue.len() {
                if app.queue_selected_index > 0 {
                    app.queue.move_to_front(app.queue_selected_index);
                    app.queue_selected_index = 0;
                }

                if let Some(ref mut player) = app.player_manager {
                    if let Some(track) = app.queue.get(0) {
                        let url = format!("https://www.youtube.com/watch?v={}", track.id);
                        let title = track.title.clone();
                        let _ = player.play(&url, &title);
                    }
                }
            }
        }
        KeyCode::Delete | KeyCode::Backspace => {
            app.queue.remove(app.queue_selected_index);
            if app.queue_selected_index >= app.queue.len() && app.queue_selected_index > 0 {
                app.queue_selected_index -= 1;
            }
        }
        KeyCode::Char('c') => {
            app.queue.clear();
            app.queue_selected_index = 0;
        }
        KeyCode::Char('n') => {
            // Next track (from queue panel)
            if app.player_manager.is_some() {
                let next = app.queue.pop_front();
                if let Some(next_track) = next {
                    let url = format!("https://www.youtube.com/watch?v={}", next_track.id);
                    let title = next_track.title.clone();
                    if let Some(pm) = app.player_manager.as_mut() {
                        let _ = pm.play(&url, &title);
                    }
                }
            }
        }
        KeyCode::Char('h') => {
            app.input_mode = InputMode::Help;
        }
        _ => {}
    }
}

fn handle_help_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('h') | KeyCode::Char('q') => {
            app.input_mode = InputMode::Browse;
        }
        _ => {}
    }
}

fn handle_settings_keys(app: &mut App, key: KeyEvent) {
    // If editing a text field, handle text input
    if let Some(field) = app.settings_editing {
        match key.code {
            KeyCode::Char(c) => {
                // Append character to appropriate field
                match field {
                    SettingsField::DownloadDir => {
                        app.config.download_dir.push(c);
                        let _ = app.config.save();
                    }
                    SettingsField::ResultsPerPage => {
                        // Only accept digits
                        if c.is_ascii_digit() {
                            if let Some(ref mut input) = app.results_per_page_input {
                                input.push(c);
                                // Try to parse and save (if valid)
                                if let Ok(new_val) = input.parse::<usize>() {
                                    if new_val <= 999 {  // Reasonable upper bound
                                        app.config.results_per_page = new_val;
                                        let _ = app.config.save();
                                    }
                                }
                            }
                        }
                    }
                    SettingsField::CustomFormat => {
                        app.config.custom_format.push(c);
                        let _ = app.config.save();
                    }
                }
            }
            KeyCode::Backspace => {
                // Remove last character
                match field {
                    SettingsField::DownloadDir => {
                        app.config.download_dir.pop();
                        let _ = app.config.save();
                    }
                    SettingsField::ResultsPerPage => {
                        if let Some(ref mut input) = app.results_per_page_input {
                            input.pop();
                            // If not empty, try to parse and save
                            if !input.is_empty() {
                                if let Ok(new_val) = input.parse::<usize>() {
                                    app.config.results_per_page = new_val;
                                    let _ = app.config.save();
                                }
                            }
                            // If empty string, don't update config yet (wait for exit)
                        }
                    }
                    SettingsField::CustomFormat => {
                        app.config.custom_format.pop();
                        let _ = app.config.save();
                    }
                }
            }
            KeyCode::Enter | KeyCode::Esc => {
                // Exit edit mode and finalize ResultsPerPage
                if field == SettingsField::ResultsPerPage {
                    if let Some(ref input) = app.results_per_page_input {
                        // Parse the input string, default to 20 if empty or invalid
                        let value = input.parse::<usize>().unwrap_or(20);
                        // Clamp to 1-100 range
                        app.config.results_per_page = value.clamp(1, 100);
                        let _ = app.config.save();
                    }
                    app.results_per_page_input = None;
                }
                app.settings_editing = None;
            }
            _ => {}
        }
        return;
    }

    // Define selectable indices (skip section headers)
    const SELECTABLE_INDICES: &[usize] = &[2, 3, 4, 5, 9, 10, 14, 18];

    match key.code {
        KeyCode::Esc => {
            app.settings_open = false;
            app.settings_editing = None;
            app.results_per_page_input = None;
        }
        KeyCode::Up => {
            // Find the previous selectable index
            let current = app.settings_selected_index;
            let pos = SELECTABLE_INDICES.iter().position(|&x| x == current);

            if let Some(pos) = pos {
                if pos > 0 {
                    app.settings_selected_index = SELECTABLE_INDICES[pos - 1];
                }
            }
        }
        KeyCode::Down => {
            // Find the next selectable index
            let current = app.settings_selected_index;
            let pos = SELECTABLE_INDICES.iter().position(|&x| x == current);

            if let Some(pos) = pos {
                if pos < SELECTABLE_INDICES.len() - 1 {
                    app.settings_selected_index = SELECTABLE_INDICES[pos + 1];
                }
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            // Handle action based on the selected index
            match app.settings_selected_index {
                2 => {
                    // Audio Only checkbox
                    let _ = app.config.toggle_audio_only();
                }
                3 => {
                    // Bandwidth Limit checkbox
                    let _ = app.config.toggle_bandwidth_limit();
                }
                4 => {
                    // Keep Temp checkbox
                    let _ = app.config.toggle_keep_temp();
                }
                5 => {
                    // Include Shorts checkbox
                    let _ = app.config.toggle_include_shorts();
                }
                9 => {
                    // Download Mode checkbox
                    let _ = app.config.toggle_download_mode();
                }
                10 => {
                    // Download Dir text field - enter edit mode
                    app.settings_editing = Some(SettingsField::DownloadDir);
                }
                14 => {
                    // Results Per Page text field - enter edit mode
                    app.settings_editing = Some(SettingsField::ResultsPerPage);
                    app.results_per_page_input = Some(app.config.results_per_page.to_string());
                }
                18 => {
                    // Custom Format text field - enter edit mode
                    app.settings_editing = Some(SettingsField::CustomFormat);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchResult;
    use crate::config::Config;

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

        assert_eq!(app.pending_action, AppAction::NewSearch("new query".to_string()));
        assert_eq!(app.focused_panel, FocusedPanel::Results);
        assert!(app.search_input.is_empty());
    }

    #[test]
    fn test_settings_open_with_s_key() {
        let mut app = App::new("test query".to_string(), 10, Config::default());
        app.settings_open = false;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('s'));
        handle_browse_keys(&mut app, key);

        assert!(app.settings_open);
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
        app.settings_selected_index = 10; // Download Dir

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
        app.focused_panel = FocusedPanel::Results;

        // Open settings with 's' key
        let key = KeyEvent::from(KeyCode::Char('s'));
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

        // Navigate to download dir (index 10)
        // From index 3, need to go: 4, 5, 9, 10 (4 down presses)
        for _ in 0..4 {
            let key = KeyEvent::from(KeyCode::Down);
            handle_browse_keys(&mut app, key);
        }
        assert_eq!(app.settings_selected_index, 10);

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

        // Verify text was appended
        assert_eq!(app.config.download_dir, format!("{}test", initial_dir));

        // Exit edit mode
        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);
        assert_eq!(app.settings_editing, None);

        // Close settings
        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);
        assert!(!app.settings_open);

        // Verify changes persisted
        assert!(app.config.bandwidth_limit);
        assert!(app.config.download_dir.ends_with("test"));
    }
}
