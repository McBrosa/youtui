//! Terminal video view. Video uses an independent yt-dlp/ffmpeg pipeline so
//! failures and startup latency never affect the long-lived audio player. See
//! docs/superpowers/specs/2026-08-11-terminal-video-view-design.md.

mod mpv_source;

use std::collections::HashMap;
use std::fs::File;
use std::io::{IsTerminal, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
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
use crate::player_manager::PlaybackStatus;

/// Decode-size ceiling for the pixel renderer. Panes larger than this are
/// upscaled by the terminal-side image resize instead of pushing more raw
/// RGB through the ffmpeg pipe and graphics protocol.
/// ponytail: fixed 720p ceiling; make it configurable if 4K terminals care.
const MAX_PIXEL_WIDTH: u32 = 1280;
const MAX_PIXEL_HEIGHT: u32 = 720;
const VIDEO_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const HIDDEN_VIDEO_GRACE: Duration = Duration::from_secs(3);
// mpv's position is polled every 250 ms; smaller startup offsets are within one poll.
const STARTUP_SYNC_TOLERANCE: f64 = 0.25;
const STREAM_RESOLVE_TIMEOUT: Duration = Duration::from_secs(15);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);
// mpv's output is captured so youtui can serialize Kitty frames with Ratatui.
// Drain at slightly over 60Hz for SHM sources; the bounded direct fallback is
// itself capped at 24fps.
const MPV_KITTY_TICK_RATE: Duration = Duration::from_millis(16);

static MPV_KITTY_AVAILABLE: OnceLock<bool> = OnceLock::new();
static FFMPEG_INITIAL_REQUEST_SIZE: OnceLock<bool> = OnceLock::new();

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
    Some(picker_for_protocol(protocol, font_size_from_winsize()?))
}

