use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::config::clamp_results_per_page;

const MIN_DURATION: u32 = 180;
const SEARCH_CEILING: usize = 500;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(45);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub duration: String,
    pub channel: String,
    pub views: String,
    /// Relative upload age ("3 days ago"), empty when unknown.
    pub published: String,
    pub id: String,
}

impl SearchResult {
    pub fn from_line_parts(
        title: &str,
        duration: &str,
        channel: &str,
        views: &str,
        published: &str,
        id: &str,
    ) -> Option<Self> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        Some(SearchResult {
            title: title.to_string(),
            duration: duration.to_string(),
            channel: channel.to_string(),
            views: views.to_string(),
            published: published.to_string(),
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
#[derive(Clone)]
pub struct PaginatedSearch {
    query: String,
    pub page_size: usize,
    pub filter_shorts: bool,
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
            page_size: clamp_results_per_page(page_size),
            filter_shorts,
            results: Vec::new(),
            raw_cursor: 0,
            exhausted: false,
        }
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    /// Make sure we have enough filtered results to display `page` (0-indexed).
    /// Returns the number of displayable results we have.
    #[cfg(test)]
    fn ensure_page(&mut self, page: usize) -> Result<usize> {
        self.ensure_page_with_cancel(page, &AtomicBool::new(false))
    }

    #[cfg(test)]
    pub(crate) fn ensure_page_with_cancel(
        &mut self,
        page: usize,
        cancelled: &AtomicBool,
    ) -> Result<usize> {
        self.ensure_page_with_cancel_and_progress(page, cancelled, |_| {})
    }

    /// Fetch enough accepted entries for `page` plus one page of look-ahead.
    ///
    /// A single lazy yt-dlp process is retained for the whole request. Progress
    /// snapshots are emitted in small batches so the UI can render useful
    /// results while yt-dlp is still walking the search playlist.
    pub(crate) fn ensure_page_with_cancel_and_progress<F>(
        &mut self,
        page: usize,
        cancelled: &AtomicBool,
        mut on_progress: F,
    ) -> Result<usize>
    where
        F: FnMut(&PaginatedSearch),
    {
        self.page_size = clamp_results_per_page(self.page_size);
        let page_size = self.page_size;
        let needed = page.saturating_add(1).saturating_mul(page_size);
        let target = needed.saturating_add(page_size).min(SEARCH_CEILING);

        if cancelled.load(Ordering::Relaxed) {
            bail!("Search cancelled");
        }
        if self.results.len() >= target || self.exhausted {
            return Ok(self.results.len());
        }

        let start = self.raw_cursor.saturating_add(1);
        if start > SEARCH_CEILING {
            self.exhausted = true;
            return Ok(self.results.len());
        }

        let search_id = format!("ytsearch{}:{}", SEARCH_CEILING, self.query);
        let range = format!("{start}:{SEARCH_CEILING}");

        let mut cmd = Command::new("yt-dlp");
        cmd.arg("--flat-playlist")
            .arg("--lazy-playlist")
            .arg("--no-warnings")
            .arg("--extractor-args")
            .arg("youtubetab:approximate_date")
            .arg("--playlist-items")
            .arg(&range)
            .arg(&search_id)
            .arg("--dump-json");

        let started = Instant::now();
        let progress_batch = page_size.clamp(1, 5);
        let mut unreported = 0_usize;
        let mut valid_lines = 0_usize;
        let mut malformed_lines = 0_usize;

        let streamed =
            run_streaming_search_command(cmd, cancelled, started, SEARCH_TIMEOUT, |line| {
                match self.consume_search_line(line) {
                    ConsumedLine::Accepted => {
                        valid_lines += 1;
                        unreported += 1;
                        if unreported >= progress_batch {
                            on_progress(self);
                            unreported = 0;
                        }
                    }
                    ConsumedLine::Ignored => valid_lines += 1,
                    ConsumedLine::Malformed => malformed_lines += 1,
                }
                self.results.len() >= target
            });

        if unreported > 0 {
            on_progress(self);
        }

        let completion = streamed?;
        if malformed_lines > 0
            && valid_lines == 0
            && matches!(&completion.end, CommandEnd::Completed(status) if status.success())
        {
            // A successful exit does not make an unusable output stream a
            // trustworthy end-of-results signal. Leave it retryable.
            self.exhausted = false;
            bail!("yt-dlp returned {malformed_lines} malformed search entries");
        }

        match completion.end {
            CommandEnd::Stopped => {
                self.exhausted = self.raw_cursor >= SEARCH_CEILING;
            }
            CommandEnd::Completed(status) if status.success() => {
                self.exhausted = true;
            }
            CommandEnd::Completed(status) => {
                self.exhausted = false;
                let detail = completion.stderr.trim();
                bail!(
                    "yt-dlp search failed with exit code {}{}{}",
                    status.code().unwrap_or(-1),
                    if detail.is_empty() { "" } else { ": " },
                    detail
                );
            }
            CommandEnd::Cancelled => bail!("Search cancelled"),
            CommandEnd::TimedOut => bail!(
                "yt-dlp search timed out after {} seconds",
                SEARCH_TIMEOUT.as_secs()
            ),
            CommandEnd::PollFailed(error) => {
                return Err(error).context("Failed to monitor yt-dlp search process");
            }
            CommandEnd::ReadFailed(error) => {
                return Err(error).context("Failed to read yt-dlp search output");
            }
            CommandEnd::OutputClosed => bail!("yt-dlp search output closed unexpectedly"),
        }

        Ok(self.results.len())
    }

    fn consume_search_line(&mut self, line: &str) -> ConsumedLine {
        let line = line.trim();
        if line.is_empty() {
            return ConsumedLine::Ignored;
        }

        let fallback_cursor = self.raw_cursor.saturating_add(1).min(SEARCH_CEILING);
        let parsed = match parse_search_entry(line) {
            Ok(parsed) => parsed,
            Err(_) => {
                return ConsumedLine::Malformed;
            }
        };
        let Some(parsed) = parsed else {
            self.raw_cursor = fallback_cursor;
            return ConsumedLine::Ignored;
        };

        self.raw_cursor = parsed
            .playlist_index
            .unwrap_or(fallback_cursor)
            .max(self.raw_cursor)
            .min(SEARCH_CEILING);

        if self.filter_shorts
            && parsed
                .duration_seconds
                .is_some_and(|seconds| seconds < MIN_DURATION as f64)
        {
            return ConsumedLine::Ignored;
        }

        if self
            .results
            .iter()
            .any(|result| result.id == parsed.result.id)
        {
            return ConsumedLine::Ignored;
        }

        self.results.push(parsed.result);
        ConsumedLine::Accepted
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsumedLine {
    Accepted,
    Ignored,
    Malformed,
}

enum CommandEnd {
    Completed(ExitStatus),
    Stopped,
    Cancelled,
    TimedOut,
    PollFailed(std::io::Error),
    ReadFailed(std::io::Error),
    OutputClosed,
}

struct StreamingCompletion {
    end: CommandEnd,
    stderr: String,
}

enum StreamSignal {
    Line(String),
    Eof,
    Failed(std::io::Error),
}

fn run_streaming_search_command<F>(
    mut command: Command,
    cancelled: &AtomicBool,
    started: Instant,
    timeout: Duration,
    mut on_line: F,
) -> Result<StreamingCompletion>
where
    F: FnMut(&str) -> bool,
{
    if cancelled.load(Ordering::Relaxed) {
        bail!("Search cancelled");
    }

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Give the extractor an isolated process group so cancellation also closes
    // pipes inherited by helpers it may spawn.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("Failed to run yt-dlp search")?;
    let stdout = child
        .stdout
        .take()
        .context("Failed to capture yt-dlp search output")?;
    let mut stderr = child
        .stderr
        .take()
        .context("Failed to capture yt-dlp search errors")?;

    let (end, stdout_join, stderr_join) = std::thread::scope(|scope| {
        let (line_tx, line_rx) = mpsc::channel();
        let stdout_reader = scope.spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = line_tx.send(StreamSignal::Eof);
                        break;
                    }
                    Ok(_) => {
                        if line_tx.send(StreamSignal::Line(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = line_tx.send(StreamSignal::Failed(error));
                        break;
                    }
                }
            }
        });
        let stderr_reader = scope.spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });

        let mut exit_status = None;
        let mut stdout_done = false;
        let end = loop {
            if cancelled.load(Ordering::Relaxed) {
                break CommandEnd::Cancelled;
            }
            if started.elapsed() >= timeout {
                break CommandEnd::TimedOut;
            }

            if exit_status.is_none() {
                match child.try_wait() {
                    Ok(Some(status)) => exit_status = Some(status),
                    Ok(None) => {}
                    Err(error) => break CommandEnd::PollFailed(error),
                }
            }

            if stdout_done {
                if let Some(status) = exit_status.take() {
                    break CommandEnd::Completed(status);
                }
                std::thread::sleep(PROCESS_POLL_INTERVAL);
                continue;
            }

            match line_rx.recv_timeout(PROCESS_POLL_INTERVAL) {
                Ok(StreamSignal::Line(line)) => {
                    if on_line(&line) {
                        break CommandEnd::Stopped;
                    }
                }
                Ok(StreamSignal::Eof) => stdout_done = true,
                Ok(StreamSignal::Failed(error)) => break CommandEnd::ReadFailed(error),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break CommandEnd::OutputClosed,
            }
        };

        if !matches!(&end, CommandEnd::Completed(_)) {
            terminate_search_process(&mut child);
        }

        (end, stdout_reader.join(), stderr_reader.join())
    });

    stdout_join.map_err(|_| anyhow::anyhow!("yt-dlp stdout reader panicked"))?;
    let stderr = stderr_join
        .map_err(|_| anyhow::anyhow!("yt-dlp stderr reader panicked"))?
        .context("Failed to read yt-dlp search errors")?;

    Ok(StreamingCompletion {
        end,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

fn terminate_search_process(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(process_group) = i32::try_from(child.id()) {
        // SAFETY: the child was spawned into a fresh group whose ID is its PID;
        // a negative target sends SIGKILL only to that isolated group.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }

    // Keep a direct kill as a portable fallback and always reap the child.
    let _ = child.kill();
    let _ = child.wait();
}

struct ParsedSearchEntry {
    result: SearchResult,
    duration_seconds: Option<f64>,
    playlist_index: Option<usize>,
}

fn parse_search_entry(line: &str) -> serde_json::Result<Option<ParsedSearchEntry>> {
    let entry: Value = serde_json::from_str(line)?;
    let Some(id) = entry.get("id").and_then(Value::as_str) else {
        return Ok(None);
    };

    let title = entry
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled");
    let duration = entry
        .get("duration_string")
        .and_then(Value::as_str)
        .unwrap_or("N/A");
    let channel = entry
        .get("channel")
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let views = entry
        .get("view_count")
        .and_then(Value::as_u64)
        .map(format_view_count)
        .unwrap_or_else(|| "0 views".to_string());
    let published = entry
        .get("timestamp")
        .and_then(Value::as_i64)
        .map(|timestamp| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs() as i64)
                .unwrap_or(timestamp);
            let age = format_relative_age(now.saturating_sub(timestamp));
            match entry
                .get("upload_date")
                .and_then(Value::as_str)
                .and_then(format_upload_date)
            {
                Some(date) => format!("{age} ({date})"),
                None => age,
            }
        })
        .unwrap_or_default();
    let duration_seconds = entry.get("duration").and_then(Value::as_f64);
    let playlist_index = entry
        .get("playlist_index")
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok());

    Ok(
        SearchResult::from_line_parts(title, duration, channel, &views, &published, id).map(|result| {
            ParsedSearchEntry {
                result,
                duration_seconds,
                playlist_index,
            }
        }),
    )
}

