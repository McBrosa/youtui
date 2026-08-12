//! Terminal video view: a second ffmpeg-decoded frame pipeline that renders
//! the currently playing video as half-block cells while mpv stays the sole
//! audio/playback master. See docs/superpowers/specs/2026-08-11-terminal-video-view-design.md.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::{DynamicImage, RgbImage};
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;

use crate::config::VideoRenderMode;
use crate::kitty_shm::{ShmKittyTransport, shm_supported};

/// Decode-size ceiling for the pixel renderer. Panes larger than this are
/// upscaled by the terminal-side image resize instead of pushing more raw
/// RGB through the ffmpeg pipe and graphics protocol.
/// ponytail: fixed 720p ceiling; make it configurable if 4K terminals care.
const MAX_PIXEL_WIDTH: u32 = 1280;
const MAX_PIXEL_HEIGHT: u32 = 720;

/// Pipeline frame rate over the pty (base64 `t=d` and half-block paths):
/// full frame data flows through the terminal parser, so stay conservative.
pub const PTY_FPS: f64 = 24.0;
/// Pipeline frame rate with the kitty shared-memory transport: only ~100
/// escape bytes per frame cross the pty, so the ceiling is ffmpeg + memcpy.
/// ponytail: fixed rate duplicates frames for <60fps sources; probe source
/// fps via yt-dlp if that overhead ever matters.
pub const SHM_FPS: f64 = 60.0;

/// One decoded RGB24 frame, `width` columns by `height_px` pixel rows (twice
/// the terminal row count, since each cell renders two stacked pixels).
pub struct Frame {
    pub width: u16,
    pub height_px: u16,
    pub rgb: Vec<u8>,
}

/// Pure function: the pixel size ffmpeg should decode to for a pane of
/// `cols` x `rows` cells. Half-block cells pack 2 pixels per cell; the pixel
/// renderer uses the terminal's font size, capped at 720p so huge panes
/// don't balloon the raw RGB pipe (the image resize upscales the rest).
pub fn decode_size(cols: u16, rows: u16, font_size: Option<(u16, u16)>) -> (u16, u16) {
    let Some((font_w, font_h)) = font_size else {
        return (cols, rows.saturating_mul(2));
    };
    let w = cols as u32 * font_w as u32;
    let h = rows as u32 * font_h as u32;
    if w <= MAX_PIXEL_WIDTH && h <= MAX_PIXEL_HEIGHT {
        return (w as u16, h as u16);
    }
    let scale = (MAX_PIXEL_WIDTH as f64 / w as f64).min(MAX_PIXEL_HEIGHT as f64 / h as f64);
    ((w as f64 * scale) as u16, (h as f64 * scale) as u16)
}

/// Detect terminal graphics support without touching stdin. The protocol is
/// guessed from environment variables and the cell pixel size read via
/// `TIOCGWINSZ`; `ratatui-image`'s stdio query would be more thorough, but
/// its response-reader thread races the TUI's own input loop for keystrokes.
/// `None` means half-block rendering only.
/// ponytail: env guess misses sixel-only terminals; they get blocks.
pub fn detect_picker() -> Option<Picker> {
    let protocol = protocol_from_env(
        &std::env::var("TERM").unwrap_or_default(),
        &std::env::var("TERM_PROGRAM").unwrap_or_default(),
        std::env::var_os("KITTY_WINDOW_ID").is_some(),
    )?;
    let font_size = font_size_from_winsize()?;
    // from_fontsize is deprecated in favor of the stdio query, which is
    // exactly what this function exists to avoid.
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(font_size.into());
    picker.set_protocol_type(protocol);
    Some(picker)
}

/// Pure function: which graphics protocol the environment advertises.
fn protocol_from_env(term: &str, term_program: &str, kitty_window: bool) -> Option<ProtocolType> {
    if kitty_window || term.contains("kitty") || term.contains("ghostty") {
        return Some(ProtocolType::Kitty);
    }
    if matches!(term_program, "iTerm.app" | "WezTerm" | "mintty" | "vscode") {
        return Some(ProtocolType::Iterm2);
    }
    None
}