fn picker_for_protocol(protocol: ProtocolType, font_size: (u16, u16)) -> Picker {
    #[expect(
        deprecated,
        reason = "querying stdio for cell size races the TUI input reader"
    )]
    let mut picker = Picker::from_fontsize(font_size.into());
    picker.set_protocol_type(protocol);
    picker
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
    initial_request_size_supported: bool,
) -> Vec<String> {
    let mut args = Vec::new();
    if initial_request_size_supported && (url.starts_with("https://") || url.starts_with("http://"))
    {
        // Some CDNs delay an open-ended GET. Fetch the header in a bounded range,
        // then let ffmpeg request the bytes needed at the audio playhead.
        args.extend(["-initial_request_size".to_string(), "65536".to_string()]);
    }
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
    /// slower than the pipeline) and make the frame clock lag mpv until the
    /// drift check killed a perfectly healthy session.
    frames_read: Arc<AtomicU64>,
    /// Decode without frame pacing up to this frame while catching up with audio.
    catch_up_frame: Arc<AtomicU64>,
    w_px: u16,
    h_px: u16,
    fps: f64,
    start_pos: f64,
    started_at: Instant,
    synchronized: bool,
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
        let args = ffmpeg_args(
            stream_url,
            http_headers,
            position,
            w_px,
            h_px,
            fps,
            FFMPEG_INITIAL_REQUEST_SIZE.get() == Some(&true),
        );

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
        let catch_up_frame = Arc::new(AtomicU64::new(0));

        let reader = {
            let exchange = Arc::clone(&exchange);
            let frames_read = Arc::clone(&frames_read);
            let catch_up_frame = Arc::clone(&catch_up_frame);
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
                        let (recycled, count) = {
                            let mut exchange = exchange.lock().unwrap();
                            let recycled = exchange.publish(frame);
                            (recycled, frames_read.fetch_add(1, Ordering::Relaxed) + 1)
                        };
                        buf = recycled.unwrap_or_else(|| vec![0u8; frame_size]);
                        if count < catch_up_frame.load(Ordering::Relaxed) {
                            next_frame_at = Instant::now();
                            continue;
                        }

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
            catch_up_frame,
            w_px,
            h_px,
            fps,
            start_pos: position,
            started_at: Instant::now(),
            synchronized: false,
            stopped: false,
            from_cache: false,
            dead_handled: false,
        })
    }

    /// Latest frame if a new one arrived since the last poll.
    pub fn poll_frame(&mut self) -> Option<(Frame, f64)> {
        let mut exchange = self.exchange.lock().unwrap();
        let frame = exchange.take_latest()?;
        Some((frame, self.frame_position()))
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

    fn frame_position(&self) -> f64 {
        // Frame one is the frame requested with -ss.
        self.start_pos + self.frames_read().saturating_sub(1) as f64 / self.fps
    }

    /// Catch up to the audio that actually played while ffmpeg opened the input.
    /// Return true when seeking the decoder is preferable to reading intervening frames.
    fn needs_seek(&mut self, mpv_pos: f64, paused: bool) -> bool {
        if self.frames_read() == 0 {
            return false;
        }
        let frame_pos = self.frame_position();
        if self.synchronized {
            return if paused {
                (mpv_pos - frame_pos).abs() > STARTUP_SYNC_TOLERANCE
            } else {
                drift_exceeded(mpv_pos, frame_pos)
            };
        }
        let advance = mpv_pos - self.start_pos;
        let lag = mpv_pos - frame_pos;
        if advance < -STARTUP_SYNC_TOLERANCE
            || advance > self.started_at.elapsed().as_secs_f64() + STARTUP_SYNC_TOLERANCE * 2.0
            || (paused && lag.abs() > STARTUP_SYNC_TOLERANCE)
        {
            return true;
        }
        if lag <= STARTUP_SYNC_TOLERANCE {
            self.synchronized = true;
            self.catch_up_frame.store(0, Ordering::Relaxed);
            return drift_exceeded(mpv_pos, frame_pos);
        }
        // A slow seek or decoder must not trigger an unbounded burst of decoded frames.
        if advance > VIDEO_STARTUP_TIMEOUT.as_secs_f64() + 2.0 {
            return true;
        }
        self.catch_up_frame
            .store((advance * self.fps).round() as u64 + 1, Ordering::Relaxed);
        false
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
    from_audio_player: bool,
}

struct PendingResolve {
    video_id: String,
    quality: PixelVideoQuality,
    rx: Receiver<Result<ResolvedStream, String>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

struct PresentedFrame {
    position: f64,
    size: (u16, u16),
    mode: VideoRenderMode,
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
    /// When the pane closed and the audio playhead at that instant.
    hidden_since: Option<(Instant, f64)>,
    presented: Option<PresentedFrame>,
    /// Raw cell-renderer output or pixels for restoring a cleared Kitty image.
    last_frame: Option<Frame>,
    /// Last frame encoded for the generic terminal graphics protocol.
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
            hidden_since: None,
            presented: None,
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

    /// Release video processes and presentation state, retaining resolved URLs.
    pub fn stop(&mut self) {
        self.session = None;
        self.resolving = None; // cancel and join the resolver before starting another
        self.hidden_since = None;
        self.clear_output();
        self.error = None;
        self.current_video_id = None;
        self.current_quality = None;
        self.paused = false;
        self.last_die = None;
        self.give_up = false;
        self.retried_after_evict = false;
    }

    /// Keep an already requested video warm for a quick return to the same track.
    pub fn hide(
        &mut self,
        status: &PlaybackStatus,
        video_id: Option<&str>,
        quality: PixelVideoQuality,
    ) {
        if self.current_video_id.is_none() {
            return;
        }
        if self.current_video_id.as_deref() != video_id
            || self.current_quality != Some(quality)
            || !(status.playing || status.paused)
            || self.error.is_some()
            || self.give_up
        {
            self.stop();
            return;
        }
        if self.hidden_since.is_none() {
            self.hidden_since = Some((Instant::now(), status.time_pos));
        }
        self.maintain_hidden(status);
    }

    fn clear_output(&mut self) {
        self.presented = None;
        self.last_frame = None;
        self.last_protocol = None;
        if let Some(transport) = self.kitty_transport.as_mut() {
            transport.reset();
        }
    }

    fn maintain_hidden(&mut self, status: &PlaybackStatus) {
        let Some((hidden_at, position)) = self.hidden_since else {
            return;
        };
        if self.presented.as_ref().is_some_and(|frame| {
            !status.paused || (frame.position - status.time_pos).abs() > STARTUP_SYNC_TOLERANCE
        }) {
            self.clear_output();
        }
        let elapsed = hidden_at.elapsed();
        if elapsed >= HIDDEN_VIDEO_GRACE {
            // A resolver may have finished after the pane closed. Keep its URL
            // for the next visit without starting a decoder in the background.
            if let Some(pending) = &self.resolving
                && let Ok(Ok(stream)) = pending.rx.try_recv()
            {
                self.cache.insert(pending.video_id.clone(), stream);
            }
            self.stop();
        } else if status.paused || drift_exceeded(status.time_pos, position + elapsed.as_secs_f64())
        {
            self.session = None;
        }
    }

    /// Start at most one resolver for the requested stream while video is visible.
    fn begin_resolve(&mut self, video_id: &str, quality: PixelVideoQuality, socket: Option<&Path>) {
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
        let socket = socket.filter(|_| !self.retried_after_evict);
        self.resolving = Some(spawn_resolve(video_id, quality, socket));
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
    pub fn sync(
        &mut self,
        status: &PlaybackStatus,
        video_id: Option<&str>,
        socket: Option<&Path>,
        (cols, rows): (u16, u16),
        mode: VideoRenderMode,
        quality: PixelVideoQuality,
    ) {
        let Some(video_id) = video_id.filter(|_| status.playing || status.paused) else {
            self.stop();
            return;
        };

        self.maintain_hidden(status);
        let reopening = self.hidden_since.take().is_some();
        if self.current_video_id.as_deref() != Some(video_id)
            || self.current_quality != Some(quality)
        {
            self.switch_track(video_id, quality);
        }

        if let Some(frame) = &self.presented {
            if frame.size != (cols, rows)
                || frame.mode != mode
                || (status.paused
                    && (frame.position - status.time_pos).abs() > STARTUP_SYNC_TOLERANCE)
            {
                self.clear_output();
            } else if status.paused && (!reopening || self.restore_kitty_frame(cols, rows)) {
                self.session = None;
                self.paused = true;
                return;
            } else if status.paused {
                self.clear_output();
            }
        }
        // A pause before the first frame must still drive URL resolution and
        // decode one frame. Otherwise the early return above leaves a completed
        // prefetch unread forever and the pane stays on "loading video…".
        self.paused = status.paused;

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

        self.restart_if_stale(video_id, status.time_pos, w_px, h_px, fps);
        self.handle_session_death(video_id);

        if self.give_up {
            return;
        }

        if let Some((frame, position)) = self.session.as_mut().and_then(VideoSession::poll_frame)
            && self.store_frame(frame, pixels, cols, rows)
        {
            self.presented = Some(PresentedFrame {
                position,
                size: (cols, rows),
                mode,
            });
        }
        if self.session.is_some() {
            return;
        }

        if self.resolving.is_none()
            && let Some(stream) = self
                .cache
                .get(video_id)
                .filter(|stream| stream.quality == quality)
                .cloned()
        {
            self.start_session(&stream, status.time_pos, w_px, h_px, fps, true);
            return;
        }

        self.poll_resolve(
            video_id,
            quality,
            socket,
            status.time_pos,
            (w_px, h_px),
            fps,
        );
    }

    fn restore_kitty_frame(&mut self, cols: u16, rows: u16) -> bool {
        // Clearing the terminal can delete Kitty image data along with its
        // placements. A paused decoder cannot supply a replacement frame.
        if self
            .kitty_transport
            .as_ref()
            .is_some_and(KittyTransport::has_frame)
            || matches!(self.last_protocol, Some(Protocol::Kitty(_)))
        {
            return self
                .last_frame
                .take()
                .is_some_and(|frame| self.store_frame(frame, true, cols, rows));
        }
        true
    }

    fn retain_frame(&mut self, frame: Frame) {
        if let Some(previous) = self.last_frame.replace(frame)
            && let Some(session) = &self.session
        {
            session.recycle_buffer(previous.rgb);
        }
    }

    /// Store a decoded frame in the form the renderer needs: encoded for the
    /// terminal's graphics protocol when the pixel renderer is active,
    /// otherwise raw for the half-block path.
    fn store_frame(&mut self, frame: Frame, pixels: bool, cols: u16, rows: u16) -> bool {
        if !pixels {
            self.retain_frame(frame);
            self.last_protocol = None;
            return true;
        }
        if let Some(transport) = self.kitty_transport.as_mut() {
            match transport.push_frame(&frame, cols, rows) {
                Ok(()) => {
                    self.retain_frame(frame);
                    self.last_protocol = None;
                    return true;
                }
                Err(_error) => {
                    // Keep explicit cell placement when shared memory fails.
                    let mut direct = KittyTransport::new(false);
                    if direct.push_frame(&frame, cols, rows).is_ok() {
                        self.kitty_transport = Some(direct);
                        self.retain_frame(frame);
                        self.last_protocol = None;
                        return true;
                    }
                }
            }
        }
        let Some(picker) = self.picker.as_ref() else {
            self.retain_frame(frame);
            self.last_protocol = None;
            return true;
        };
        // The generic Kitty encoder (used for tmux passthrough) takes
        // ownership of its RGB input and only transmits the encoded image once.
        let replay = (picker.protocol_type() == ProtocolType::Kitty).then(|| Frame {
            width: frame.width,
            height_px: frame.height_px,
            rgb: frame.rgb.clone(),
        });
        let Some(image) = RgbImage::from_raw(
            u32::from(frame.width),
            u32::from(frame.height_px),
            frame.rgb,
        ) else {
            return false; // impossible: buffer size is derived from these dims
        };
        let size = Size::new(cols, rows);
        // Scale (not Fit): the decode size is capped at 720p, so panes larger
        // than that need the image upscaled to fill the cell area.
        let Ok(protocol) =
            picker.new_protocol(DynamicImage::ImageRgb8(image), size, Resize::Scale(None))
        else {
            return false;
        };
        self.last_protocol = Some(protocol);
        self.last_frame = replay;
        true
    }

    fn has_output(&self) -> bool {
        self.last_frame.is_some()
            || self.last_protocol.is_some()
            || self
                .kitty_transport
                .as_ref()
                .is_some_and(KittyTransport::has_frame)
    }

    fn restart_if_stale(&mut self, video_id: &str, time_pos: f64, w_px: u16, h_px: u16, fps: f64) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.w_px != w_px
            || session.h_px != h_px
            || session.fps != fps
            || session.needs_seek(time_pos, self.paused)
        {
            self.session = None;
        } else if startup_timed_out(session.frames_read(), session.started_at.elapsed()) {
            let from_cache = session.from_cache;
            self.session = None;
            self.fail_or_refresh_source(
                video_id,
                from_cache,
                "video unavailable: stream startup timed out",
            );
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

        if died_immediately && (from_cache || self.retried_after_evict) {
            self.fail_or_refresh_source(
                video_id,
                from_cache,
                "video unavailable: ffmpeg exited before the first frame",
            );
            return;
        }

        self.record_death();
    }

    fn fail_or_refresh_source(&mut self, video_id: &str, from_cache: bool, message: &str) {
        self.cache.remove(video_id);
        if from_cache && !self.retried_after_evict {
            self.retried_after_evict = true;
        } else {
            self.error = Some(message.to_string());
            self.give_up = true;
        }
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
        socket: Option<&Path>,
        position: f64,
        (w_px, h_px): (u16, u16),
        fps: f64,
    ) {
        let needs_new_request = match &self.resolving {
            Some(pending) => pending.video_id != video_id || pending.quality != quality,
            None => true,
        };
        if needs_new_request {
            self.begin_resolve(video_id, quality, socket);
            return;
        }

        let Some(pending) = &self.resolving else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(Ok(stream)) => {
                self.cache.insert(video_id.to_string(), stream.clone());
                self.resolving = None;
                self.start_session(&stream, position, w_px, h_px, fps, stream.from_audio_player);
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
fn spawn_resolve(
    video_id: &str,
    quality: PixelVideoQuality,
    socket: Option<&Path>,
) -> PendingResolve {
    let (tx, rx) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let track = video_id.to_owned();
    let socket = socket.map(Path::to_owned);
    let worker = thread::Builder::new()
        .name("youtui-video-resolver".to_string())
        .spawn(move || {
            let stream = socket
                .as_deref()
                .and_then(|path| mpv_source::from_player(path, &track, quality))
                .map(Ok)
                .unwrap_or_else(|| {
                    let url = format!("https://www.youtube.com/watch?v={track}");
                    resolve_stream_url(&url, quality, &worker_cancelled)
                });
            if stream.is_ok() {
                probe_ffmpeg_http_options(&worker_cancelled);
            }
            let _ = tx.send(stream);
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

// Older ffmpeg versions reject the HTTP option. Probe once on the video worker
// so startup, playback controls and audio-only sessions never wait for it.
fn probe_ffmpeg_http_options(cancelled: &AtomicBool) {
    if FFMPEG_INITIAL_REQUEST_SIZE.get().is_some() {
        return;
    }
    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-h", "protocol=http"]);
    let supported =
        run_resolve_command(command, cancelled, Duration::from_secs(1)).is_ok_and(|help| {
            String::from_utf8_lossy(&help)
                .split_ascii_whitespace()
                .any(|word| word == "-initial_request_size")
        });
    if !cancelled.load(Ordering::Relaxed) {
        let _ = FFMPEG_INITIAL_REQUEST_SIZE.set(supported);
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
    let mut command = crate::deps::ytdlp_command();
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

    let headers = lines
        .next()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok());

    Ok(ResolvedStream {
        url: url.to_string(),
        http_headers: ffmpeg_headers(headers.as_ref()),
        quality,
        from_audio_player: false,
    })
}

fn ffmpeg_headers(value: Option<&serde_json::Value>) -> String {
    let mut output = String::new();
    if let Some(headers) = value.and_then(serde_json::Value::as_object) {
        for (name, value) in headers {
            if let Some(value) = value.as_str()
                && safe_http_header(name, value)
            {
                output.push_str(name);
                output.push_str(": ");
                output.push_str(value);
                output.push_str("\r\n");
            }
        }
    }
    output
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
            true,
        );
        assert_eq!(
            args,
            vec![
                "-initial_request_size",
                "65536",
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
        let args = ffmpeg_args("https://example.com/stream", "", 0.0, 80, 48, 24.0, false);

        assert!(!args.iter().any(|arg| arg == "-initial_request_size"));
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
        state.set_picker(picker_for_protocol(ProtocolType::Iterm2, (8, 16)));
        assert!(state.pixels_active(VideoRenderMode::Auto));
        assert!(state.pixels_active(VideoRenderMode::Pixels));
        assert!(!state.pixels_active(VideoRenderMode::Blocks));
    }

    #[test]
    fn frame_rate_tracks_the_effective_transport_not_detected_capability() {
        let mut state = VideoState::new();
        state.picker = Some(picker_for_protocol(ProtocolType::Kitty, (8, 16)));
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
        state.picker = Some(picker_for_protocol(ProtocolType::Kitty, (8, 16)));
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
                from_audio_player: false,
            },
        );
        state.begin_resolve("abc123", PixelVideoQuality::P144, None);
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

    #[cfg(unix)]
    fn session_with_queued_frame(fps: f64) -> VideoSession {
        VideoSession {
            child: Command::new("cat").stdin(Stdio::piped()).spawn().unwrap(),
            reader: None,
            exchange: Arc::new(Mutex::new(FrameExchange {
                latest: Some(Frame {
                    width: 1,
                    height_px: 2,
                    rgb: vec![0; 6],
                }),
                spare: None,
            })),
            frames_read: Arc::new(AtomicU64::new(1)),
            catch_up_frame: Arc::new(AtomicU64::new(0)),
            w_px: 1,
            h_px: 2,
            fps,
            start_pos: 0.0,
            started_at: Instant::now(),
            synchronized: false,
            stopped: false,
            from_cache: false,
            dead_handled: false,
        }
    }

    #[cfg(unix)]
    #[test]
    fn delayed_first_frame_syncs_before_checking_for_drift() {
        let mut state = VideoState::new();
        let quality = PixelVideoQuality::P144;
        let mode = VideoRenderMode::Blocks;
        let fps = state.fps(mode);
        state.switch_track("abc", quality);
        let mut session = session_with_queued_frame(fps);
        session.started_at = Instant::now() - Duration::from_secs(5);
        state.session = Some(session);
        let status = PlaybackStatus {
            playing: true,
            time_pos: 5.0,
            ..PlaybackStatus::default()
        };

        // Keep the regression hermetic even if the session would ordinarily restart the resolver.
        state.restart_if_stale("abc", status.time_pos, 1, 2, fps);
        assert!(state.session.is_some(), "first frame discarded as drift");
        let session = state.session.as_ref().unwrap();
        let target = 1 + (5.0 * fps) as u64;
        assert_eq!(session.catch_up_frame.load(Ordering::Relaxed), target);
        assert_eq!(session.frame_position(), 0.0);
        assert!(!session.synchronized);
        state.sync(&status, Some("abc"), None, (1, 1), mode, quality);
        assert!(state.last_frame.is_some());
        // Advance the reader to its unpaced target. The frame timestamp now matches mpv.
        state
            .session
            .as_ref()
            .unwrap()
            .frames_read
            .store(target, Ordering::Relaxed);
        state.restart_if_stale("abc", status.time_pos, 1, 2, fps);
        let session = state.session.as_ref().unwrap();
        assert!(session.synchronized);
        assert_eq!(session.frame_position(), 5.0);
        assert_eq!(session.catch_up_frame.load(Ordering::Relaxed), 0);
        state.restart_if_stale("abc", status.time_pos + 3.0, 1, 2, fps);
        assert!(
            state.session.is_none(),
            "seek after synchronization must restart"
        );
    }

    #[cfg(unix)]
    #[test]
    fn startup_seeks_and_pauses_never_request_an_unpaced_frame_burst() {
        for (playhead, elapsed, paused) in [(300.0, 3, false), (24.0, 24, false), (3.0, 3, true)] {
            let mut session = session_with_queued_frame(PTY_FPS);
            session.started_at = Instant::now() - Duration::from_secs(elapsed);
            assert!(session.needs_seek(playhead, paused));
            assert_eq!(session.catch_up_frame.load(Ordering::Relaxed), 0);
        }
        let mut session = session_with_queued_frame(PTY_FPS);
        assert!(!session.needs_seek(0.0, true));
        assert!(session.synchronized);
        assert_eq!(session.catch_up_frame.load(Ordering::Relaxed), 0);
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

    #[cfg(unix)]
    #[test]
    fn decoder_startup_timeout_refreshes_a_reused_source_only_once() {
        let quality = PixelVideoQuality::P144;
        let mut state = VideoState::new();
        state.switch_track("abc", quality);
        for reused in [true, false] {
            let mut session = session_with_queued_frame(PTY_FPS);
            session.frames_read.store(0, Ordering::Relaxed);
            session.started_at = Instant::now() - VIDEO_STARTUP_TIMEOUT - Duration::from_millis(1);
            session.from_cache = reused;
            state.session = Some(session);
            state.cache.insert(
                "abc".to_string(),
                ResolvedStream {
                    url: "expired".to_string(),
                    http_headers: String::new(),
                    quality,
                    from_audio_player: reused,
                },
            );
            state.restart_if_stale("abc", 0.0, 1, 2, PTY_FPS);
            assert!(state.session.is_none() && !state.cache.contains_key("abc"));
            assert!(state.retried_after_evict);
            assert_eq!(state.give_up, !reused);
            assert_eq!(
                state.error.as_deref(),
                (!reused).then_some("video unavailable: stream startup timed out")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn reopened_pane_reuses_the_decoder_and_discards_it_on_a_hidden_seek_or_pause() {
        let mut state = VideoState::new();
        let quality = PixelVideoQuality::P144;
        let mode = VideoRenderMode::Blocks;
        let mut status = PlaybackStatus {
            playing: true,
            ..PlaybackStatus::default()
        };
        state.switch_track("abc", quality);
        let session = session_with_queued_frame(state.fps(mode));
        let pid = session.child.id();
        state.session = Some(session);
        state.hide(&status, Some("abc"), quality);
        state.sync(&status, Some("abc"), None, (1, 1), mode, quality);
        assert_eq!(state.session.as_ref().unwrap().child.id(), pid);
        assert!(matches!(
            state.render_state(),
            VideoDisplay::Frame(_, false)
        ));

        state.hide(&status, Some("abc"), quality);
        assert!(!state.has_output());
        status.time_pos = 60.0;
        state.hide(&status, Some("abc"), quality);
        assert!(state.session.is_none());

        state.hidden_since = None;
        state.session = Some(session_with_queued_frame(state.fps(mode)));
        status.paused = true;
        state.hide(&status, Some("abc"), quality);
        assert!(state.session.is_none());
    }

    #[test]
    fn hidden_pane_releases_workers_and_caches_a_finished_resolution() {
        let quality = PixelVideoQuality::P240;
        let mut state = VideoState::new();
        state.switch_track("abc", quality);
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        state.resolving = Some(PendingResolve {
            video_id: "abc".to_string(),
            quality,
            rx,
            cancelled: Arc::clone(&cancelled),
            worker: None,
        });
        let status = PlaybackStatus {
            playing: true,
            ..PlaybackStatus::default()
        };
        state.hide(&status, Some("abc"), quality);
        assert!(!cancelled.load(Ordering::Relaxed));
        tx.send(Ok(ResolvedStream {
            url: "video".to_string(),
            http_headers: String::new(),
            quality,
            from_audio_player: false,
        }))
        .unwrap();
        state.hidden_since = Some((Instant::now() - HIDDEN_VIDEO_GRACE, 0.0));
        state.hide(&status, Some("abc"), quality);
        assert!(cancelled.load(Ordering::Relaxed));
        assert_eq!(state.cache["abc"].url, "video");
        assert!(state.resolving.is_none() && state.session.is_none());
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
                from_audio_player: false,
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
            &PlaybackStatus::default(),
            None,
            None,
            (80, 24),
            VideoRenderMode::Auto,
            PixelVideoQuality::P144,
        );

        assert!(state.current_video_id.is_none());
        assert!(matches!(state.render_state(), VideoDisplay::Placeholder));
    }

    #[cfg(unix)]
    #[test]
    fn quick_reopen_reuses_a_paused_frame_only_at_its_playhead_size_and_mode() {
        let quality = PixelVideoQuality::P144;
        let mode = VideoRenderMode::Blocks;
        for (playhead, size, reopened_mode) in [
            (15.0, (1, 1), mode),
            (5.0, (2, 1), mode),
            (5.0, (1, 1), VideoRenderMode::Auto),
        ] {
            let mut state = VideoState::new();
            state.switch_track("abc", quality);
            let mut session = session_with_queued_frame(state.fps(mode));
            session.start_pos = 5.0;
            state.session = Some(session);
            let mut status = PlaybackStatus {
                playing: true,
                time_pos: 5.0,
                ..PlaybackStatus::default()
            };
            state.sync(&status, Some("abc"), None, (1, 1), mode, quality);
            status.paused = true;
            state.hide(&status, Some("abc"), quality);
            state.sync(&status, Some("abc"), None, (1, 1), mode, quality);
            assert!(matches!(state.render_state(), VideoDisplay::Frame(_, true)));
            assert!(state.session.is_none() && state.resolving.is_none());

            state.hide(&status, Some("abc"), quality);
            // Block resolution so an invalidated frame cannot be silently replaced.
            let (_tx, rx) = mpsc::channel();
            state.resolving = Some(PendingResolve {
                video_id: "abc".into(),
                quality,
                rx,
                cancelled: Arc::new(AtomicBool::new(false)),
                worker: None,
            });
            status.time_pos = playhead;
            state.sync(&status, Some("abc"), None, size, reopened_mode, quality);
            assert!(!state.has_output());
            assert!(matches!(state.render_state(), VideoDisplay::Loading));
        }
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
            &PlaybackStatus {
                playing: true,
                paused: true,
                time_pos: 8.0,
                ..PlaybackStatus::default()
            },
            Some("abc"),
            None,
            (80, 24),
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
            let status = PlaybackStatus {
                playing: true,
                ..PlaybackStatus::default()
            };
            for _ in 0..20 {
                state.sync(
                    &status,
                    Some("abc"),
                    None,
                    (80, 24),
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
(sleep 5; printf escaped > "$2") &
wait"#,
                "resolver-fixture",
            ])
            .arg(&ready)
            .arg(&escaped);
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let deadline = if timeout {
            // Allow the shell to start on a busy machine. The descendant
            // still runs longer than this deadline, so timeout must kill it.
            Duration::from_secs(2)
        } else {
            Duration::from_secs(5)
        };
        let worker = thread::spawn(move || {
            let result = run_resolve_command(command, &worker_cancelled, deadline);
            tx.send(result.map(|_| ResolvedStream {
                url: "unused".to_string(),
                http_headers: String::new(),
                quality: PixelVideoQuality::P144,
                from_audio_player: false,
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
        thread::sleep(Duration::from_millis(5100));
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
        state.picker = Some(picker_for_protocol(ProtocolType::Kitty, (16, 34)));
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
    fn paused_kitty_reopen_transmits_the_cached_pixels_again_after_screen_clear() {
        use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

        let area = Rect::new(0, 0, 2, 1);
        let render = |state: &VideoState| {
            let mut buffer = Buffer::empty(area);
            match state.render_state() {
                VideoDisplay::Kitty(transport, _) => transport.render(area, &mut buffer),
                VideoDisplay::Pixels(protocol, _) => {
                    ratatui_image::Image::new(protocol).render(area, &mut buffer)
                }
                _ => panic!("expected a Kitty frame"),
            }
            buffer.cell((0, 0)).unwrap().symbol().to_owned()
        };
        for use_shm in [Some(false), Some(true), None] {
            let mut state = VideoState::new();
            let quality = PixelVideoQuality::P144;
            let mode = VideoRenderMode::Auto;
            let status = PlaybackStatus {
                playing: true,
                paused: true,
                time_pos: 3.0,
                ..PlaybackStatus::default()
            };
            state.switch_track("video", quality);
            state.picker = Some(picker_for_protocol(ProtocolType::Kitty, (1, 2)));
            state.kitty_transport = use_shm.map(KittyTransport::new);
            let frame = Frame {
                width: 2,
                height_px: 2,
                rgb: vec![55; 12],
            };
            assert!(state.store_frame(frame, true, 2, 1));
            state.presented = Some(PresentedFrame {
                position: status.time_pos,
                size: (2, 1),
                mode,
            });

            for pass in 0..2 {
                let output = render(&state);
                assert!(
                    output.contains("a=T"),
                    "no Kitty pixel upload on render {pass}"
                );
                if use_shm == Some(true) {
                    assert!(output.contains("t=s"));
                    let payload = output
                        .split_once(';')
                        .unwrap()
                        .1
                        .split_once("\x1b\\")
                        .unwrap()
                        .0;
                    let bytes = base64_simd::STANDARD.decode_to_vec(payload).unwrap();
                    let name = std::ffi::CString::new(bytes).unwrap();
                    // SAFETY: this is the NUL-terminated name of the shm object the transport just uploaded.
                    assert_eq!(unsafe { libc::shm_unlink(name.as_ptr()) }, 0);
                }
                assert!(
                    !render(&state).contains("a=T"),
                    "ordinary redraw should reuse placement"
                );
                if pass == 0 {
                    state.hide(&status, Some("video"), quality);
                    state.sync(&status, Some("video"), None, (2, 1), mode, quality);
                }
            }
            assert!(state.resolving.is_none() && state.session.is_none());
        }
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
