use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::ui::app::{App, AppAction, FocusedPanel, InputMode};

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

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut app = App::new("test query".to_string(), 10);
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
        let mut app = App::new("test query".to_string(), 10);
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
        let mut app = App::new("test query".to_string(), 10);
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
        let mut app = App::new("test query".to_string(), 10);
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
        let mut app = App::new("test query".to_string(), 10);
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
        let mut app = App::new("test query".to_string(), 10);
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
        let mut app = App::new("test query".to_string(), 10);
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
        let mut app = App::new("test query".to_string(), 10);
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
        let mut app = App::new("test query".to_string(), 10);
        app.input_mode = InputMode::Browse;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('h'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.input_mode, InputMode::Help);
    }

    #[test]
    fn test_quit_key() {
        let mut app = App::new("test query".to_string(), 10);
        app.should_quit = false;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Char('q'));
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }

    #[test]
    fn test_esc_key_quits_from_results() {
        let mut app = App::new("test query".to_string(), 10);
        app.should_quit = false;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }

    #[test]
    fn test_ctrl_c_quits() {
        let mut app = App::new("test query".to_string(), 10);
        app.should_quit = false;
        app.focused_panel = FocusedPanel::Results;

        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }

    #[test]
    fn test_tab_cycles_focus_forward() {
        let mut app = App::new("test".to_string(), 10);
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
        let mut app = App::new("test".to_string(), 10);
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
        let mut app = App::new("test".to_string(), 10);
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
        let mut app = App::new("old query".to_string(), 10);
        app.focused_panel = FocusedPanel::SearchBar;
        app.search_input = "new query".to_string();

        let key = KeyEvent::from(KeyCode::Enter);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.pending_action, AppAction::NewSearch("new query".to_string()));
        assert_eq!(app.focused_panel, FocusedPanel::Results);
        assert!(app.search_input.is_empty());
    }
}
