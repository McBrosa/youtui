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
