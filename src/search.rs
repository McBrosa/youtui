use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;

const MIN_DURATION: u32 = 180;
const SEARCH_CEILING: usize = 500;

#[derive(Clone)]
pub struct SearchResult {
    pub title: String,
    pub duration: String,
    pub channel: String,
    pub views: String,
    pub id: String,
}

impl SearchResult {
    pub fn from_line_parts(title: &str, duration: &str, channel: &str, views: &str, id: &str) -> Option<Self> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        Some(SearchResult {
            title: title.to_string(),
            duration: duration.to_string(),
            channel: channel.to_string(),
            views: views.to_string(),
            id: id.to_string(),
        })
    }

    #[allow(dead_code)]
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }

    pub fn safe_title(&self) -> String {
        self.title
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '_' || *c == ' ' || *c == '-')
            .collect()
    }
}

pub fn check_ytdlp() -> Result<()> {
    which::which("yt-dlp").map_err(|_| {
        anyhow::anyhow!("yt-dlp is not installed\nPlease install it with: pip install yt-dlp")
    })?;
    Ok(())
}

/// Lazy-paginated search: fetches one batch of raw yt-dlp results at a time
/// and caches everything already fetched.
pub struct PaginatedSearch {
    query: String,
    page_size: usize,
    filter_shorts: bool,
    /// All results that have passed filtering so far.
    pub results: Vec<SearchResult>,
    /// How many raw yt-dlp playlist items we have consumed (1-indexed high-water mark).
    raw_cursor: usize,
    /// No more results available from yt-dlp.
    pub exhausted: bool,
}

impl PaginatedSearch {
    pub fn new(query: &str, page_size: usize, filter_shorts: bool) -> Self {
        PaginatedSearch {
            query: query.to_string(),
            page_size,
            filter_shorts,
            results: Vec::new(),
            raw_cursor: 0,
            exhausted: false,
        }
    }

    /// Reset for a brand-new query, clearing the cache.
    pub fn reset(&mut self, query: &str) {
        self.query = query.to_string();
        self.results.clear();
        self.raw_cursor = 0;
        self.exhausted = false;
    }

    /// Make sure we have enough filtered results to display `page` (0-indexed).
    /// Returns the number of displayable results we have.
    pub fn ensure_page(&mut self, page: usize) -> Result<usize> {
        let needed = (page + 1) * self.page_size;
        while self.results.len() < needed && !self.exhausted {
            self.fetch_batch()?;
        }
        Ok(self.results.len())
    }

    /// Fetch one batch of raw results from yt-dlp and append the ones that
    /// pass filtering to `self.results`.
    fn fetch_batch(&mut self) -> Result<()> {
        // When filtering shorts, many raw results get discarded, so fetch
        // a larger raw batch to fill a display page in fewer round-trips.
        let raw_batch = if self.filter_shorts {
            self.page_size * 3
        } else {
            self.page_size
        };

        let start = self.raw_cursor + 1; // yt-dlp playlist items are 1-indexed
        let end = (self.raw_cursor + raw_batch).min(SEARCH_CEILING);

        if start > SEARCH_CEILING {
            self.exhausted = true;
            return Ok(());
        }

        let search_id = format!("ytsearch{}:{}", SEARCH_CEILING, self.query);
        let range = format!("{}:{}", start, end);

        if self.results.is_empty() {
            println!("{}", "Searching...".yellow());
        } else {
            println!("{}", "Fetching more results...".yellow());
        }

        let mut cmd = Command::new("yt-dlp");
        cmd.arg("--flat-playlist")
            .arg("--no-warnings")
            .arg("--playlist-items").arg(&range)
            .arg(&search_id)
            .arg("--print")
            .arg("%(title)s|%(duration_string|N/A)s|%(channel|Unknown)s|%(view_count|0)s|%(id)s|%(duration|0)s");

        let output = cmd
            .output()
            .context("Failed to run yt-dlp search")?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if !output.status.success() && stdout.is_empty() && self.raw_cursor == 0 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "{} yt-dlp search returned exit code {}",
                "Warning:".red(),
                output.status.code().unwrap_or(-1)
            );
            if !stderr.is_empty() {
                eprintln!("{}", stderr);
            }
        }

        let mut raw_count = 0;
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            raw_count += 1;

            let parts: Vec<&str> = line.splitn(6, '|').collect();
            if parts.len() < 6 {
                continue;
            }

            if self.filter_shorts {
                let secs: f64 = parts[5].trim().parse().unwrap_or(0.0);
                if secs < MIN_DURATION as f64 {
                    continue;
                }
            }

            if let Some(result) = SearchResult::from_line_parts(parts[0], parts[1], parts[2], parts[3], parts[4]) {
                self.results.push(result);
            }
        }

        self.raw_cursor = end;

        // If yt-dlp returned fewer raw items than we asked for, or we hit
        // the ceiling, there's nothing left.
        if raw_count < raw_batch || end >= SEARCH_CEILING {
            self.exhausted = true;
        }

        println!(
            "{}",
            format!("Found {} videos so far.", self.results.len()).green()
        );

        Ok(())
    }
}
