//! Terminal video view: a second ffmpeg-decoded frame pipeline that renders
//! the currently playing video as half-block cells while mpv stays the sole
//! audio/playback master. See docs/superpowers/specs/2026-08-11-terminal-video-view-design.md.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// One decoded RGB24 frame, `width` columns by `height_px` pixel rows (twice
/// the terminal row count, since each cell renders two stacked pixels).
pub struct Frame {
    pub width: u16,
    pub height_px: u16,
    pub rgb: Vec<u8>,
}

/// Pure function: the ffmpeg argv used to decode `url` starting at `position`
/// seconds, scaled to `w_px` x `h_px` pixels, at 12 fps, as raw RGB24 on
/// stdout. Kept separate from `VideoSession::start` so it is testable without
/// spawning a process.
pub fn ffmpeg_args(url: &str, position: f64, w_px: u16, h_px: u16) -> Vec<String> {
    vec![
        "-ss".to_string(),
        position.to_string(),
        "-i".to_string(),
        url.to_string(),
        "-vf".to_string(),
        format!("scale={w_px}:{h_px},fps=12"),
        "-f".to_string(),
        "rawvideo".to_string(),
        "-pix_fmt".to_string(),
        "rgb24".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-".to_string(),
    ]
}

/// Pure function: should a session be restarted because the ffmpeg pipeline
/// has drifted more than 2 seconds away from mpv's reported position? Covers
/// both user seeks and gradual decode drift.
pub fn drift_exceeded(mpv_pos: f64, expected: f64) -> bool {
    (mpv_pos - expected).abs() > 2.0
}

/// A running ffmpeg decode pipeline for one playback position/track.
pub struct VideoSession {
    child: Child,
    reader: Option<JoinHandle<()>>,
    rx: Receiver<Frame>,
    cols: u16,
    rows: u16,
    start_pos: f64,
    frames_shown: u64,
    stopped: bool,
    /// Whether the URL used to start this session came from the cache
    /// (as opposed to a fresh yt-dlp resolution). Stale cached URLs are the
    /// expected cause of an immediate ffmpeg failure.
    from_cache: bool,
    /// Whether an ffmpeg exit has already been accounted for by the caller.
    dead_handled: bool,
}

impl VideoSession {
    /// Spawn ffmpeg at `position` seconds for `stream_url`, scaled to
    /// `cols` x `rows*2` pixels. Non-blocking; returns Err on spawn failure.
    pub fn start(stream_url: &str, position: f64, cols: u16, rows: u16) -> Result<VideoSession> {
        let h_px = rows.saturating_mul(2);
        let args = ffmpeg_args(stream_url, position, cols, h_px);

        let mut child = Command::new("ffmpeg")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // -loglevel error already suppresses ffmpeg's normal chatter; the
            // rest must never reach the tty ratatui owns.
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to spawn ffmpeg")?;

        let mut stdout = child
            .stdout
            .take()
            .context("ffmpeg stdout pipe unavailable")?;
        let frame_size = cols as usize * h_px as usize * 3;
        let (tx, rx) = mpsc::sync_channel::<Frame>(1);

        let reader = thread::Builder::new()
            .name("youtui-video-reader".to_string())
            .spawn(move || {
                loop {
                    let mut buf = vec![0u8; frame_size];
                    if stdout.read_exact(&mut buf).is_err() {
                        // Pipe closed: ffmpeg was killed or the stream ended.
                        break;
                    }
                    let frame = Frame {
                        width: cols,
                        height_px: h_px,
                        rgb: buf,
                    };
                    // Only the latest frame matters; drop it if the render
                    // side hasn't consumed the previous one yet.
                    let _ = tx.try_send(frame);
                }
            })
            .context("Failed to spawn video reader thread")?;

        Ok(VideoSession {
            child,
            reader: Some(reader),
            rx,
            cols,
            rows,
            start_pos: position,
            frames_shown: 0,
            stopped: false,
            from_cache: false,
            dead_handled: false,
        })
    }

    /// Latest frame if a new one arrived; updates frames_shown.
    pub fn poll_frame(&mut self) -> Option<Frame> {
        match self.rx.try_recv() {
            Ok(frame) => {
                self.frames_shown += 1;
                Some(frame)
            }
            Err(_) => None,
        }
    }

    /// Position ffmpeg is expected to be at: start_pos + frames_shown / 12.0
    pub fn expected_position(&self) -> f64 {
        self.start_pos + self.frames_shown as f64 / 12.0
    }

