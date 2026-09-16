//! Terminal video view. Video uses an independent yt-dlp/ffmpeg pipeline so
//! failures and startup latency never affect the long-lived audio player. See
//! docs/superpowers/specs/2026-08-11-terminal-video-view-design.md.

use std::collections::HashMap;
use std::fs::File;
use std::io::{IsTerminal, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::{DynamicImage, RgbImage};
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;

use crate::config::{PixelVideoQuality, VideoRenderMode};
use crate::kitty_shm::{KittyTransport, shm_supported};

/// Decode-size ceiling for the pixel renderer. Panes larger than this are
/// upscaled by the terminal-side image resize instead of pushing more raw
/// RGB through the ffmpeg pipe and graphics protocol.
/// ponytail: fixed 720p ceiling; make it configurable if 4K terminals care.
const MAX_PIXEL_WIDTH: u32 = 1280;
const MAX_PIXEL_HEIGHT: u32 = 720;
const VIDEO_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_RESOLVE_TIMEOUT: Duration = Duration::from_secs(15);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);
// mpv's output is captured so youtui can serialize Kitty frames with Ratatui.
// Drain at slightly over 60Hz for SHM sources; the bounded direct fallback is
// itself capped at 24fps.
const MPV_KITTY_TICK_RATE: Duration = Duration::from_millis(16);

static MPV_KITTY_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Pipeline frame rate over the pty (base64 `t=d` and half-block paths):
/// full frame data flows through the terminal parser, so stay conservative.
pub const PTY_FPS: f64 = 24.0;
/// Pipeline frame rate with the kitty shared-memory transport: only ~100
/// escape bytes per frame cross the pty, so the ceiling is ffmpeg + memcpy.
/// ponytail: fixed rate duplicates frames for <60fps sources; probe source
/// fps via yt-dlp if that overhead ever matters.
pub const SHM_FPS: f64 = 60.0;

/// Cell and pixel dimensions passed to mpv's Kitty video output. The pane is
/// always at the top-left of youtui's alternate screen; mpv centers the video
/// inside these bounds while preserving its aspect ratio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpvKittyGeometry {
    pub cols: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
}

/// One decoded RGB24 frame, `width` columns by `height_px` pixel rows (twice
/// the terminal row count, since each cell renders two stacked pixels).
pub struct Frame {
    pub width: u16,
    pub height_px: u16,
    pub rgb: Vec<u8>,
}

/// Single-producer/single-consumer exchange for decoded frames. Keeping one
/// spare RGB allocation lets the reader reuse buffers once the UI has copied
/// or replaced a frame, avoiding a full allocate-and-zero cycle per frame.
#[derive(Default)]
struct FrameExchange {
    latest: Option<Frame>,
    spare: Option<Vec<u8>>,
}

impl FrameExchange {
    fn publish(&mut self, frame: Frame) -> Option<Vec<u8>> {
        self.latest
            .replace(frame)
            .map(|replaced| replaced.rgb)
            .or_else(|| self.spare.take())
    }

    fn take_latest(&mut self) -> Option<Frame> {
        self.latest.take()
    }

    fn recycle(&mut self, buffer: Vec<u8>, expected_len: usize) {
        if buffer.len() == expected_len && self.spare.is_none() {
            self.spare = Some(buffer);
        }
    }
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
    if kitty_window || term.contains("kitty") || is_ghostty(term, term_program) {
        return Some(ProtocolType::Kitty);
    }
    if matches!(term_program, "iTerm.app" | "WezTerm" | "mintty" | "vscode") {
        return Some(ProtocolType::Iterm2);
    }
    None
}

fn is_ghostty(term: &str, term_program: &str) -> bool {
    term.to_ascii_lowercase().contains("ghostty")
        || term_program.to_ascii_lowercase().contains("ghostty")
}

/// Ghostty 1.3 on macOS supports direct Kitty image packets but rejects the
/// POSIX shared-memory (`t=s`) medium. Keep mpv's native renderer and select a
/// bounded direct transport for this terminal instead.
pub fn ghostty_terminal() -> bool {
    is_ghostty(
        &std::env::var("TERM").unwrap_or_default(),
        &std::env::var("TERM_PROGRAM").unwrap_or_default(),
    )
}

/// Whether the current stdout is a local Kitty-protocol terminal. Native mpv
/// graphics cannot cross SSH and their escape sequences must not be swallowed
/// by a multiplexer that youtui does not explicitly coordinate.
pub fn local_kitty_terminal() -> bool {
    if !std::io::stdout().is_terminal() {
        return false;
    }
    let kitty = protocol_from_env(
        &std::env::var("TERM").unwrap_or_default(),
        &std::env::var("TERM_PROGRAM").unwrap_or_default(),
        std::env::var_os("KITTY_WINDOW_ID").is_some(),
    ) == Some(ProtocolType::Kitty);
    let remote =
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
    let multiplexer = std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some();
    shm_supported(kitty, remote, multiplexer)
}

/// Probe once for mpv's built-in `vo=kitty` driver (added in mpv 0.36). Old
/// mpv builds continue through youtui's existing ffmpeg renderer.
pub fn mpv_kitty_available() -> bool {
    *MPV_KITTY_AVAILABLE.get_or_init(|| {
        let Ok(output) = Command::new("mpv")
            .args(["--no-config", "--vo=help"])
            .stdin(Stdio::null())
            .output()
        else {
            return false;
        };
        output.status.success() && output_has_kitty_vo(&output.stdout, &output.stderr)
    })
}

