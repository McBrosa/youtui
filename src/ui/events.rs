use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::ui::app::{App, InputMode};

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
        _ => {}
    }
}

fn handle_search_keys(_app: &mut App, _key: KeyEvent) {
    // TODO: Implement search input handling
}

fn handle_help_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('h') | KeyCode::Char('q') => {
            app.input_mode = InputMode::Browse;
        }
        _ => {}
    }
}