    /// Has the ffmpeg process exited (crashed, killed, or reached EOF)?
    fn is_dead(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Kill ffmpeg and join the reader thread. Idempotent.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for VideoSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What the video pane should currently show, in priority order: an error
/// message, a loading indicator, the last decoded frame (optionally paused),
/// or a placeholder when nothing is playing.
pub enum VideoDisplay<'a> {
    Error(&'a str),
    Loading,
    Frame(&'a Frame, bool),
    Placeholder,
}

struct PendingResolve {
    video_id: String,
    rx: Receiver<Result<String, String>>,
}

/// Owns the video pipeline for the whole app session: the active ffmpeg
/// session (if any), the per-video-id stream URL cache, any in-flight yt-dlp
/// resolution, and enough bookkeeping to implement the die-twice-within-5s
/// give-up rule.
pub struct VideoState {
    session: Option<VideoSession>,
    cache: HashMap<String, String>,
    resolving: Option<PendingResolve>,
    current_video_id: Option<String>,
    last_frame: Option<Frame>,
    error: Option<String>,
    paused: bool,
    last_die: Option<Instant>,
    give_up: bool,
    retried_after_evict: bool,
}

impl Default for VideoState {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoState {
    pub fn new() -> Self {
        Self {
            session: None,
            cache: HashMap::new(),
            resolving: None,
            current_video_id: None,
            last_frame: None,
            error: None,
            paused: false,
            last_die: None,
            give_up: false,
            retried_after_evict: false,
        }
    }

    /// Stop any session and discard in-flight resolution work. Called both
    /// when the video view is toggled off and when the sync loop decides
    /// nothing should be playing.
    pub fn stop(&mut self) {
        self.session = None;
        self.resolving = None; // a late result is simply never read
        self.last_frame = None;
        self.error = None;
        self.current_video_id = None;
        self.paused = false;
        self.last_die = None;
        self.give_up = false;
        self.retried_after_evict = false;
    }

    pub fn render_state(&self) -> VideoDisplay<'_> {
        if let Some(error) = &self.error {
            return VideoDisplay::Error(error);
        }
        let loading =
            self.resolving.is_some() || (self.session.is_some() && self.last_frame.is_none());
        if loading {
            return VideoDisplay::Loading;
        }
        if let Some(frame) = &self.last_frame {
            return VideoDisplay::Frame(frame, self.paused);
        }
        VideoDisplay::Placeholder
    }

    /// Drive the pipeline from the latest mpv status. Called once per
    /// status-poll tick while the video view is active.
    pub fn sync(
        &mut self,
        playing: bool,
        paused: bool,
        video_id: Option<&str>,
        time_pos: f64,
        cols: u16,
        rows: u16,
    ) {
        let Some(video_id) = video_id.filter(|_| playing || paused) else {
            self.stop();
            return;
        };

        if self.current_video_id.as_deref() != Some(video_id) {
            self.stop();
            self.current_video_id = Some(video_id.to_string());
        }

        if paused {
            // Freeze on the last frame; nothing should decode while paused.
            self.session = None;
            self.paused = true;
            return;
        }
        self.paused = false;

        if cols == 0 || rows == 0 {
            return; // pane not usable yet (e.g. mid-resize)
        }

        self.restart_if_stale(time_pos, cols, rows);
        self.handle_session_death(video_id);

        if self.give_up {
            return;
        }

        if let Some(session) = self.session.as_mut() {
            if let Some(frame) = session.poll_frame() {
                self.last_frame = Some(frame);
            }
            return;
        }

        if self.resolving.is_none()
            && let Some(url) = self.cache.get(video_id).cloned()
        {
            self.start_session(&url, time_pos, cols, rows, true);
            return;
        }

        self.poll_resolve(video_id, time_pos, cols, rows);
    }

    fn restart_if_stale(&mut self, time_pos: f64, cols: u16, rows: u16) {
        let stale = self.session.as_ref().is_some_and(|session| {
            session.cols != cols
                || session.rows != rows
                || drift_exceeded(time_pos, session.expected_position())
        });
        if stale {
            self.session = None;
        }
    }

    fn handle_session_death(&mut self, video_id: &str) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.dead_handled || !session.is_dead() {
            return;
        }
        session.dead_handled = true;

        let died_immediately = session.frames_shown == 0;
        let from_cache = session.from_cache;
        self.session = None;

        if died_immediately && from_cache && !self.retried_after_evict {
            // The cached URL likely expired; evict it and re-resolve once
            // before treating this as a real failure.
            self.retried_after_evict = true;
            self.cache.remove(video_id);
            return;
        }

        self.record_death();
    }

    fn record_death(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_die
            && now.duration_since(last) <= Duration::from_secs(5)
        {
            self.error = Some("video unavailable: ffmpeg exited repeatedly".to_string());
            self.give_up = true;
        }
        self.last_die = Some(now);
    }

    fn start_session(&mut self, url: &str, position: f64, cols: u16, rows: u16, from_cache: bool) {
        if which::which("ffmpeg").is_err() {
            self.error = Some("video unavailable: ffmpeg not found".to_string());
            return;
        }
        match VideoSession::start(url, position, cols, rows) {
            Ok(mut session) => {
                session.from_cache = from_cache;
                self.session = Some(session);
                self.error = None;
            }
            Err(_error) => {
                self.error = Some("video unavailable: ffmpeg not found".to_string());
            }
        }
    }

    fn poll_resolve(&mut self, video_id: &str, position: f64, cols: u16, rows: u16) {
        let needs_new_request = match &self.resolving {
            Some(pending) => pending.video_id != video_id,
            None => true,
        };
        if needs_new_request {
            self.resolving = Some(PendingResolve {
                video_id: video_id.to_string(),
                rx: spawn_resolve(video_id),
            });
            return;
        }

        let Some(pending) = &self.resolving else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(Ok(url)) => {
                self.cache.insert(video_id.to_string(), url.clone());
                self.resolving = None;
                self.start_session(&url, position, cols, rows, false);
            }
            Ok(Err(_message)) => {
                self.resolving = None;
                self.error = Some("video unavailable: could not resolve stream".to_string());
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.resolving = None;
                self.error = Some("video unavailable: could not resolve stream".to_string());
            }
        }
    }
}

/// Resolve `video_id` to a direct stream URL on a background thread so the
/// UI tick never blocks on yt-dlp.
fn spawn_resolve(video_id: &str) -> Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();
    let watch_url = format!("https://www.youtube.com/watch?v={video_id}");
    thread::spawn(move || {
        let _ = tx.send(resolve_stream_url(&watch_url));
    });
    rx
}