fn output_has_kitty_vo(stdout: &[u8], stderr: &[u8]) -> bool {
    [stdout, stderr].into_iter().any(|bytes| {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim_start)
            .any(|line| line == "kitty" || line.starts_with("kitty "))
    })
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
pub fn ffmpeg_args(
    url: &str,
    http_headers: &str,
    position: f64,
    w_px: u16,
    h_px: u16,
    fps: f64,
) -> Vec<String> {
    let mut args = Vec::new();
    if !http_headers.is_empty() {
        // Direct Googlevideo URLs are tied to the client identity yt-dlp used
        // to resolve them. In particular, omitting its User-Agent can turn an
        // otherwise valid URL into a 403 response in ffmpeg.
        args.extend(["-headers".to_string(), http_headers.to_string()]);
    }
    args.extend([
        // Do not use ffmpeg's input-side -re/readrate here. When a source
        // cannot seek efficiently, it makes -ss decode/discard in real time,
        // so opening video at 2:00 can literally take two minutes. The reader
        // thread below applies backpressure after the first frame instead.
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
    ]);
    args
}

/// Pure function: should a session be restarted because the ffmpeg pipeline
/// has drifted more than 2 seconds away from mpv's reported position? Covers
/// both user seeks and gradual decode drift.
pub fn drift_exceeded(mpv_pos: f64, expected: f64) -> bool {
    (mpv_pos - expected).abs() > 2.0
}

fn startup_timed_out(frames_read: u64, elapsed: Duration) -> bool {
    frames_read == 0 && elapsed > VIDEO_STARTUP_TIMEOUT
}