/// Format seconds-since-upload as "x hours/days/weeks/months/years ago".
/// yt-dlp's approximate_date is day-granular, so sub-day ages read "1 hour ago" at minimum.
fn format_relative_age(age_seconds: i64) -> String {
    let hours = (age_seconds / 3600).max(1);
    let days = age_seconds / 86_400;
    let (count, unit) = if days >= 365 {
        (days / 365, "year")
    } else if days >= 30 {
        (days / 30, "month")
    } else if days >= 7 {
        (days / 7, "week")
    } else if days >= 1 {
        (days, "day")
    } else {
        (hours, "hour")
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// "20260730" -> "2026-07-30". yt-dlp's approximate date has no real time of
/// day (always midnight UTC), so only the date is shown.
fn format_upload_date(raw: &str) -> Option<String> {
    if raw.len() != 8 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..8]))
}

fn format_view_count(count: u64) -> String {
    let (divisor, suffix) = if count >= 1_000_000_000 {
        (1_000_000_000_u64, "B")
    } else if count >= 1_000_000 {
        (1_000_000_u64, "M")
    } else if count >= 1_000 {
        (1_000_u64, "K")
    } else {
        return format!("{count} views");
    };

    let tenths = ((u128::from(count) * 10 + u128::from(divisor / 2)) / u128::from(divisor)) as u64;
    if tenths.is_multiple_of(10) {
        format!("{}{suffix} views", tenths / 10)
    } else {
        format!("{}.{:01}{suffix} views", tenths / 10, tenths % 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn parses_json_without_confusing_pipes_for_field_delimiters() {
        let line = r#"{
            "title":"One | Two",
            "duration_string":"3:20",
            "channel":"A | B",
            "view_count":1234,
            "id":"correct-id",
            "duration":200.0
        }"#;

        let parsed = parse_search_entry(line).unwrap().unwrap();
        assert_eq!(parsed.result.title, "One | Two");
        assert_eq!(parsed.result.channel, "A | B");
        assert_eq!(parsed.result.views, "1.2K views");
        assert_eq!(parsed.result.id, "correct-id");
        assert_eq!(parsed.duration_seconds, Some(200.0));
    }

    #[test]
    fn json_parser_uses_safe_defaults_for_nullable_metadata() {
        let line = r#"{
            "title":null,
            "duration_string":null,
            "channel":null,
            "view_count":null,
            "id":"video-id",
            "duration":null
        }"#;

        let parsed = parse_search_entry(line).unwrap().unwrap();
        assert_eq!(parsed.result.title, "Untitled");
        assert_eq!(parsed.result.duration, "N/A");
        assert_eq!(parsed.result.channel, "Unknown");
        assert_eq!(parsed.result.views, "0 views");
        assert_eq!(parsed.duration_seconds, None);
    }

    #[test]
    fn json_parser_rejects_an_empty_video_id() {
        let parsed = parse_search_entry(r#"{"title":"No ID","id":"   "}"#).unwrap();
        assert!(parsed.is_none());
    }

    #[test]
    fn pagination_clamps_invalid_sizes_and_avoids_overflow() {
        let mut search = PaginatedSearch::new("test", 0, false);
        assert_eq!(search.page_size, 1);

        search.page_size = usize::MAX;
        search.exhausted = true;
        assert_eq!(search.ensure_page(usize::MAX).unwrap(), 0);
        assert_eq!(search.page_size, crate::config::MAX_RESULTS_PER_PAGE);
    }

    #[test]
    fn pagination_honors_cancellation_before_starting_yt_dlp() {
        let mut search = PaginatedSearch::new("test", 10, false);
        let cancelled = AtomicBool::new(true);

        let error = search.ensure_page_with_cancel(0, &cancelled).unwrap_err();

        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn running_search_process_is_killed_when_cancelled() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            signal.store(true, Ordering::Relaxed);
        });
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = Instant::now();

        let completion = run_streaming_search_command(
            command,
            &cancelled,
            started,
            Duration::from_secs(5),
            |_| false,
        )
        .unwrap();
        canceller.join().unwrap();

        assert!(matches!(completion.end, CommandEnd::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_kills_descendants_that_inherit_search_pipes() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            signal.store(true, Ordering::Relaxed);
        });
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 5 & wait");
        let started = Instant::now();

        let completion = run_streaming_search_command(
            command,
            &cancelled,
            started,
            Duration::from_secs(5),
            |_| false,
        )
        .unwrap();
        canceller.join().unwrap();

        assert!(matches!(completion.end, CommandEnd::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn search_deadline_kills_and_reaps_the_running_process() {
        let cancelled = AtomicBool::new(false);
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = Instant::now();

        let completion = run_streaming_search_command(
            command,
            &cancelled,
            started,
            Duration::from_millis(50),
            |_| false,
        )
        .unwrap();

        assert!(matches!(completion.end, CommandEnd::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn partial_output_survives_a_nonzero_process_exit() {
        let cancelled = AtomicBool::new(false);
        let mut search = PaginatedSearch::new("test", 10, false);
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(r#"printf '%s\n' '{"id":"partial","duration":200}'; exit 7"#);

        let completion = run_streaming_search_command(
            command,
            &cancelled,
            Instant::now(),
            Duration::from_secs(1),
            |line| {
                search.consume_search_line(line);
                false
            },
        )
        .unwrap();

        assert!(matches!(
            completion.end,
            CommandEnd::Completed(status) if !status.success()
        ));
        assert_eq!(search.results.len(), 1);
        assert_eq!(search.results[0].id, "partial");
        assert_eq!(search.raw_cursor, 1);
        assert!(!search.exhausted);
    }

    #[test]
    fn shorts_filter_keeps_unknown_durations_and_skips_known_shorts() {
        let mut search = PaginatedSearch::new("test", 10, true);

        assert_eq!(
            search.consume_search_line(r#"{"id":"unknown","duration":null}"#),
            ConsumedLine::Accepted
        );
        assert_eq!(
            search.consume_search_line(r#"{"id":"short","duration":30}"#),
            ConsumedLine::Ignored
        );
        assert_eq!(
            search.consume_search_line(r#"{"id":"long","duration":180}"#),
            ConsumedLine::Accepted
        );

        assert_eq!(
            search
                .results
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["unknown", "long"]
        );
    }

    #[test]
    fn malformed_entries_do_not_discard_adjacent_valid_results() {
        let mut search = PaginatedSearch::new("test", 10, false);

        assert_eq!(
            search.consume_search_line(r#"{"id":"first","duration":200}"#),
            ConsumedLine::Accepted
        );
        assert_eq!(
            search.consume_search_line("{not-json"),
            ConsumedLine::Malformed
        );
        assert_eq!(
            search.consume_search_line(r#"{"id":"second","duration":200}"#),
            ConsumedLine::Accepted
        );

        assert_eq!(
            search
                .results
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        // The malformed entry has no trustworthy playlist index, so the
        // cursor advances only with the two valid entries.
        assert_eq!(search.raw_cursor, 2);
    }

    #[test]
    fn retried_playlist_entries_are_deduplicated_by_video_id() {
        let mut search = PaginatedSearch::new("test", 10, false);

        assert_eq!(
            search.consume_search_line(r#"{"id":"same","playlist_index":1}"#),
            ConsumedLine::Accepted
        );
        assert_eq!(
            search.consume_search_line(r#"{"id":"same","playlist_index":2}"#),
            ConsumedLine::Ignored
        );

        assert_eq!(search.results.len(), 1);
        assert_eq!(search.raw_cursor, 2);
    }

    #[test]
    fn view_counts_are_compact_and_readable() {
        assert_eq!(
            format_upload_date("20260730"),
            Some("2026-07-30".to_string())
        );
        assert_eq!(format_upload_date("garbage!"), None);
        assert_eq!(format_relative_age(0), "1 hour ago");
        assert_eq!(format_relative_age(3 * 3600), "3 hours ago");
        assert_eq!(format_relative_age(86_400), "1 day ago");
        assert_eq!(format_relative_age(6 * 86_400), "6 days ago");
        assert_eq!(format_relative_age(14 * 86_400), "2 weeks ago");
        assert_eq!(format_relative_age(60 * 86_400), "2 months ago");
        assert_eq!(format_relative_age(800 * 86_400), "2 years ago");
        assert_eq!(format_view_count(999), "999 views");
        assert_eq!(format_view_count(1_000), "1K views");
        assert_eq!(format_view_count(1_250), "1.3K views");
        assert_eq!(format_view_count(2_000_000), "2M views");
        assert_eq!(format_view_count(3_450_000_000), "3.5B views");
    }
}
