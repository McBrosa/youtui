use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::ui::app::{App, AppAction, InputMode};

#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
}

pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    match app.input_mode {
        InputMode::Browse => handle_browse_keys(app, key),
        InputMode::Search => handle_search_keys(app, key),
        InputMode::Help => handle_help_keys(app, key),
    }
}

fn handle_browse_keys(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            app.should_quit = true;
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
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
        }
        (KeyCode::Char('p'), _) if app.has_prev_page() => {
            app.page -= 1;
            app.selected_index = 0;
        }
        (KeyCode::Char('h'), _) => {
            app.input_mode = InputMode::Help;
        }
        (KeyCode::Char('s'), _) => {
            app.input_mode = InputMode::Search;
            app.search_input.clear();
        }
        (KeyCode::Char(c), _) if c.is_ascii_digit() => {
            // Number quick-pick
            app.number_input.push(c);
        }
        (KeyCode::Enter, _) => {
            // Handle enter for number input or arrow selection
            if !app.number_input.is_empty() {
                if let Ok(num) = app.number_input.parse::<usize>() {
                    if num > 0 && num <= app.results.len() {
                        app.pending_action = AppAction::Play(num - 1);
                    }
                }
                app.number_input.clear();
            } else {
                // Play selected item
                let global_idx = app.page * app.page_size + app.selected_index;
                app.pending_action = AppAction::Play(global_idx);
            }
        }
        (KeyCode::Backspace, _) => {
            app.number_input.pop();
        }
        _ => {}
    }
}

fn handle_search_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => {
            app.search_input.push(c);
        }
        KeyCode::Backspace => {
            app.search_input.pop();
        }
        KeyCode::Enter => {
            if !app.search_input.is_empty() {
                app.query = app.search_input.clone();
                // TODO: Trigger new search
                app.input_mode = InputMode::Browse;
            }
        }
        KeyCode::Esc => {
            app.input_mode = InputMode::Browse;
            app.search_input.clear();
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

        let key = KeyEvent::from(KeyCode::Up);
        handle_browse_keys(&mut app, key);

        // Should stay at 0
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn test_arrow_down_navigation() {
        let mut app = App::new("test query".to_string(), 10);
        app.results = create_test_results(5);
        app.total_results = 5;
        app.selected_index = 2;

        let key = KeyEvent::from(KeyCode::Down);
        handle_browse_keys(&mut app, key);

        assert_eq!(app.selected_index, 3);
    }

    #[test]
    fn test_arrow_down_at_bottom() {
        let mut app = App::new("test query".to_string(), 10);
        app.results = create_test_results(5);
        app.total_results = 5;
        app.selected_index = 4; // Last item on page

        let key = KeyEvent::from(KeyCode::Down);
        handle_browse_keys(&mut app, key);

        // Should stay at 4 (last index)
        assert_eq!(app.selected_index, 4);
    }

    #[test]
    fn test_next_page_navigation() {
        let mut app = App::new("test query".to_string(), 10);
        app.results = create_test_results(25);
        app.total_results = 25;
        app.page = 0;
        app.selected_index = 5;

        let key = KeyEvent::from(KeyCode::Char('n'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.page, 1);
        assert_eq!(app.selected_index, 0); // Reset to top of new page
    }

    #[test]
    fn test_next_page_when_not_available() {
        let mut app = App::new("test query".to_string(), 10);
        app.results = create_test_results(10);
        app.total_results = 10;
        app.exhausted = true;
        app.page = 0;

        let key = KeyEvent::from(KeyCode::Char('n'));
        handle_browse_keys(&mut app, key);

        // Should not advance
        assert_eq!(app.page, 0);
    }

    #[test]
    fn test_prev_page_navigation() {
        let mut app = App::new("test query".to_string(), 10);
        app.results = create_test_results(25);
        app.total_results = 25;
        app.page = 2;
        app.selected_index = 5;

        let key = KeyEvent::from(KeyCode::Char('p'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.page, 1);
        assert_eq!(app.selected_index, 0); // Reset to top of new page
    }

    #[test]
    fn test_prev_page_at_first_page() {
        let mut app = App::new("test query".to_string(), 10);
        app.results = create_test_results(25);
        app.total_results = 25;
        app.page = 0;

        let key = KeyEvent::from(KeyCode::Char('p'));
        handle_browse_keys(&mut app, key);

        // Should not go back
        assert_eq!(app.page, 0);
    }

    #[test]
    fn test_help_key_switches_mode() {
        let mut app = App::new("test query".to_string(), 10);
        app.input_mode = InputMode::Browse;

        let key = KeyEvent::from(KeyCode::Char('h'));
        handle_browse_keys(&mut app, key);

        assert_eq!(app.input_mode, InputMode::Help);
    }

    #[test]
    fn test_quit_key() {
        let mut app = App::new("test query".to_string(), 10);
        app.should_quit = false;

        let key = KeyEvent::from(KeyCode::Char('q'));
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }

    #[test]
    fn test_esc_key_quits() {
        let mut app = App::new("test query".to_string(), 10);
        app.should_quit = false;

        let key = KeyEvent::from(KeyCode::Esc);
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }

    #[test]
    fn test_ctrl_c_quits() {
        let mut app = App::new("test query".to_string(), 10);
        app.should_quit = false;

        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        handle_browse_keys(&mut app, key);

        assert!(app.should_quit);
    }
}
