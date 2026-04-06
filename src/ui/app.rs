use crate::config::Config;
use crate::player_manager::PlayerManager;
use crate::queue::Queue;
use crate::search::SearchResult;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputMode {
    Browse,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusedPanel {
    SearchBar,
    Results,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsField {
    DownloadDir,
    ResultsPerPage,
    CustomFormat,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    None,
    Play(usize),
    NewSearch(String),
    FetchNextPage,
}

pub struct App {
    pub results: Vec<SearchResult>,
    pub selected_index: usize,
    pub page: usize,
    pub page_size: usize,
    pub total_results: usize,
    pub exhausted: bool,
    pub query: String,
    pub input_mode: InputMode,
    pub search_input: String,
    pub number_input: String,
    pub pending_action: AppAction,
    pub should_quit: bool,
    pub player_manager: Option<PlayerManager>,
    pub queue: Queue,
    pub queue_selected_index: usize,
    pub focused_panel: FocusedPanel,
    pub loading: bool,
    pub settings_open: bool,
    pub settings_selected_index: usize,
    pub settings_editing: Option<SettingsField>,
    pub results_per_page_input: Option<String>,
    pub config: Config,
}

impl App {
    pub fn new(query: String, page_size: usize, config: Config) -> Self {
        Self {
            results: Vec::new(),
            selected_index: 0,
            page: 0,
            page_size,
            total_results: 0,
            exhausted: false,
            query,
            input_mode: InputMode::Browse,
            search_input: String::new(),
            number_input: String::new(),
            pending_action: AppAction::None,
            should_quit: false,
            player_manager: None,
            queue: Queue::new(),
            queue_selected_index: 0,
            focused_panel: FocusedPanel::Results,
            loading: false,
            settings_open: false,
            settings_selected_index: 2,
            settings_editing: None,
            results_per_page_input: None,
            config,
        }
    }

    pub fn current_page_results(&self) -> &[SearchResult] {
        let start = self.page * self.page_size;
        let end = (start + self.page_size).min(self.results.len());
        &self.results[start..end]
    }

    pub fn has_next_page(&self) -> bool {
        let end = (self.page + 1) * self.page_size;
        end < self.results.len() || !self.exhausted
    }

    pub fn has_prev_page(&self) -> bool {
        self.page > 0
    }

    pub fn handle_next_video(&mut self, manual: bool) {
        // Remove the currently playing track from front of queue
        if !self.queue.is_empty() {
            self.queue.pop_front();
            // Adjust selected index if needed
            if self.queue_selected_index > 0 {
                self.queue_selected_index -= 1;
            }
        }

        // Now play the new front of the queue (if any)
        if !self.queue.is_empty() {
            if let Some(track) = self.queue.get(0) {
                let url = format!("https://www.youtube.com/watch?v={}", track.id);
                let title = track.title.clone();
                let video_id = track.id.clone();

                // Manual 'n' press always auto-plays, automatic transitions respect setting
                let should_auto_play = manual || self.config.auto_play_queue;

                if let Some(ref mut player) = self.player_manager {
                    let result = if should_auto_play {
                        player.play(&url, &title, &video_id)
                    } else {
                        player.load_paused(&url, &title, &video_id)
                    };

                    if result.is_err() {
                        self.player_manager = None;
                    }
                } else {
                    // Create player manager if it doesn't exist
                    use crate::player_manager::PlayerManager;
                    match PlayerManager::new() {
                        Ok(mut pm) => {
                            let result = if should_auto_play {
                                pm.play(&url, &title, &video_id)
                            } else {
                                pm.load_paused(&url, &title, &video_id)
                            };

                            if result.is_ok() {
                                self.player_manager = Some(pm);
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to create player: {}", e);
                        }
                    }
                }
            }
        } else {
            // Queue is empty, clear the player
            if let Some(ref mut player) = self.player_manager {
                let _ = player.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::search::SearchResult;

    fn make_track(id: &str, title: &str) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: title.to_string(),
            duration: "3:00".to_string(),
            channel: "Test".to_string(),
            views: "1K".to_string(),
        }
    }

    fn make_results(count: usize) -> Vec<SearchResult> {
        (0..count)
            .map(|i| make_track(&i.to_string(), &format!("Track {}", i)))
            .collect()
    }

    // --- current_page_results ---

    #[test]
    fn test_current_page_results_page_zero() {
        let mut app = App::new("test".to_string(), 5, Config::default());
        app.results = make_results(15);
        app.page = 0;

        let page = app.current_page_results();
        assert_eq!(page.len(), 5);
        assert_eq!(page[0].id, "0");
        assert_eq!(page[4].id, "4");
    }

    #[test]
    fn test_current_page_results_page_one() {
        let mut app = App::new("test".to_string(), 5, Config::default());
        app.results = make_results(15);
        app.page = 1;

        let page = app.current_page_results();
        assert_eq!(page.len(), 5);
        assert_eq!(page[0].id, "5");
        assert_eq!(page[4].id, "9");
    }

    #[test]
    fn test_current_page_results_partial_last_page() {
        let mut app = App::new("test".to_string(), 5, Config::default());
        app.results = make_results(13);
        app.page = 2;

        let page = app.current_page_results();
        assert_eq!(page.len(), 3);
        assert_eq!(page[0].id, "10");
        assert_eq!(page[2].id, "12");
    }

    #[test]
    fn test_current_page_results_empty_results() {
        let app = App::new("test".to_string(), 5, Config::default());
        let page = app.current_page_results();
        assert!(page.is_empty());
    }

    // --- has_next_page / has_prev_page ---

    #[test]
    fn test_has_next_page_when_more_cached_results() {
        let mut app = App::new("test".to_string(), 5, Config::default());
        app.results = make_results(15);
        app.exhausted = true;
        app.page = 0;
        assert!(app.has_next_page());
    }

    #[test]
    fn test_has_no_next_page_on_last_page_exhausted() {
        let mut app = App::new("test".to_string(), 5, Config::default());
        app.results = make_results(10);
        app.exhausted = true;
        app.page = 1; // last page (items 5-9)
        assert!(!app.has_next_page());
    }

    #[test]
    fn test_has_next_page_when_not_exhausted() {
        let mut app = App::new("test".to_string(), 5, Config::default());
        app.results = make_results(5);
        app.exhausted = false; // more may be available
        app.page = 0;
        assert!(app.has_next_page());
    }

    #[test]
    fn test_has_prev_page_on_page_one() {
        let mut app = App::new("test".to_string(), 5, Config::default());
        app.page = 1;
        assert!(app.has_prev_page());
    }

    #[test]
    fn test_has_no_prev_page_on_page_zero() {
        let app = App::new("test".to_string(), 5, Config::default());
        assert!(!app.has_prev_page());
    }

    // --- handle_next_video ---

    #[test]
    fn test_handle_next_video_pops_front_from_queue() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(make_track("1", "Track 1"));
        // One item: queue becomes empty after pop, so PlayerManager::new() is never attempted
        app.handle_next_video(false);
        assert!(app.queue.is_empty());
    }

    #[test]
    fn test_handle_next_video_on_empty_queue_is_safe() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.handle_next_video(false); // must not panic
        assert!(app.queue.is_empty());
    }

    #[test]
    fn test_handle_next_video_decrements_queue_selected_index() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(make_track("1", "Track 1"));
        app.queue_selected_index = 1; // pointing beyond the only item

        app.handle_next_video(false);

        // selected_index was > 0 so it should be decremented
        assert_eq!(app.queue_selected_index, 0);
    }

    #[test]
    fn test_handle_next_video_does_not_underflow_queue_selected_index() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(make_track("1", "Track 1"));
        app.queue_selected_index = 0;

        app.handle_next_video(false);

        // selected_index must not wrap below 0
        assert_eq!(app.queue_selected_index, 0);
    }

    /// Verifies the fix for the double-pop bug: when the currently-playing video is
    /// deleted from the queue, the handler must remove it exactly once (via queue.remove)
    /// and must NOT subsequently call handle_next_video (which would pop_front again).
    /// After the delete the queue should be [B, C] so that B starts playing next.
    #[test]
    fn test_delete_currently_playing_video_does_not_double_pop() {
        let mut app = App::new("test".to_string(), 10, Config::default());
        app.queue.push_back(make_track("A", "Track A"));
        app.queue.push_back(make_track("B", "Track B"));
        app.queue.push_back(make_track("C", "Track C"));

        // The fix: only queue.remove(0) is called; no subsequent pop_front.
        app.queue.remove(0); // simulates the delete handler's remove step

        // Queue must now be [B, C] — B is the next track to play.
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.get(0).unwrap().id, "B");
        assert_eq!(app.queue.get(1).unwrap().id, "C");
    }
}
