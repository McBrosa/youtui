use crate::search::SearchResult;
use std::collections::VecDeque;

pub struct Queue {
    tracks: VecDeque<SearchResult>,
    pub selected_index: usize,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            tracks: VecDeque::new(),
            selected_index: 0,
        }
    }

    pub fn push_back(&mut self, track: SearchResult) {
        self.tracks.push_back(track);
        self.normalize_selection();
    }

    pub fn pop_front(&mut self) -> Option<SearchResult> {
        let track = self.tracks.pop_front();
        if track.is_some() && self.selected_index > 0 {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
        self.normalize_selection();
        track
    }

    pub fn remove(&mut self, index: usize) -> Option<SearchResult> {
        let track = self.tracks.remove(index)?;
        if index < self.selected_index {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
        self.normalize_selection();
        Some(track)
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.selected_index = 0;
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&SearchResult> {
        self.tracks.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SearchResult> {
        self.tracks.iter()
    }

    pub fn move_to_front(&mut self, index: usize) {
        if index < self.tracks.len()
            && index > 0
            && let Some(track) = self.tracks.remove(index)
        {
            self.tracks.push_front(track);
            self.selected_index = 0;
        }
        self.normalize_selection();
    }

    fn normalize_selection(&mut self) {
        self.selected_index = self.selected_index.min(self.tracks.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_track(id: &str, title: &str) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: title.to_string(),
            duration: "3:00".to_string(),
            channel: "Test".to_string(),
            views: "1K".to_string(),
        }
    }

    #[test]
    fn test_push_and_len() {
        let mut queue = Queue::new();
        assert_eq!(queue.len(), 0);

        queue.push_back(create_test_track("1", "Track 1"));
        assert_eq!(queue.len(), 1);

        queue.push_back(create_test_track("2", "Track 2"));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_pop_front() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));

        let track = queue.pop_front().unwrap();
        assert_eq!(track.id, "1");
        assert_eq!(queue.len(), 1);

        let track = queue.pop_front().unwrap();
        assert_eq!(track.id, "2");
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_remove() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));

        let track = queue.remove(1).unwrap();
        assert_eq!(track.id, "2");
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_clear() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));

        queue.clear();
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }

    // --- move_to_front ---

    #[test]
    fn test_move_to_front_middle_item() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));

        queue.move_to_front(2);

        assert_eq!(queue.get(0).unwrap().id, "3");
        assert_eq!(queue.get(1).unwrap().id, "1");
        assert_eq!(queue.get(2).unwrap().id, "2");
        assert_eq!(queue.selected_index, 0);
    }

    #[test]
    fn test_move_to_front_already_at_front_is_noop() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.selected_index = 1;

        queue.move_to_front(0);

        // Order unchanged, selected_index preserved
        assert_eq!(queue.get(0).unwrap().id, "1");
        assert_eq!(queue.get(1).unwrap().id, "2");
        assert_eq!(queue.selected_index, 1);
    }

    #[test]
    fn test_move_to_front_resets_selected_index() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));
        queue.selected_index = 2;

        queue.move_to_front(2);

        assert_eq!(queue.selected_index, 0);
    }

    // --- pop_front adjusts selected_index ---

    #[test]
    fn test_pop_front_decrements_selected_index_when_above_zero() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));
        queue.selected_index = 2;

        queue.pop_front();

        assert_eq!(queue.selected_index, 1);
        assert_eq!(queue.get(0).unwrap().id, "2");
    }

    #[test]
    fn test_pop_front_does_not_underflow_selected_index() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.selected_index = 0;

        queue.pop_front();

        assert_eq!(queue.selected_index, 0);
    }

    // --- remove adjusts selected_index ---

    #[test]
    fn test_remove_item_before_selected_decrements_index() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));
        queue.selected_index = 2;

        queue.remove(0); // Remove Track 1 (before selected)

        assert_eq!(queue.selected_index, 1);
    }

    #[test]
    fn test_remove_item_after_selected_does_not_change_index() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));
        queue.selected_index = 0;

        queue.remove(2); // Remove Track 3 (after selected)

        assert_eq!(queue.selected_index, 0);
    }

    #[test]
    fn test_remove_at_selected_index_does_not_adjust_index() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));
        queue.selected_index = 1;

        queue.remove(1); // Remove the selected item itself

        // index == removed index, no decrement (per `index < selected_index` condition)
        assert_eq!(queue.selected_index, 1);
    }

    #[test]
    fn test_remove_selected_last_item_clamps_selection() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.selected_index = 1;

        queue.remove(1);

        assert_eq!(queue.selected_index, 0);
        assert_eq!(queue.get(0).unwrap().id, "1");
    }

    #[test]
    fn test_empty_pop_repairs_out_of_range_selection() {
        let mut queue = Queue::new();
        queue.selected_index = usize::MAX;

        assert!(queue.pop_front().is_none());
        assert_eq!(queue.selected_index, 0);
    }

    #[test]
    fn test_remove_out_of_bounds_returns_none_and_is_safe() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));

        let result = queue.remove(5);

        assert!(result.is_none());
        assert_eq!(queue.len(), 1);
    }

    // --- get / iter ---

    #[test]
    fn test_get_valid_index() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));

        assert_eq!(queue.get(0).unwrap().id, "1");
        assert_eq!(queue.get(1).unwrap().id, "2");
    }

    #[test]
    fn test_get_out_of_bounds_returns_none() {
        let queue = Queue::new();
        assert!(queue.get(0).is_none());
    }

    #[test]
    fn test_iter_returns_all_tracks_in_order() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.push_back(create_test_track("3", "Track 3"));

        let ids: Vec<&str> = queue.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2", "3"]);
    }

    #[test]
    fn test_clear_resets_selected_index() {
        let mut queue = Queue::new();
        queue.push_back(create_test_track("1", "Track 1"));
        queue.push_back(create_test_track("2", "Track 2"));
        queue.selected_index = 1;

        queue.clear();

        assert_eq!(queue.selected_index, 0);
        assert!(queue.is_empty());
    }
}
