use std::collections::VecDeque;
use crate::search::SearchResult;

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
    }

    pub fn pop_front(&mut self) -> Option<SearchResult> {
        if !self.tracks.is_empty() && self.selected_index > 0 {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
        self.tracks.pop_front()
    }

    pub fn remove(&mut self, index: usize) -> Option<SearchResult> {
        if index >= self.tracks.len() {
            return None;
        }
        if index < self.selected_index {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
        self.tracks.remove(index)
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
        if index < self.tracks.len() && index > 0 {
            let track = self.tracks.remove(index).unwrap();
            self.tracks.push_front(track);
            self.selected_index = 0;
        }
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
}