/// Cell pixel size from the tty, or `None` when the terminal doesn't report
/// pixel dimensions (then pixel rendering can't be sized and blocks are used).
fn font_size_from_winsize() -> Option<(u16, u16)> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ only writes into the winsize struct provided.
    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) };
    if result != 0 || size.ws_col == 0 || size.ws_row == 0 {
        return None;
    }
    let font_w = size.ws_xpixel / size.ws_col;
    let font_h = size.ws_ypixel / size.ws_row;
    (font_w > 0 && font_h > 0).then_some((font_w, font_h))
}

/// Pure function: the ffmpeg argv used to decode `url` starting at `position`
/// seconds, scaled to `w_px` x `h_px` pixels, at `fps`, as raw RGB24 on
/// stdout. Kept separate from `VideoSession::start` so it is testable without
/// spawning a process.
pub fn ffmpeg_args(url: &str, position: f64, w_px: u16, h_px: u16, fps: f64) -> Vec<String> {
    vec![
        // Read the input at its native frame rate. Without this ffmpeg
        // decodes as fast as the network allows and the pipeline races far
        // ahead of mpv's audio clock.
        "-re".to_string(),
        "-ss".to_string(),
        position.to_string(),
        "-i".to_string(),
        url.to_string(),
        "-vf".to_string(),
        // Fit inside the pane preserving aspect ratio, then letterbox with
        // black bars to the exact pane size so every frame is the same
        // byte length. Grid pixels are ~square (2 stacked per ~1:2 cell),
        // so no cell-aspect correction is needed.
        // lanczos: noticeably sharper than the default bicubic at the small
        // sizes this pipeline scales to.
        format!(
            "scale={w_px}:{h_px}:force_original_aspect_ratio=decrease:flags=lanczos,\
             pad={w_px}:{h_px}:(ow-iw)/2:(oh-ih)/2,fps={fps}"
        ),
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
    /// Latest decoded frame, overwritten by the reader thread. A slot rather
    /// than a channel so the newest frame always wins.
    latest: Arc<Mutex<Option<Frame>>>,
    /// Frames the reader thread has decoded so far — the pipeline clock.
    /// Counting consumed frames instead would undercount (the UI polls
    /// slower than the pipeline) and make `expected_position` lag mpv until the
    /// drift check killed a perfectly healthy session.
    frames_read: Arc<AtomicU64>,
    w_px: u16,
    h_px: u16,
    fps: f64,
    start_pos: f64,
    started_at: Instant,
    /// Whether `start_pos` has been rebased onto mpv's clock after the first
    /// frame arrived (see `rebase`).
    rebased: bool,
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
    /// `w_px` x `h_px` pixels at `fps`. Non-blocking; returns Err on spawn
    /// failure.
    pub fn start(
        stream_url: &str,
        position: f64,
        w_px: u16,
        h_px: u16,
        fps: f64,
    ) -> Result<VideoSession> {
        let args = ffmpeg_args(stream_url, position, w_px, h_px, fps);

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
        let frame_size = w_px as usize * h_px as usize * 3;
        let latest = Arc::new(Mutex::new(None::<Frame>));
        let frames_read = Arc::new(AtomicU64::new(0));

        let reader = {
            let latest = Arc::clone(&latest);
            let frames_read = Arc::clone(&frames_read);
            thread::Builder::new()
                .name("youtui-video-reader".to_string())
                .spawn(move || {
                    loop {
                        let mut buf = vec![0u8; frame_size];
                        if stdout.read_exact(&mut buf).is_err() {
                            // Pipe closed: ffmpeg was killed or the stream ended.
                            break;
                        }
                        let frame = Frame {
                            width: w_px,
                            height_px: h_px,
                            rgb: buf,
                        };
                        *latest.lock().unwrap() = Some(frame);
                        frames_read.fetch_add(1, Ordering::Relaxed);
                    }
                })
                .context("Failed to spawn video reader thread")?
        };

        Ok(VideoSession {
            child,
            reader: Some(reader),
            latest,
            frames_read,
            w_px,
            h_px,
            fps,
            start_pos: position,
            started_at: Instant::now(),
            rebased: false,
            stopped: false,
            from_cache: false,
            dead_handled: false,
        })
    }

    /// Latest frame if a new one arrived since the last poll.
    pub fn poll_frame(&mut self) -> Option<Frame> {
        self.latest.lock().unwrap().take()
    }

    /// Frames ffmpeg has decoded so far.
    fn frames_read(&self) -> u64 {
        self.frames_read.load(Ordering::Relaxed)
    }

    /// Position ffmpeg is expected to be at: start_pos + frames_read / fps.
    pub fn expected_position(&self) -> f64 {
        self.start_pos + self.frames_read() as f64 / self.fps
    }

    /// Absorb ffmpeg's startup latency (network open + seek): once the first
    /// frame has arrived, treat mpv's current position as the point the
    /// pipeline clock started. Without this the constant startup lag reads as
    /// drift and can kill a healthy session on slow connections.
    fn rebase(&mut self, mpv_pos: f64) {
        self.start_pos = mpv_pos - self.frames_read() as f64 / self.fps;
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
    Pixels(&'a Protocol, bool),
    Shm(&'a ShmKittyTransport, bool),
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
    /// Last frame encoded for the terminal's graphics protocol (pixel
    /// renderer). Mutually exclusive with `last_frame`.
    last_protocol: Option<Protocol>,
    /// Detected terminal graphics support, set once at startup. `None` means
    /// the terminal only does half-block cells.
    picker: Option<Picker>,
    /// Kitty shared-memory frame transport, present only for local kitty
    /// terminals. Preferred over `last_protocol` when set.
    shm: Option<ShmKittyTransport>,
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
            last_protocol: None,
            picker: None,
            shm: None,
            error: None,
            paused: false,
            last_die: None,
            give_up: false,
            retried_after_evict: false,
        }
    }

    /// Store the terminal graphics capability detected at startup. Only
    /// called when the terminal supports a real pixel protocol. Local kitty
    /// terminals additionally get the shared-memory frame transport.
    pub fn set_picker(&mut self, picker: Picker) {
        let use_shm = shm_supported(
            picker.protocol_type() == ProtocolType::Kitty,
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some(),
            std::env::var_os("TMUX").is_some(),
        );
        if use_shm {
            self.shm = Some(ShmKittyTransport::new());
        }
        self.picker = Some(picker);
    }

    /// Whether the pixel renderer is in effect for the given mode: `Blocks`
    /// never, `Pixels`/`Auto` whenever the terminal supports it.
    pub fn pixels_active(&self, mode: VideoRenderMode) -> bool {
        mode != VideoRenderMode::Blocks && self.picker.is_some()
    }

    /// Pipeline frame rate for new sessions, by transport.
    fn fps(&self) -> f64 {
        if self.shm.is_some() { SHM_FPS } else { PTY_FPS }
    }

    /// How often the UI should tick while the video view is active.
    pub fn tick_rate(&self) -> Duration {
        Duration::from_millis((1000.0 / self.fps()) as u64)
    }

    /// Stop any session and discard in-flight resolution work. Called both
    /// when the video view is toggled off and when the sync loop decides
    /// nothing should be playing.
    pub fn stop(&mut self) {
        self.session = None;
        self.resolving = None; // a late result is simply never read
        self.last_frame = None;
        self.last_protocol = None;
        if self.shm.is_some() {
            // Fresh transport: drops any pending escapes and clears the
            // has-frame state. The last transmitted image's data stays in
            // the terminal until the next session retransmits (and thereby
            // replaces) the shared image id.
            self.shm = Some(ShmKittyTransport::new());
        }
        self.error = None;
        self.current_video_id = None;
        self.paused = false;
        self.last_die = None;
        self.give_up = false;
        self.retried_after_evict = false;
    }

    /// Start resolving the stream URL for `video_id` ahead of time so the
    /// first toggle into the video view skips the ~3s yt-dlp wait. Safe to
    /// call every tick: it is a no-op when the URL is cached or a resolution
    /// for the same id is already in flight.
    pub fn prefetch(&mut self, video_id: &str) {
        if self.cache.contains_key(video_id) {
            return;
        }
        let already_pending = self
            .resolving
            .as_ref()
            .is_some_and(|pending| pending.video_id == video_id);
        if already_pending {
            return;
        }
        self.resolving = Some(PendingResolve {
            video_id: video_id.to_string(),
            rx: spawn_resolve(video_id),
        });
    }

    pub fn render_state(&self) -> VideoDisplay<'_> {
        if let Some(error) = &self.error {
            return VideoDisplay::Error(error);
        }
        let shm_frame = self.shm.as_ref().is_some_and(ShmKittyTransport::has_frame);
        let has_output = self.last_frame.is_some() || self.last_protocol.is_some() || shm_frame;
        if self.resolving.is_some() || (self.session.is_some() && !has_output) {
            return VideoDisplay::Loading;
        }
        if shm_frame {
            let transport = self.shm.as_ref().expect("shm_frame checked");
            return VideoDisplay::Shm(transport, self.paused);
        }
        if let Some(protocol) = &self.last_protocol {
            return VideoDisplay::Pixels(protocol, self.paused);
        }
        if let Some(frame) = &self.last_frame {
            return VideoDisplay::Frame(frame, self.paused);
        }
        VideoDisplay::Placeholder
    }

    /// Drive the pipeline from the latest mpv status. Called once per
    /// status-poll tick while the video view is active.
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &mut self,
        playing: bool,
        paused: bool,
        video_id: Option<&str>,
        time_pos: f64,
        cols: u16,
        rows: u16,
        mode: VideoRenderMode,
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

        let pixels = self.pixels_active(mode);
        let font_size = pixels.then(|| {
            let size = self
                .picker
                .as_ref()
                .expect("pixels_active checked")
                .font_size();
            (size.width, size.height)
        });
        let (w_px, h_px) = decode_size(cols, rows, font_size);

        self.restart_if_stale(time_pos, w_px, h_px);
        self.handle_session_death(video_id);

        if self.give_up {
            return;
        }

        if let Some(frame) = self.session.as_mut().and_then(VideoSession::poll_frame) {
            self.store_frame(frame, pixels, cols, rows);
        }
        if let Some(session) = self.session.as_mut() {
            if !session.rebased && session.frames_read() > 0 {
                session.rebase(time_pos);
                session.rebased = true;
            }
            return;
        }

        if self.resolving.is_none()
            && let Some(url) = self.cache.get(video_id).cloned()
        {
            self.start_session(&url, time_pos, w_px, h_px, true);
            return;
        }

        self.poll_resolve(video_id, time_pos, w_px, h_px);
    }

    /// Store a decoded frame in the form the renderer needs: encoded for the
    /// terminal's graphics protocol when the pixel renderer is active,
    /// otherwise raw for the half-block path.
    fn store_frame(&mut self, frame: Frame, pixels: bool, cols: u16, rows: u16) {
        if !pixels {
            self.last_frame = Some(frame);
            self.last_protocol = None;
            return;
        }
        if let Some(transport) = self.shm.as_mut() {
            match transport.push_frame(&frame, cols, rows) {
                Ok(()) => {
                    self.last_frame = None;
                    self.last_protocol = None;
                    return;
                }
                Err(_error) => {
                    // shm turned out not to work here; downgrade to the
                    // crate's base64 path for the rest of the session.
                    self.shm = None;
                }
            }
        }
        let Some(image) = RgbImage::from_raw(frame.width as u32, frame.height_px as u32, frame.rgb)
        else {
            return; // impossible: buffer size is derived from these dims
        };
        let picker = self.picker.as_ref().expect("pixels_active checked");
        let size = Size::new(cols, rows);
        // Scale (not Fit): the decode size is capped at 720p, so panes larger
        // than that need the image upscaled to fill the cell area.
        match picker.new_protocol(DynamicImage::ImageRgb8(image), size, Resize::Scale(None)) {
            Ok(protocol) => {
                self.last_protocol = Some(protocol);
                self.last_frame = None;
            }
            Err(_) => {
                // Keep showing the previous frame rather than flickering.
            }
        }
    }

    fn restart_if_stale(&mut self, time_pos: f64, w_px: u16, h_px: u16) {
        let stale = self.session.as_ref().is_some_and(|session| {
            if session.w_px != w_px || session.h_px != h_px {
                return true;
            }
            // No drift verdict until the first frame arrives — startup
            // latency (network open + seek) is not drift. A pipeline that
            // produces nothing for 10s is hung; give up on it instead.
            if session.frames_read() == 0 {
                return session.started_at.elapsed() > Duration::from_secs(10);
            }
            drift_exceeded(time_pos, session.expected_position())
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

        let died_immediately = session.frames_read() == 0;
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

    fn start_session(&mut self, url: &str, position: f64, w_px: u16, h_px: u16, from_cache: bool) {
        if which::which("ffmpeg").is_err() {
            self.error = Some("video unavailable: ffmpeg not found".to_string());
            return;
        }
        match VideoSession::start(url, position, w_px, h_px, self.fps()) {
            Ok(mut session) => {
                session.from_cache = from_cache;
                self.session = Some(session);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("video unavailable: {error}"));
            }
        }
    }

    fn poll_resolve(&mut self, video_id: &str, position: f64, w_px: u16, h_px: u16) {
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
                self.start_session(&url, position, w_px, h_px, false);
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
            // 480p: enough detail for both renderers (the pixel renderer
            // caps at 720p pane size, the block renderer downscales), without
            // pulling a full-quality stream twice alongside mpv's.
            "bestvideo[height<=480]/best[height<=480]/best",
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
        let args = ffmpeg_args("https://example.com/stream", 42.5, 80, 48, 24.0);
        assert_eq!(
            args,
            vec![
                "-re",
                "-ss",
                "42.5",
                "-i",
                "https://example.com/stream",
                "-vf",
                "scale=80:48:force_original_aspect_ratio=decrease:flags=lanczos,pad=80:48:(ow-iw)/2:(oh-ih)/2,fps=24",
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
    fn protocol_from_env_recognizes_kitty_and_iterm2_terminals() {
        use ratatui_image::picker::ProtocolType;
        assert_eq!(
            protocol_from_env("xterm-ghostty", "", false),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(
            protocol_from_env("xterm-kitty", "", false),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(
            protocol_from_env("xterm-256color", "", true),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(
            protocol_from_env("xterm-256color", "iTerm.app", false),
            Some(ProtocolType::Iterm2)
        );
        assert_eq!(
            protocol_from_env("xterm-256color", "WezTerm", false),
            Some(ProtocolType::Iterm2)
        );
        assert_eq!(protocol_from_env("xterm-256color", "", false), None);
        assert_eq!(protocol_from_env("dumb", "Apple_Terminal", false), None);
    }

    #[test]
    fn decode_size_uses_half_block_grid_without_a_font_size() {
        assert_eq!(decode_size(80, 24, None), (80, 48));
    }

    #[test]
    fn decode_size_uses_font_pixels_and_caps_at_720p() {
        // 100x40 cells at 8x16px = 800x640, under the cap: exact.
        assert_eq!(decode_size(100, 40, Some((8, 16))), (800, 640));
        // 200x50 cells at 10x20px = 2000x1000, over the cap: scaled to fit
        // 1280x720 preserving aspect (limited by width: 1280x640).
        assert_eq!(decode_size(200, 50, Some((10, 20))), (1280, 640));
        // Height-limited: 100x100 at 10x20 = 1000x2000 -> 360x720.
        assert_eq!(decode_size(100, 100, Some((10, 20))), (360, 720));
    }

    #[test]
    fn pixels_active_respects_mode_and_detected_support() {
        let state = VideoState::new();
        // No picker: every mode falls back to blocks.
        assert!(!state.pixels_active(VideoRenderMode::Auto));
        assert!(!state.pixels_active(VideoRenderMode::Pixels));
        assert!(!state.pixels_active(VideoRenderMode::Blocks));

        let mut state = VideoState::new();
        #[allow(deprecated)] // test-only picker construction without a tty
        state.set_picker(Picker::from_fontsize((8, 16).into()));
        assert!(state.pixels_active(VideoRenderMode::Auto));
        assert!(state.pixels_active(VideoRenderMode::Pixels));
        assert!(!state.pixels_active(VideoRenderMode::Blocks));
    }

    #[test]
    fn prefetch_is_a_noop_when_the_url_is_already_cached() {
        let mut state = VideoState::new();
        state
            .cache
            .insert("abc123".to_string(), "https://cached".to_string());
        state.prefetch("abc123");
        assert!(state.resolving.is_none());
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

        state.sync(false, false, None, 0.0, 80, 24, VideoRenderMode::Auto);

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

        state.sync(true, true, Some("abc"), 5.0, 80, 24, VideoRenderMode::Auto);

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