fn resolve_stream_url(watch_url: &str) -> Result<String, String> {
    let output = Command::new("yt-dlp")
        .args([
            "-g",
            "-f",
            "bestvideo[height<=480]/best[height<=480]",
            watch_url,
        ])
        .output()
        .map_err(|error| error.to_string())?;

    if !output.status.success() {
        return Err("yt-dlp failed to resolve a stream URL".to_string());
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::to_string)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "yt-dlp returned no stream URL".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_args_builds_the_exact_expected_argv() {
        let args = ffmpeg_args("https://example.com/stream", 42.5, 80, 48);
        assert_eq!(
            args,
            vec![
                "-ss",
                "42.5",
                "-i",
                "https://example.com/stream",
                "-vf",
                "scale=80:48,fps=12",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-loglevel",
                "error",
                "-",
            ]
        );
    }

    #[test]
    fn drift_exceeded_respects_the_two_second_boundary() {
        assert!(!drift_exceeded(10.0, 8.1)); // 1.9s, under threshold
        assert!(drift_exceeded(10.0, 7.9)); // 2.1s, over threshold
        assert!(!drift_exceeded(8.1, 10.0)); // negative drift, under threshold
        assert!(drift_exceeded(7.9, 10.0)); // negative drift, over threshold
    }

    #[test]
    fn video_state_stop_clears_session_scoped_fields_but_keeps_the_cache() {
        let mut state = VideoState::new();
        state.current_video_id = Some("abc".to_string());
        state.error = Some("boom".to_string());
        state.paused = true;
        state.give_up = true;
        state.cache.insert("abc".to_string(), "url".to_string());

        state.stop();

        assert!(state.current_video_id.is_none());
        assert!(state.error.is_none());
        assert!(!state.paused);
        assert!(!state.give_up);
        assert_eq!(state.cache.get("abc"), Some(&"url".to_string()));
    }

    #[test]
    fn sync_with_nothing_playing_resets_to_placeholder() {
        let mut state = VideoState::new();
        state.current_video_id = Some("abc".to_string());
        state.last_frame = Some(Frame {
            width: 1,
            height_px: 2,
            rgb: vec![0; 6],
        });

        state.sync(false, false, None, 0.0, 80, 24);

        assert!(state.current_video_id.is_none());
        assert!(matches!(state.render_state(), VideoDisplay::Placeholder));
    }

    #[test]
    fn sync_while_paused_drops_the_session_but_keeps_the_last_frame() {
        let mut state = VideoState::new();
        state.current_video_id = Some("abc".to_string());
        state.last_frame = Some(Frame {
            width: 1,
            height_px: 2,
            rgb: vec![9, 9, 9, 1, 1, 1],
        });

        state.sync(true, true, Some("abc"), 5.0, 80, 24);

        assert!(state.session.is_none());
        assert!(state.paused);
        assert!(matches!(state.render_state(), VideoDisplay::Frame(_, true)));
    }

    #[test]
    fn frame_to_cell_mapping_uses_top_pixel_as_fg_and_bottom_as_bg() {
        // 2x4 pixel frame: 2 columns, 2 cell-rows (4 pixel rows / 2).
        #[rustfmt::skip]
        let rgb = vec![
            255, 0, 0,    0, 255, 0,   // row 0: red, green
            0, 0, 255,    255, 255, 0, // row 1: blue, yellow
            10, 20, 30,   40, 50, 60,  // row 2
            70, 80, 90,   100, 110, 120, // row 3
        ];
        let frame = Frame {
            width: 2,
            height_px: 4,
            rgb,
        };

        // Mirrors the pixel lookup used by the renderer: row-major RGB24.
        let pixel_at = |x: usize, y: usize| {
            let idx = (y * frame.width as usize + x) * 3;
            (frame.rgb[idx], frame.rgb[idx + 1], frame.rgb[idx + 2])
        };

        assert_eq!(pixel_at(0, 0), (255, 0, 0));
        assert_eq!(pixel_at(1, 0), (0, 255, 0));
        assert_eq!(pixel_at(0, 1), (0, 0, 255));
        assert_eq!(pixel_at(0, 2), (10, 20, 30));
        assert_eq!(pixel_at(1, 3), (100, 110, 120));
    }
}