/// A running ffmpeg decode pipeline for one playback position/track.
pub struct VideoSession {
    child: Child,
    reader: Option<JoinHandle<()>>,
    /// Latest decoded frame, overwritten by the reader thread. A slot rather
    /// than a channel so the newest frame always wins.
    exchange: Arc<Mutex<FrameExchange>>,
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
        http_headers: &str,
        position: f64,
        w_px: u16,
        h_px: u16,
        fps: f64,
    ) -> Result<Self> {
        let args = ffmpeg_args(stream_url, http_headers, position, w_px, h_px, fps);

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
        let frame_interval = Duration::from_secs_f64(1.0 / fps);
        let exchange = Arc::new(Mutex::new(FrameExchange::default()));
        let frames_read = Arc::new(AtomicU64::new(0));

        let reader = {
            let exchange = Arc::clone(&exchange);
            let frames_read = Arc::clone(&frames_read);
            thread::Builder::new()
                .name("youtui-video-reader".to_string())
                .spawn(move || {
                    let mut next_frame_at = Instant::now();
                    let mut buf = vec![0u8; frame_size];
                    loop {
                        if stdout.read_exact(&mut buf).is_err() {
                            // Pipe closed: ffmpeg was killed or the stream ended.
                            break;
                        }
                        let frame = Frame {
                            width: w_px,
                            height_px: h_px,
                            rgb: buf,
                        };
                        let recycled = exchange.lock().unwrap().publish(frame);
                        buf = recycled.unwrap_or_else(|| vec![0u8; frame_size]);
                        frames_read.fetch_add(1, Ordering::Relaxed);

                        // Pace only after a complete first frame is available.
                        // Sleeping here fills ffmpeg's stdout pipe and naturally
                        // backpressures the decoder without slowing input seek.
                        next_frame_at += frame_interval;
                        let now = Instant::now();
                        if next_frame_at > now {
                            thread::sleep(next_frame_at - now);
                        } else {
                            next_frame_at = now;
                        }
                    }
                })
                .context("Failed to spawn video reader thread")?
        };

        Ok(Self {
            child,
            reader: Some(reader),
            exchange,
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
        self.exchange.lock().unwrap().take_latest()
    }

    /// Return a consumed buffer to the reader when it matches this session's
    /// frame size. A resize can leave an older raw frame in the UI, so reject
    /// mismatched allocations instead of handing them to `read_exact`.
    fn recycle_buffer(&self, buffer: Vec<u8>) {
        let expected_len = usize::from(self.w_px) * usize::from(self.h_px) * 3;
        self.exchange.lock().unwrap().recycle(buffer, expected_len);
    }

    /// Frames ffmpeg has decoded so far.
    fn frames_read(&self) -> u64 {
        self.frames_read.load(Ordering::Relaxed)
    }

    /// Position ffmpeg is expected to be at: `start_pos + frames_read / fps`.
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
    Kitty(&'a KittyTransport, bool),
    Placeholder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResolvedStream {
    url: String,
    /// CRLF-delimited HTTP request headers in ffmpeg's `-headers` format.
    http_headers: String,
    quality: PixelVideoQuality,
}

struct PendingResolve {
    video_id: String,
    quality: PixelVideoQuality,
    rx: Receiver<Result<ResolvedStream, String>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for PendingResolve {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct ResolveProcess(Child);

impl Drop for ResolveProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Ok(process_group) = i32::try_from(self.0.id()) {
            // SAFETY: this child was spawned in its own process group. A negative
            // PID targets that group, including packaged yt-dlp descendants.
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Owns the video pipeline for the whole app session: the active ffmpeg
/// session (if any), the per-video-id stream URL cache, any in-flight yt-dlp
/// resolution, and enough bookkeeping to implement the die-twice-within-5s
/// give-up rule.
pub struct VideoState {
    session: Option<VideoSession>,
    cache: HashMap<String, ResolvedStream>,
    resolving: Option<PendingResolve>,
    current_video_id: Option<String>,
    current_quality: Option<PixelVideoQuality>,
    last_frame: Option<Frame>,
    /// Last frame encoded for the terminal's graphics protocol (pixel
    /// renderer). Mutually exclusive with `last_frame`.
    last_protocol: Option<Protocol>,
    /// Detected terminal graphics support, set once at startup. `None` means
    /// the terminal only does half-block cells.
    picker: Option<Picker>,
    /// Kitty frame transport with cell-based scaling. Shared memory is used
    /// locally when supported; other Kitty terminals receive RGB chunks.
    kitty_transport: Option<KittyTransport>,
    /// mpv's native Kitty output is usable on this exact terminal. Kept as a
    /// stored capability so rendering never spawns a process or reads env.
    mpv_kitty: bool,
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
            current_quality: None,
            last_frame: None,
            last_protocol: None,
            picker: None,
            kitty_transport: None,
            mpv_kitty: false,
            error: None,
            paused: false,
            last_die: None,
            give_up: false,
            retried_after_evict: false,
        }
    }

    /// Store the terminal graphics capability detected at startup. Only
    /// called when the terminal supports a real pixel protocol. Local Kitty
    /// terminals use shared memory when the implementation accepts it.
    pub fn set_picker(&mut self, picker: Picker) {
        let use_shm = shm_supported(
            picker.protocol_type() == ProtocolType::Kitty,
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some(),
            std::env::var_os("TMUX").is_some(),
        ) && !ghostty_terminal();
        self.kitty_transport = (picker.protocol_type() == ProtocolType::Kitty
            && std::env::var_os("TMUX").is_none())
        .then(|| KittyTransport::new(use_shm));
        self.mpv_kitty = picker.protocol_type() == ProtocolType::Kitty
            && local_kitty_terminal()
            && mpv_kitty_available();
        self.picker = Some(picker);
    }

    /// Whether mpv should own pixel rendering for this terminal and selected
    /// mode. `Blocks` deliberately keeps youtui's portable ffmpeg renderer.
    pub fn mpv_kitty_active(&self, mode: VideoRenderMode) -> bool {
        mode != VideoRenderMode::Blocks && self.mpv_kitty
    }

    pub fn mpv_kitty_geometry(&self, cols: u16, rows: u16) -> Option<MpvKittyGeometry> {
        let picker = self.picker.as_ref()?;
        (picker.protocol_type() == ProtocolType::Kitty).then(|| {
            let font = picker.font_size();
            MpvKittyGeometry {
                cols,
                rows,
                width_px: u32::from(cols) * u32::from(font.width),
                height_px: u32::from(rows) * u32::from(font.height),
            }
        })
    }

    /// Whether the pixel renderer is in effect for the given mode: `Blocks`
    /// never, `Pixels`/`Auto` whenever the terminal supports it.
    pub fn pixels_active(&self, mode: VideoRenderMode) -> bool {
        mode != VideoRenderMode::Blocks && self.picker.is_some()
    }

    /// Pipeline frame rate for new sessions, by the transport the selected
    /// render mode will actually use. Merely detecting Kitty shm must not make
    /// an explicitly selected half-block session decode and repaint at 60fps.
    fn fps(&self, mode: VideoRenderMode) -> f64 {
        if self.pixels_active(mode)
            && self
                .kitty_transport
                .as_ref()
                .is_some_and(KittyTransport::uses_shared_memory)
        {
            SHM_FPS
        } else {
            PTY_FPS
        }
    }

    /// How often the UI should tick while the video view is active.
    pub fn tick_rate(&self, mode: VideoRenderMode, native_mpv: bool) -> Duration {
        if native_mpv {
            MPV_KITTY_TICK_RATE
        } else {
            Duration::from_secs_f64(1.0 / self.fps(mode))
        }
    }

    /// Stop any session and discard in-flight resolution work. Called both
    /// when the video view is toggled off and when the sync loop decides
    /// nothing should be playing.
    pub fn stop(&mut self) {
        self.session = None;
        self.resolving = None; // cancel and join the resolver before starting another
        self.last_frame = None;
        self.last_protocol = None;
        if let Some(transport) = self.kitty_transport.as_mut() {
            transport.reset();
        }
        self.error = None;
        self.current_video_id = None;
        self.current_quality = None;
        self.paused = false;
        self.last_die = None;
        self.give_up = false;
        self.retried_after_evict = false;
    }

    /// Start at most one resolver for the requested stream while video is visible.
    fn begin_resolve(&mut self, video_id: &str, quality: PixelVideoQuality) {
        if self
            .cache
            .get(video_id)
            .is_some_and(|stream| stream.quality == quality)
            || self
                .resolving
                .as_ref()
                .is_some_and(|pending| pending.video_id == video_id && pending.quality == quality)
        {
            return;
        }
        self.resolving = None;
        self.resolving = Some(spawn_resolve(video_id, quality));
    }

    /// Reset per-track rendering state while retaining a matching background
    /// resolution. The old implementation called `stop()` directly here,
    /// discarding the very prefetch result meant to make this transition fast.
    fn switch_track(&mut self, video_id: &str, quality: PixelVideoQuality) {
        let matching_resolve = self
            .resolving
            .take()
            .filter(|pending| pending.video_id == video_id && pending.quality == quality);
        self.stop();
        self.resolving = matching_resolve;
        self.current_video_id = Some(video_id.to_string());
        self.current_quality = Some(quality);
    }

    pub fn render_state(&self) -> VideoDisplay<'_> {
        if let Some(error) = &self.error {
            return VideoDisplay::Error(error);
        }
        let has_output = self.has_output();
        if self.resolving.is_some() || (self.session.is_some() && !has_output) {
            return VideoDisplay::Loading;
        }
        if let Some(transport) = self
            .kitty_transport
            .as_ref()
            .filter(|transport| transport.has_frame())
        {
            return VideoDisplay::Kitty(transport, self.paused);
        }
        if let Some(protocol) = &self.last_protocol {
            return VideoDisplay::Pixels(protocol, self.paused);
        }
        if let Some(frame) = &self.last_frame {
            return VideoDisplay::Frame(frame, self.paused);
        }
        VideoDisplay::Placeholder
    }

    /// Drive the pipeline from the latest mpv status snapshot. Called once per
    /// video-delivery tick while the video view is active.
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
        quality: PixelVideoQuality,
    ) {
        let Some(video_id) = video_id.filter(|_| playing || paused) else {
            self.stop();
            return;
        };

        if self.current_video_id.as_deref() != Some(video_id)
            || self.current_quality != Some(quality)
        {
            self.switch_track(video_id, quality);
        }

        if paused && self.has_output() {
            // Freeze on the last frame; nothing should decode while paused.
            self.session = None;
            self.paused = true;
            return;
        }
        // A pause before the first frame must still drive URL resolution and
        // decode one frame. Otherwise the early return above leaves a completed
        // prefetch unread forever and the pane stays on "loading video…".
        self.paused = paused;

        if cols == 0 || rows == 0 {
            return; // pane not usable yet (e.g. mid-resize)
        }

        let pixels = self.pixels_active(mode);
        let fps = self.fps(mode);
        let font_size = self.picker.as_ref().filter(|_| pixels).map(|picker| {
            let size = picker.font_size();
            (size.width, size.height)
        });
        let (w_px, h_px) = decode_size(cols, rows, font_size);

        if self.restart_if_stale(time_pos, w_px, h_px, fps) {
            // Never leave the pane saying "loading" forever. A fresh manual
            // toggle may retry, but it must resolve a new URL first.
            self.cache.remove(video_id);
            self.error = Some("video unavailable: stream startup timed out".to_string());
            self.give_up = true;
            return;
        }
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
            && let Some(stream) = self
                .cache
                .get(video_id)
                .filter(|stream| stream.quality == quality)
                .cloned()
        {
            self.start_session(&stream, time_pos, w_px, h_px, fps, true);
            return;
        }

        self.poll_resolve(video_id, quality, time_pos, w_px, h_px, fps);
    }

    /// Store a decoded frame in the form the renderer needs: encoded for the
    /// terminal's graphics protocol when the pixel renderer is active,
    /// otherwise raw for the half-block path.
    fn store_frame(&mut self, frame: Frame, pixels: bool, cols: u16, rows: u16) {
        if !pixels {
            let replaced = self.last_frame.replace(frame);
            self.last_protocol = None;
            if let Some(replaced) = replaced
                && let Some(session) = &self.session
            {
                session.recycle_buffer(replaced.rgb);
            }
            return;
        }
        if let Some(transport) = self.kitty_transport.as_mut() {
            match transport.push_frame(&frame, cols, rows) {
                Ok(()) => {
                    self.last_frame = None;
                    self.last_protocol = None;
                    if let Some(session) = &self.session {
                        session.recycle_buffer(frame.rgb);
                    }
                    return;
                }
                Err(_error) => {
                    // Keep explicit cell placement when shared memory fails.
                    let mut direct = KittyTransport::new(false);
                    if direct.push_frame(&frame, cols, rows).is_ok() {
                        self.kitty_transport = Some(direct);
                        self.last_frame = None;
                        self.last_protocol = None;
                        if let Some(session) = &self.session {
                            session.recycle_buffer(frame.rgb);
                        }
                        return;
                    }
                }
            }
        }
        let Some(picker) = self.picker.as_ref() else {
            let replaced = self.last_frame.replace(frame);
            self.last_protocol = None;
            if let Some(replaced) = replaced
                && let Some(session) = &self.session
            {
                session.recycle_buffer(replaced.rgb);
            }
            return;
        };
        let Some(image) = RgbImage::from_raw(
            u32::from(frame.width),
            u32::from(frame.height_px),
            frame.rgb,
        ) else {
            return; // impossible: buffer size is derived from these dims
        };
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

    fn has_output(&self) -> bool {
        self.last_frame.is_some()
            || self.last_protocol.is_some()
            || self
                .kitty_transport
                .as_ref()
                .is_some_and(KittyTransport::has_frame)
    }

    /// Restart for ordinary drift/resize and report whether startup timed out.
    fn restart_if_stale(&mut self, time_pos: f64, w_px: u16, h_px: u16, fps: f64) -> bool {
        let (stale, timed_out) = self.session.as_ref().map_or((false, false), |session| {
            if session.w_px != w_px || session.h_px != h_px || session.fps != fps {
                return (true, false);
            }
            // No drift verdict until the first frame arrives — startup
            // latency (network open + seek) is not drift.
            if session.frames_read() == 0 {
                let timed_out = startup_timed_out(0, session.started_at.elapsed());
                return (timed_out, timed_out);
            }
            (drift_exceeded(time_pos, session.expected_position()), false)
        });
        if stale {
            self.session = None;
        }
        timed_out
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

    fn start_session(
        &mut self,
        stream: &ResolvedStream,
        position: f64,
        w_px: u16,
        h_px: u16,
        fps: f64,
        from_cache: bool,
    ) {
        if which::which("ffmpeg").is_err() {
            self.error = Some("video unavailable: ffmpeg not found".to_string());
            return;
        }
        match VideoSession::start(&stream.url, &stream.http_headers, position, w_px, h_px, fps) {
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

    fn poll_resolve(
        &mut self,
        video_id: &str,
        quality: PixelVideoQuality,
        position: f64,
        w_px: u16,
        h_px: u16,
        fps: f64,
    ) {
        let needs_new_request = match &self.resolving {
            Some(pending) => pending.video_id != video_id || pending.quality != quality,
            None => true,
        };
        if needs_new_request {
            self.begin_resolve(video_id, quality);
            return;
        }

        let Some(pending) = &self.resolving else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(Ok(stream)) => {
                self.cache.insert(video_id.to_string(), stream.clone());
                self.resolving = None;
                self.start_session(&stream, position, w_px, h_px, fps, false);
            }
            Ok(Err(reason)) => {
                self.resolving = None;
                self.error = Some(format!("video unavailable: {reason}"));
                self.give_up = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.resolving = None;
                self.error = Some("video unavailable: resolver stopped".to_string());
                self.give_up = true;
            }
        }
    }
}

/// Resolve a video stream on a cancellable worker owned by the video pane.
fn spawn_resolve(video_id: &str, quality: PixelVideoQuality) -> PendingResolve {
    let (tx, rx) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let watch_url = format!("https://www.youtube.com/watch?v={video_id}");
    let worker = thread::Builder::new()
        .name("youtui-video-resolver".to_string())
        .spawn(move || {
            let _ = tx.send(resolve_stream_url(&watch_url, quality, &worker_cancelled));
        })
        .ok();
    PendingResolve {
        video_id: video_id.to_string(),
        quality,
        rx,
        cancelled,
        worker,
    }
}

fn stream_format(quality: PixelVideoQuality) -> String {
    let height = quality.height();
    // Prefer H.264 at the requested height: terminal playback values low
    // first-frame latency and cheap software decoding over AV1's bandwidth
    // efficiency. Retain the codec-agnostic and combined-stream fallbacks for
    // videos that do not expose an AVC rendition.
    format!(
        "bestvideo[height<={height}][vcodec^=avc1]/bestvideo[height<={height}]/\
         best[height<={height}]/best"
    )
}

fn resolve_stream_url(
    watch_url: &str,
    quality: PixelVideoQuality,
    cancelled: &AtomicBool,
) -> Result<ResolvedStream, String> {
    let format = stream_format(quality);
    let mut command = Command::new("yt-dlp");
    command.args([
        "--no-playlist",
        "--skip-download",
        "-f",
        &format,
        "--print",
        "%(url)s",
        "--print",
        "%(http_headers)j",
        "--",
        watch_url,
    ]);
    let stdout = run_resolve_command(command, cancelled, STREAM_RESOLVE_TIMEOUT)?;
    parse_resolve_output(&stdout, quality)
}

fn run_resolve_command(
    mut command: Command,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("video resolution cancelled".to_string());
    }
    // Packaged yt-dlp unpacks Python into TMPDIR. Own that directory so even
    // forced termination removes its extraction files.
    let temp_dir = tempfile::Builder::new()
        .prefix("youtui-resolve-")
        .tempdir()
        .map_err(|error| format!("cannot create video resolver directory: {error}"))?;
    let stdout_path = temp_dir.path().join("stdout");
    let stderr_path = temp_dir.path().join("stderr");
    let stdout = File::create(&stdout_path).map_err(|error| error.to_string())?;
    let stderr = File::create(&stderr_path).map_err(|error| error.to_string())?;
    command
        .env("TMPDIR", temp_dir.path())
        .env("TMP", temp_dir.path())
        .env("TEMP", temp_dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    command.process_group(0);

    // Declared after temp_dir: kill the whole group before removing its files.
    let mut process = ResolveProcess(command.spawn().map_err(|error| error.to_string())?);
    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("video resolution cancelled".to_string());
        }
        if started.elapsed() >= timeout {
            return Err("yt-dlp timed out resolving the video stream".to_string());
        }
        if let Some(status) = process.0.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    };
    if !status.success() {
        let mut detail = String::new();
        File::open(stderr_path)
            .map_err(|error| error.to_string())?
            .take(4096)
            .read_to_string(&mut detail)
            .map_err(|error| error.to_string())?;
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            format!("yt-dlp failed to resolve a stream URL ({status})")
        } else {
            format!("yt-dlp failed: {detail}")
        });
    }
    // Files avoid pipe backpressure while waiting for the extractor to exit.
    let mut stdout = Vec::new();
    File::open(stdout_path)
        .map_err(|error| error.to_string())?
        .take(128 * 1024)
        .read_to_end(&mut stdout)
        .map_err(|error| error.to_string())?;
    Ok(stdout)
}

fn parse_resolve_output(
    stdout: &[u8],
    quality: PixelVideoQuality,
) -> Result<ResolvedStream, String> {
    let output = String::from_utf8_lossy(stdout);
    let mut lines = output.lines();
    let url = lines
        .next()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "yt-dlp returned no stream URL".to_string())?;

    let mut headers = lines
        .next()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|value| value.as_object().cloned())
        .map(|headers| {
            headers
                .into_iter()
                .filter_map(|(name, value)| {
                    let value = value.as_str()?;
                    safe_http_header(&name, value).then_some((name, value.to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    headers.sort_unstable_by(|(left_name, _), (right_name, _)| left_name.cmp(right_name));
    let mut http_headers = String::new();
    for (name, value) in headers {
        http_headers.push_str(&name);
        http_headers.push_str(": ");
        http_headers.push_str(&value);
        http_headers.push_str("\r\n");
    }

    Ok(ResolvedStream {
        url: url.to_string(),
        http_headers,
        quality,
    })
}

fn safe_http_header(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !value.contains(['\r', '\n', '\0'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_args_builds_the_exact_expected_argv() {
        let args = ffmpeg_args(
            "https://example.com/stream",
            "User-Agent: yt-dlp-agent\r\n",
            42.5,
            80,
            48,
            24.0,
        );
        assert_eq!(
            args,
            vec![
                "-headers",
                "User-Agent: yt-dlp-agent\r\n",
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
    fn ffmpeg_args_omit_empty_http_headers_and_input_throttling() {
        let args = ffmpeg_args("https://example.com/stream", "", 0.0, 80, 48, 24.0);

        assert!(!args.iter().any(|arg| arg == "-headers"));
        assert!(!args.iter().any(|arg| arg == "-re" || arg == "-readrate"));
    }

    #[test]
    fn resolve_output_carries_safe_http_headers_to_ffmpeg() {
        let stream = parse_resolve_output(
            br#"https://example.com/video
{"User-Agent":"browser-agent","Accept":"*/*","Bad_Name":"ignored","X-Test":"bad\nvalue"}
"#,
            PixelVideoQuality::P360,
        )
        .unwrap();

        assert_eq!(stream.url, "https://example.com/video");
        assert_eq!(stream.quality, PixelVideoQuality::P360);
        assert_eq!(
            stream.http_headers,
            "Accept: */*\r\nUser-Agent: browser-agent\r\n"
        );
    }

    #[test]
    fn stream_format_caps_the_source_at_the_selected_quality() {
        assert_eq!(
            stream_format(PixelVideoQuality::P144),
            "bestvideo[height<=144][vcodec^=avc1]/bestvideo[height<=144]/\
             best[height<=144]/best"
        );
        assert_eq!(
            stream_format(PixelVideoQuality::P720),
            "bestvideo[height<=720][vcodec^=avc1]/bestvideo[height<=720]/\
             best[height<=720]/best"
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
            protocol_from_env("xterm-256color", "ghostty", false),
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
    fn mpv_vo_probe_matches_only_the_kitty_driver_entry() {
        assert!(output_has_kitty_vo(
            b"Available video outputs:\n  gpu\n  kitty   Kitty terminal graphics protocol\n",
            b""
        ));
        assert!(!output_has_kitty_vo(
            b"Available video outputs:\n  gpu\n",
            b"warning mentions kitty elsewhere"
        ));
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
    fn frame_exchange_reuses_consumed_rgb_allocations() {
        let mut exchange = FrameExchange::default();
        let original = vec![7; 24];
        let original_ptr = original.as_ptr();

        assert!(
            exchange
                .publish(Frame {
                    width: 2,
                    height_px: 4,
                    rgb: original,
                })
                .is_none()
        );
        let consumed = exchange.take_latest().unwrap();
        exchange.recycle(consumed.rgb, 24);

        let recycled = exchange
            .publish(Frame {
                width: 2,
                height_px: 4,
                rgb: vec![9; 24],
            })
            .unwrap();
        assert_eq!(recycled.as_ptr(), original_ptr);
    }

    #[test]
    fn frame_exchange_rejects_a_buffer_from_an_old_size() {
        let mut exchange = FrameExchange::default();
        exchange.recycle(vec![0; 12], 24);
        assert!(exchange.spare.is_none());
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
    fn frame_rate_tracks_the_effective_transport_not_detected_capability() {
        let mut state = VideoState::new();
        #[allow(deprecated)] // test-only picker construction without a tty
        let mut picker = Picker::from_fontsize((8, 16).into());
        picker.set_protocol_type(ProtocolType::Kitty);
        state.picker = Some(picker);
        state.kitty_transport = Some(KittyTransport::new(true));

        assert_eq!(state.fps(VideoRenderMode::Auto), SHM_FPS);
        assert_eq!(state.fps(VideoRenderMode::Pixels), SHM_FPS);
        assert_eq!(state.fps(VideoRenderMode::Blocks), PTY_FPS);
        state.kitty_transport = Some(KittyTransport::new(false));
        assert_eq!(state.fps(VideoRenderMode::Auto), PTY_FPS);
        assert_eq!(state.fps(VideoRenderMode::Pixels), PTY_FPS);
        assert_eq!(
            state.tick_rate(VideoRenderMode::Blocks, false),
            Duration::from_secs_f64(1.0 / PTY_FPS)
        );
    }

    #[test]
    fn native_mpv_kitty_mode_is_mode_aware_and_uses_font_geometry() {
        let mut state = VideoState::new();
        #[allow(deprecated)] // test-only picker construction without a tty
        let mut picker = Picker::from_fontsize((8, 16).into());
        picker.set_protocol_type(ProtocolType::Kitty);
        state.picker = Some(picker);
        state.kitty_transport = Some(KittyTransport::new(true));
        state.mpv_kitty = true;

        assert!(state.mpv_kitty_active(VideoRenderMode::Auto));
        assert!(state.mpv_kitty_active(VideoRenderMode::Pixels));
        assert!(!state.mpv_kitty_active(VideoRenderMode::Blocks));
        assert_eq!(
            state.mpv_kitty_geometry(100, 30),
            Some(MpvKittyGeometry {
                cols: 100,
                rows: 30,
                width_px: 800,
                height_px: 480,
            })
        );
        assert_eq!(
            state.tick_rate(VideoRenderMode::Auto, true),
            MPV_KITTY_TICK_RATE
        );
    }

    #[test]
    fn prefetch_is_a_noop_when_the_url_is_already_cached() {
        let mut state = VideoState::new();
        state.cache.insert(
            "abc123".to_string(),
            ResolvedStream {
                url: "https://cached".to_string(),
                http_headers: String::new(),
                quality: PixelVideoQuality::P144,
            },
        );
        state.begin_resolve("abc123", PixelVideoQuality::P144);
        assert!(state.resolving.is_none());
    }

    #[test]
    fn switching_quality_preserves_the_matching_prefetch() {
        let (tx, rx) = mpsc::channel();
        let mut state = VideoState::new();
        state.current_video_id = Some("next".to_string());
        state.current_quality = Some(PixelVideoQuality::P144);
        state.last_frame = Some(Frame {
            width: 1,
            height_px: 2,
            rgb: vec![0; 6],
        });
        state.resolving = Some(PendingResolve {
            video_id: "next".to_string(),
            quality: PixelVideoQuality::P360,
            rx,
            cancelled: Arc::new(AtomicBool::new(false)),
            worker: None,
        });

        state.switch_track("next", PixelVideoQuality::P360);

        assert_eq!(
            state
                .resolving
                .as_ref()
                .map(|pending| pending.video_id.as_str()),
            Some("next")
        );
        assert_eq!(state.current_video_id.as_deref(), Some("next"));
        assert_eq!(state.current_quality, Some(PixelVideoQuality::P360));
        assert!(state.last_frame.is_none());
        drop(tx);
    }

    #[test]
    fn drift_exceeded_respects_the_two_second_boundary() {
        assert!(!drift_exceeded(10.0, 8.1)); // 1.9s, under threshold
        assert!(drift_exceeded(10.0, 7.9)); // 2.1s, over threshold
        assert!(!drift_exceeded(8.1, 10.0)); // negative drift, under threshold
        assert!(drift_exceeded(7.9, 10.0)); // negative drift, over threshold
    }

    #[test]
    fn startup_timeout_only_applies_before_the_first_frame() {
        assert!(!startup_timed_out(0, VIDEO_STARTUP_TIMEOUT));
        assert!(startup_timed_out(
            0,
            VIDEO_STARTUP_TIMEOUT + Duration::from_millis(1)
        ));
        assert!(!startup_timed_out(1, Duration::from_secs(60)));
    }

    #[test]
    fn video_state_stop_clears_session_scoped_fields_but_keeps_the_cache() {
        let mut state = VideoState::new();
        state.current_video_id = Some("abc".to_string());
        state.current_quality = Some(PixelVideoQuality::P144);
        state.error = Some("boom".to_string());
        state.paused = true;
        state.give_up = true;
        state.cache.insert(
            "abc".to_string(),
            ResolvedStream {
                url: "url".to_string(),
                http_headers: String::new(),
                quality: PixelVideoQuality::P144,
            },
        );

        state.stop();

        assert!(state.current_video_id.is_none());
        assert!(state.current_quality.is_none());
        assert!(state.error.is_none());
        assert!(!state.paused);
        assert!(!state.give_up);
        assert_eq!(
            state.cache.get("abc").map(|stream| stream.url.as_str()),
            Some("url")
        );
    }

    #[test]
    fn sync_with_nothing_playing_resets_to_placeholder() {
        let mut state = VideoState::new();
        state.current_video_id = Some("abc".to_string());
        state.current_quality = Some(PixelVideoQuality::P144);
        state.last_frame = Some(Frame {
            width: 1,
            height_px: 2,
            rgb: vec![0; 6],
        });

        state.sync(
            false,
            false,
            None,
            0.0,
            80,
            24,
            VideoRenderMode::Auto,
            PixelVideoQuality::P144,
        );

        assert!(state.current_video_id.is_none());
        assert!(matches!(state.render_state(), VideoDisplay::Placeholder));
    }

    #[test]
    fn sync_while_paused_drops_the_session_but_keeps_the_last_frame() {
        let mut state = VideoState::new();
        state.current_video_id = Some("abc".to_string());
        state.current_quality = Some(PixelVideoQuality::P144);
        state.last_frame = Some(Frame {
            width: 1,
            height_px: 2,
            rgb: vec![9, 9, 9, 1, 1, 1],
        });

        state.sync(
            true,
            true,
            Some("abc"),
            5.0,
            80,
            24,
            VideoRenderMode::Auto,
            PixelVideoQuality::P144,
        );

        assert!(state.session.is_none());
        assert!(state.paused);
        assert!(matches!(state.render_state(), VideoDisplay::Frame(_, true)));
    }

    #[test]
    fn sync_while_paused_still_consumes_prefetch_until_a_first_frame_exists() {
        let (tx, rx) = mpsc::channel();
        tx.send(Err("fixture resolve failure".to_string())).unwrap();

        let mut state = VideoState::new();
        state.current_video_id = Some("abc".to_string());
        state.current_quality = Some(PixelVideoQuality::P144);
        state.resolving = Some(PendingResolve {
            video_id: "abc".to_string(),
            quality: PixelVideoQuality::P144,
            rx,
            cancelled: Arc::new(AtomicBool::new(false)),
            worker: None,
        });

        state.sync(
            true,
            true,
            Some("abc"),
            8.0,
            80,
            24,
            VideoRenderMode::Auto,
            PixelVideoQuality::P144,
        );

        assert!(state.resolving.is_none());
        assert!(state.paused);
        assert!(matches!(state.render_state(), VideoDisplay::Error(_)));
    }

    #[test]
    fn resolver_failure_stays_failed_until_the_pane_is_reopened() {
        for disconnected in [false, true] {
            let (tx, rx) = mpsc::channel();
            if !disconnected {
                tx.send(Err("fixture extraction failure".to_string()))
                    .unwrap();
            }
            drop(tx);
            let mut state = VideoState::new();
            state.current_video_id = Some("abc".to_string());
            state.current_quality = Some(PixelVideoQuality::P144);
            state.resolving = Some(PendingResolve {
                video_id: "abc".to_string(),
                quality: PixelVideoQuality::P144,
                rx,
                cancelled: Arc::new(AtomicBool::new(false)),
                worker: None,
            });
            for _ in 0..20 {
                state.sync(
                    true,
                    false,
                    Some("abc"),
                    0.0,
                    80,
                    24,
                    VideoRenderMode::Blocks,
                    PixelVideoQuality::P144,
                );
                assert!(state.resolving.is_none());
                assert!(state.give_up);
                assert!(matches!(state.render_state(), VideoDisplay::Error(_)));
            }
            state.stop();
            assert!(!state.give_up);
            assert!(state.error.is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn dropping_a_resolver_cancels_descendants_and_removes_extraction_files() {
        check_resolver_cleanup(false);
    }

    #[cfg(unix)]
    #[test]
    fn resolver_timeout_cancels_descendants_and_removes_extraction_files() {
        check_resolver_cleanup(true);
    }

    #[cfg(unix)]
    fn check_resolver_cleanup(timeout: bool) {
        let fixture = tempfile::tempdir().unwrap();
        let ready = fixture.path().join("ready");
        let escaped = fixture.path().join("escaped");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                r#"mkdir "$TMPDIR/_MEI_fixture"
printf '%s' "$TMPDIR" > "$1"
(sleep 1; printf escaped > "$2") &
wait"#,
                "resolver-fixture",
            ])
            .arg(&ready)
            .arg(&escaped);
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let deadline = if timeout {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(5)
        };
        let worker = thread::spawn(move || {
            let result = run_resolve_command(command, &worker_cancelled, deadline);
            tx.send(result.map(|_| ResolvedStream {
                url: "unused".to_string(),
                http_headers: String::new(),
                quality: PixelVideoQuality::P144,
            }))
            .unwrap();
        });
        let pending = PendingResolve {
            video_id: "fixture".to_string(),
            quality: PixelVideoQuality::P144,
            rx,
            cancelled,
            worker: Some(worker),
        };
        let started = Instant::now();
        while !ready.exists() {
            assert!(started.elapsed() < Duration::from_secs(3));
            thread::sleep(Duration::from_millis(10));
        }
        if timeout {
            let error = pending
                .rx
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap_err();
            assert!(error.contains("timed out"), "{error}");
        }
        drop(pending);
        let temp_path = std::fs::read_to_string(&ready).unwrap();
        assert!(!std::path::Path::new(&temp_path).exists());
        thread::sleep(Duration::from_millis(1100));
        assert!(
            !escaped.exists(),
            "resolver descendant survived cancellation"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolver_drains_large_output_without_pipe_backpressure() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 70000 /dev/zero"]);
        let output =
            run_resolve_command(command, &AtomicBool::new(false), Duration::from_secs(3)).unwrap();
        assert_eq!(output.len(), 70000);
    }

    #[cfg(unix)]
    #[test]
    fn resolver_returns_a_usable_stream_and_removes_its_temp_files() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            r#"printf '%s\n' 'https://example.com/video' '{"User-Agent":"fixture"}'"#,
        ]);
        let output =
            run_resolve_command(command, &AtomicBool::new(false), Duration::from_secs(3)).unwrap();
        let stream = parse_resolve_output(&output, PixelVideoQuality::P240).unwrap();
        assert_eq!(stream.url, "https://example.com/video");
        assert!(stream.http_headers.contains("User-Agent: fixture"));
    }

    #[test]
    fn direct_kitty_video_uses_full_pane_cell_dimensions_on_hidpi() {
        let mut state = VideoState::new();
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize((16, 34).into());
        picker.set_protocol_type(ProtocolType::Kitty);
        state.picker = Some(picker);
        state.kitty_transport = Some(KittyTransport::new(false));
        let (width, height_px) = decode_size(188, 51, Some((16, 34)));
        let frame = Frame {
            width,
            height_px,
            rgb: vec![0; usize::from(width) * usize::from(height_px) * 3],
        };
        state.store_frame(frame, true, 188, 51);
        assert!(state.last_protocol.is_none());
        let VideoDisplay::Kitty(transport, _) = state.render_state() else {
            panic!("expected direct Kitty transport");
        };
        let area = ratatui::layout::Rect::new(0, 0, 188, 51);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        transport.render(area, &mut buffer);
        assert!(
            buffer
                .cell((0, 0))
                .unwrap()
                .symbol()
                .contains(",c=188,r=51,")
        );
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
