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
}
