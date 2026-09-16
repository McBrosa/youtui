use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tempfile::TempDir;

use crate::{config::Config, ipc::IpcClient, video::MpvKittyGeometry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(25);
const STATUS_PROPERTIES: [&str; 5] = ["time-pos", "duration", "pause", "volume", "eof-reached"];
const KITTY_APC_START: &[u8] = b"\x1b_G";
const KITTY_APC_END: &[u8] = b"\x1b\\";

const DIRECT_KITTY_MAX_WIDTH: u32 = 480;
const DIRECT_KITTY_MAX_HEIGHT: u32 = 270;

/// Capture mpv's Kitty stream and retain only the newest complete video frame.
/// This gives Ratatui sole ownership of stdout and bounds memory when a 60fps
/// producer temporarily outruns the TUI event loop.
struct MpvKittyOutput {
    latest_frame: Arc<Mutex<Option<Vec<u8>>>>,
    reader: Option<JoinHandle<()>>,
}

impl MpvKittyOutput {
    fn spawn(stdout: ChildStdout, use_shm: bool) -> Result<Self> {
        let latest_frame = Arc::new(Mutex::new(None));
        let reader_frame = Arc::clone(&latest_frame);
        let reader = thread::Builder::new()
            .name("youtui-mpv-kitty-output".to_string())
            .spawn(move || pump_mpv_kitty_output(stdout, &reader_frame, use_shm))
            .context("Failed to start mpv Kitty output reader")?;
        Ok(Self {
            latest_frame,
            reader: Some(reader),
        })
    }

    fn take_frame(&self) -> Option<Vec<u8>> {
        self.latest_frame.lock().ok()?.take()
    }

    fn join(&mut self) {
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn pump_mpv_kitty_output<R: Read>(
    mut stdout: R,
    latest_frame: &Arc<Mutex<Option<Vec<u8>>>>,
    use_shm: bool,
) {
    let mut pending = Vec::with_capacity(512);
    let mut direct_frame = Vec::new();
    let mut buffer = [0_u8; 4096];

    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                pending.extend_from_slice(&buffer[..read]);
                while let Some(end) = find_bytes(&pending, KITTY_APC_END) {
                    let packet: Vec<u8> = pending.drain(..end + KITTY_APC_END.len()).collect();
                    if use_shm {
                        if let Some(frame) = extract_mpv_kitty_shm_frame(&packet)
                            && !publish_latest_frame(latest_frame, frame)
                        {
                            return;
                        }
                        continue;
                    }

                    let Some((apc_start, apc_end, controls)) = kitty_apc(&packet) else {
                        continue;
                    };
                    let complete;
                    if kitty_param(controls, b"a=T") && !kitty_param(controls, b"t=s") {
                        let start = cursor_prefix_start(&packet, apc_start);
                        direct_frame.clear();
                        direct_frame.extend_from_slice(&packet[start..apc_end]);
                        // `m=0` is the protocol default and mpv omits it when
                        // the complete image fits in the first APC.
                        complete = !kitty_param(controls, b"m=1");
                    } else if !direct_frame.is_empty()
                        && (kitty_param(controls, b"m=1") || kitty_param(controls, b"m=0"))
                    {
                        direct_frame.extend_from_slice(&packet[apc_start..apc_end]);
                        complete = !kitty_param(controls, b"m=1");
                    } else {
                        continue;
                    }

                    if complete
                        && !publish_latest_frame(latest_frame, std::mem::take(&mut direct_frame))
                    {
                        return;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn publish_latest_frame(latest_frame: &Arc<Mutex<Option<Vec<u8>>>>, frame: Vec<u8>) -> bool {
    let Ok(mut latest) = latest_frame.lock() else {
        return false;
    };
    *latest = Some(frame);
    true
}

/// Accept only mpv's complete shared-memory frame packet. In particular, do
/// not forward its cursor/mouse setup or cleanup bytes: mpv 0.41's Kitty VO
/// emits an unterminated delete APC on shutdown, which can leave Ghostty's
/// parser consuming subsequent TUI styling. youtui owns all non-frame terminal
/// state and clears graphics during the view transition itself.
fn extract_mpv_kitty_shm_frame(packet: &[u8]) -> Option<Vec<u8>> {
    let (apc_start, apc_end, controls) = kitty_apc(packet)?;
    if !is_mpv_kitty_frame(packet) || !kitty_param(controls, b"t=s") {
        return None;
    }
    let start = cursor_prefix_start(packet, apc_start);
    Some(repair_mpv_shm_name(packet[start..apc_end].to_vec()))
}

fn kitty_apc(packet: &[u8]) -> Option<(usize, usize, &[u8])> {
    let apc_start = find_bytes(packet, KITTY_APC_START)?;
    let controls_start = apc_start + KITTY_APC_START.len();
    let semicolon_offset = packet[controls_start..]
        .iter()
        .position(|byte| *byte == b';')?;
    let end_offset = find_bytes(&packet[controls_start..], KITTY_APC_END)?;
    let apc_end = controls_start + end_offset + KITTY_APC_END.len();
    Some((
        apc_start,
        apc_end,
        &packet[controls_start..controls_start + semicolon_offset],
    ))
}

fn cursor_prefix_start(packet: &[u8], apc_start: usize) -> usize {
    rfind_bytes(&packet[..apc_start], b"\x1b[")
        .filter(|start| is_cursor_position(&packet[*start..apc_start]))
        .unwrap_or(apc_start)
}

fn is_cursor_position(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x1b[")
        && bytes.ends_with(b"f")
        && bytes[2..bytes.len() - 1]
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
}

fn repair_mpv_shm_name(packet: Vec<u8>) -> Vec<u8> {
    let Some(apc_start) = find_bytes(&packet, KITTY_APC_START) else {
        return packet;
    };
    let controls_start = apc_start + KITTY_APC_START.len();
    let Some(semicolon_offset) = packet[controls_start..]
        .iter()
        .position(|byte| *byte == b';')
    else {
        return packet;
    };
    let payload_start = controls_start + semicolon_offset + 1;
    let controls = &packet[controls_start..payload_start - 1];
    if !kitty_param(controls, b"t=s") {
        return packet;
    }
    let Some(payload_end_offset) = find_bytes(&packet[payload_start..], KITTY_APC_END) else {
        return packet;
    };
    let payload_end = payload_start + payload_end_offset;
    let Ok(name) = base64_simd::STANDARD.decode_to_vec(&packet[payload_start..payload_end]) else {
        return packet;
    };
    if name.starts_with(b"/") || !name.starts_with(b"mpv-kitty-") {
        return packet;
    }

    let mut repaired_name = Vec::with_capacity(name.len() + 1);
    repaired_name.push(b'/');
    repaired_name.extend_from_slice(&name);
    let repaired_payload = base64_simd::STANDARD.encode_to_string(&repaired_name);

    let mut repaired = Vec::with_capacity(packet.len() + repaired_payload.len());
    repaired.extend_from_slice(&packet[..payload_start]);
    repaired.extend_from_slice(repaired_payload.as_bytes());
    repaired.extend_from_slice(&packet[payload_end..]);
    repaired
}

fn is_mpv_kitty_frame(packet: &[u8]) -> bool {
    kitty_apc(packet).is_some_and(|(_, _, controls)| kitty_param(controls, b"a=T"))
}

fn add_direct_kitty_placement(packet: Vec<u8>, geometry: MpvKittyGeometry) -> Vec<u8> {
    let Some((apc_start, _, controls)) = kitty_apc(&packet) else {
        return packet;
    };
    if !kitty_param(controls, b"a=T")
        || kitty_param(controls, b"t=s")
        || kitty_param_prefix(controls, b"c=").is_some()
        || kitty_param_prefix(controls, b"r=").is_some()
    {
        return packet;
    }
    let Some(source_width) = kitty_param_u32(controls, b"s=") else {
        return packet;
    };
    let Some(source_height) = kitty_param_u32(controls, b"v=") else {
        return packet;
    };
    if geometry.width_px == 0 || geometry.height_px == 0 {
        return packet;
    }

    let cols = scaled_cells(source_width, geometry.width_px, geometry.cols);
    let rows = scaled_cells(source_height, geometry.height_px, geometry.rows);
    let placement = format!(",c={cols},r={rows}");
    let controls_end = apc_start + KITTY_APC_START.len() + controls.len();
    let mut repaired = Vec::with_capacity(packet.len() + placement.len());
    repaired.extend_from_slice(&packet[..controls_end]);
    repaired.extend_from_slice(placement.as_bytes());
    repaired.extend_from_slice(&packet[controls_end..]);
    repaired
}

fn scaled_cells(source_px: u32, available_px: u32, available_cells: u16) -> u16 {
    let scaled = (u64::from(source_px) * u64::from(available_cells) + u64::from(available_px) / 2)
        / u64::from(available_px);
    u16::try_from(scaled)
        .unwrap_or(u16::MAX)
        .clamp(1, available_cells.max(1))
}

fn bounded_direct_geometry(geometry: MpvKittyGeometry) -> MpvKittyGeometry {
    if geometry.width_px <= DIRECT_KITTY_MAX_WIDTH && geometry.height_px <= DIRECT_KITTY_MAX_HEIGHT
    {
        return geometry;
    }
    let scale = (f64::from(DIRECT_KITTY_MAX_WIDTH) / f64::from(geometry.width_px))
        .min(f64::from(DIRECT_KITTY_MAX_HEIGHT) / f64::from(geometry.height_px));
    MpvKittyGeometry {
        cols: geometry.cols,
        rows: geometry.rows,
        width_px: (f64::from(geometry.width_px) * scale).round().max(1.0) as u32,
        height_px: (f64::from(geometry.height_px) * scale).round().max(1.0) as u32,
    }
}

fn kitty_param(controls: &[u8], expected: &[u8]) -> bool {
    controls
        .split(|byte| *byte == b',')
        .any(|parameter| parameter == expected)
}

fn kitty_param_prefix<'a>(controls: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    controls
        .split(|byte| *byte == b',')
        .find_map(|parameter| parameter.strip_prefix(prefix))
}

fn kitty_param_u32(controls: &[u8], prefix: &[u8]) -> Option<u32> {
    std::str::from_utf8(kitty_param_prefix(controls, prefix)?)
        .ok()?
        .parse()
        .ok()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|part| part == needle)
}

fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|part| part == needle)
}

pub struct PlayerManager {
    process: Child,
    kitty_output: Option<MpvKittyOutput>,
    _socket_dir: TempDir,
    socket_path: PathBuf,
    ipc: Option<IpcClient>,
    options: PlaybackOptions,
    pub status: PlaybackStatus,
    pub current_video_id: Option<String>,
    current_playlist_entry_id: Option<i64>,
    kitty_video: bool,
    kitty_geometry: Option<MpvKittyGeometry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlaybackOptions {
    audio_only: bool,
    format: String,
    native_kitty: bool,
    kitty_shm: bool,
}

impl From<&Config> for PlaybackOptions {
    fn from(config: &Config) -> Self {
        // Audio owns the durable playback clock. Video is always handled by
        // VideoState's independent yt-dlp/ffmpeg pipeline, including on local
        // Kitty terminals, so a video format can never delay or break audio.
        Self::from_config(config, false, false)
    }
}

impl PlaybackOptions {
    fn from_config(config: &Config, _native_kitty: bool, _kitty_shm: bool) -> Self {
        Self {
            audio_only: true,
            format: config.queue_audio_format(),
            native_kitty: false,
            kitty_shm: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackStatus {
    pub playing: bool,
    pub paused: bool,
    pub time_pos: f64,
    pub duration: f64,
    pub volume: i32,
    pub title: String,
    pub eof_reached: bool,
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        Self {
            playing: false,
            paused: false,
            time_pos: 0.0,
            duration: 0.0,
            volume: 100,
            title: String::new(),
            eof_reached: false,
        }
    }
}

impl PlaybackStatus {
    fn apply_property_values(&mut self, values: &[Option<Value>]) {
        if let Some(time) = values
            .first()
            .and_then(Option::as_ref)
            .and_then(Value::as_f64)
        {
            self.time_pos = time.max(0.0);
        }

        if let Some(duration) = values
            .get(1)
            .and_then(Option::as_ref)
            .and_then(Value::as_f64)
            && duration > 0.0
            && (self.duration == 0.0 || duration > self.duration)
        {
            self.duration = duration;
        }

        if let Some(paused) = values
            .get(2)
            .and_then(Option::as_ref)
            .and_then(Value::as_bool)
        {
            self.paused = paused;
        }

        if let Some(volume) = values
            .get(3)
            .and_then(Option::as_ref)
            .and_then(Value::as_f64)
        {
            self.volume = (volume as i32).clamp(0, 100);
        }

        if let Some(eof) = values
            .get(4)
            .and_then(Option::as_ref)
            .and_then(Value::as_bool)
        {
            self.eof_reached = eof;
        }
    }

    fn mark_transport_error(&mut self) {
        self.playing = false;
        self.paused = false;
        self.eof_reached = false;
    }

    fn mark_eof(&mut self) {
        self.playing = false;
        self.paused = false;
        self.eof_reached = true;
    }
}

impl PlayerManager {
    pub fn new(config: &Config) -> Result<Self> {
        // A private directory avoids collisions between multiple managers and
        // stale socket files left by a previously crashed process.
        let socket_dir = tempfile::Builder::new()
            .prefix("youtui-mpv-")
            .tempdir()
            .context("Failed to create mpv IPC directory")?;
        let socket_path = socket_dir.path().join("mpv.sock");

        let options = PlaybackOptions::from(config);
        let mut cmd = build_mpv_command(&socket_path, &options);

        let mut process = cmd.spawn().context("Failed to spawn mpv process")?;
        let kitty_output = if options.native_kitty {
            let stdout = process
                .stdout
                .take()
                .expect("native Kitty mpv stdout must be piped");
            match MpvKittyOutput::spawn(stdout, options.kitty_shm) {
                Ok(output) => Some(output),
                Err(error) => {
                    terminate_player(&mut process);
                    return Err(error);
                }
            }
        } else {
            None
        };

        Ok(Self {
            process,
            kitty_output,
            _socket_dir: socket_dir,
            socket_path,
            ipc: None,
            options,
            status: PlaybackStatus::default(),
            current_video_id: None,
            current_playlist_entry_id: None,
            kitty_video: false,
            kitty_geometry: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_test_stream(stream: std::os::unix::net::UnixStream) -> Self {
        let socket_dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        Self {
            process: Command::new("sleep").arg("5").spawn().unwrap(),
            kitty_output: None,
            socket_path: socket_dir.path().join("mpv.sock"),
            _socket_dir: socket_dir,
            ipc: Some(IpcClient::from_stream(stream).unwrap()),
            options: PlaybackOptions::from(&config),
            status: PlaybackStatus {
                playing: true,
                ..PlaybackStatus::default()
            },
            current_video_id: Some("video-id".to_string()),
            current_playlist_entry_id: Some(1),
            kitty_video: false,
            kitty_geometry: None,
        }
    }

    pub fn connect(&mut self) -> Result<()> {
        let start = Instant::now();

        loop {
            if let Some(status) = self
                .process
                .try_wait()
                .context("Failed to inspect mpv process")?
            {
                bail!("mpv exited before its IPC socket was ready ({status})");
            }

            let connection_error = match IpcClient::connect(&self.socket_path) {
                Ok(ipc) => {
                    self.ipc = Some(ipc);
                    return Ok(());
                }
                Err(error) => error,
            };

            if start.elapsed() >= CONNECT_TIMEOUT {
                bail!("mpv IPC socket was not ready after 2 seconds: {connection_error}");
            }
            std::thread::sleep(CONNECT_RETRY_DELAY);
        }
    }

    pub fn play(&mut self, config: &Config, url: &str, title: &str, video_id: &str) -> Result<()> {
        self.apply_runtime_config(config)?;
        if self.ipc.is_none() {
            self.connect()?;
        }

        let ipc = self
            .ipc
            .as_mut()
            .context("mpv IPC connection was not initialized")?;
        let response = ipc.send_command_with_data(&loadfile_command(url, false))?;

        self.status.title = title.to_string();
        self.status.playing = true;
        self.status.paused = false;
        self.status.eof_reached = false;
        self.status.time_pos = 0.0;
        self.status.duration = 0.0;
        self.current_video_id = Some(video_id.to_string());
        self.current_playlist_entry_id = playlist_entry_id(response.as_ref());

        Ok(())
    }

    pub fn load_paused(
        &mut self,
        config: &Config,
        url: &str,
        title: &str,
        video_id: &str,
    ) -> Result<()> {
        self.apply_runtime_config(config)?;
        if self.ipc.is_none() {
            self.connect()?;
        }

        let ipc = self
            .ipc
            .as_mut()
            .context("mpv IPC connection was not initialized")?;
        let response = ipc.send_command_with_data(&loadfile_command(url, true))?;

        self.status.title = title.to_string();
        self.status.playing = true;
        self.status.paused = true;
        self.status.eof_reached = false;
        self.status.time_pos = 0.0;
        self.status.duration = 0.0;
        self.current_video_id = Some(video_id.to_string());
        self.current_playlist_entry_id = playlist_entry_id(response.as_ref());

        Ok(())
    }

    fn apply_runtime_config(&mut self, config: &Config) -> Result<()> {
        let desired = PlaybackOptions::from(config);
        if self.options != desired {
            // mpv's audio/video and yt-dlp format options belong to the player
            // process. Recreate it before the next load so settings changed in
            // the TUI take effect without requiring an application restart.
            *self = Self::new(config)?;
        }
        Ok(())
    }

    fn reconnect_for_active_track(&mut self) -> Result<()> {
        if self.ipc.is_none() && self.current_video_id.is_some() {
            self.connect()?;
        }
        Ok(())
    }

    pub fn clear(&mut self) -> Result<()> {
        self.reconnect_for_active_track()?;
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["stop"])?;
        }

        self.status = PlaybackStatus::default();
        self.current_video_id = None;
        self.current_playlist_entry_id = None;

        Ok(())
    }

    pub fn toggle_pause(&mut self) -> Result<()> {
        self.reconnect_for_active_track()?;
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["cycle", "pause"])?;
            self.status.paused = !self.status.paused;
        }
        Ok(())
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        if !seconds.is_finite() {
            bail!("Seek offset must be a finite number");
        }
        self.reconnect_for_active_track()?;
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["seek", &seconds.to_string(), "relative"])?;
        }
        Ok(())
    }

    pub fn seek_absolute(&mut self, seconds: f64) -> Result<()> {
        if !seconds.is_finite() {
            bail!("Seek position must be a finite number");
        }
        self.reconnect_for_active_track()?;
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["seek", &seconds.to_string(), "absolute"])?;
        }
        Ok(())
    }

    pub fn set_volume(&mut self, volume: i32) -> Result<()> {
        let volume = volume.clamp(0, 100);
        self.reconnect_for_active_track()?;
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["set_property", "volume", &volume.to_string()])?;
            self.status.volume = volume;
        }
        Ok(())
    }

    pub fn native_kitty_enabled(&self) -> bool {
        self.options.native_kitty
    }

    /// Return the newest complete mpv terminal frame. Stale frames are
    /// replaced by the reader thread so neither direct packets nor reusable
    /// shared-memory references can build up behind the TUI.
    pub fn take_kitty_output(&mut self) -> Vec<u8> {
        let Some(frame) = self
            .kitty_output
            .as_ref()
            .and_then(MpvKittyOutput::take_frame)
        else {
            return Vec::new();
        };
        if self.options.kitty_shm {
            frame
        } else if let Some(geometry) = self.kitty_geometry {
            add_direct_kitty_placement(frame, geometry)
        } else {
            Vec::new()
        }
    }

    /// Switch the existing mpv process between its warm, hidden `null` output
    /// and the Kitty graphics output. mpv remains the audio clock and
    /// decoder in both states, so toggling does not open or seek a second
    /// YouTube stream.
    pub fn set_kitty_video(&mut self, visible: bool, geometry: MpvKittyGeometry) -> Result<()> {
        if visible && !self.options.native_kitty {
            bail!("native Kitty video output is unavailable for this player");
        }
        self.reconnect_for_active_track()?;
        let Some(ipc) = self.ipc.as_mut() else {
            return Ok(());
        };

        if visible {
            let geometry = if self.options.kitty_shm {
                geometry
            } else {
                bounded_direct_geometry(geometry)
            };
            let geometry_changed = self.kitty_geometry != Some(geometry);
            if geometry_changed && self.kitty_video {
                // Kitty VO reads geometry during initialization. Recreate it
                // only on an actual pane resize so ordinary status updates do
                // not disturb playback.
                ipc.send_command(&["set_property", "vo", "null"])?;
                self.kitty_video = false;
            }
            if geometry_changed {
                let cols = geometry.cols.to_string();
                let rows = geometry.rows.to_string();
                let width = geometry.width_px.to_string();
                let height = geometry.height_px.to_string();
                ipc.send_commands(&[
                    &["set_property", "vo-kitty-cols", &cols],
                    &["set_property", "vo-kitty-rows", &rows],
                    &["set_property", "vo-kitty-width", &width],
                    &["set_property", "vo-kitty-height", &height],
                ])?;
                self.kitty_geometry = Some(geometry);
            }
            if !self.kitty_video {
                ipc.send_command(&["set_property", "vo", "kitty"])?;
                self.kitty_video = true;
            }
        } else if self.kitty_video {
            ipc.send_command(&["set_property", "vo", "null"])?;
            self.kitty_video = false;
        }
        Ok(())
    }

    pub fn update_status(&mut self) -> Result<()> {
        if self.ipc.is_none()
            && self.current_video_id.is_some()
            && let Err(error) = self.connect()
        {
            self.status.mark_transport_error();
            return Err(error);
        }

        if let Some(ipc) = self.ipc.as_mut() {
            let poll_result = ipc.get_properties(&STATUS_PROPERTIES);
            let events = ipc.take_events();
            let reached_eof = events
                .iter()
                .any(|event| is_current_eof_event(event, self.current_playlist_entry_id));
            let load_error = events
                .iter()
                .find_map(|event| current_load_error(event, self.current_playlist_entry_id));

            if let Some(error) = load_error {
                self.status.mark_transport_error();
                self.current_video_id = None;
                self.current_playlist_entry_id = None;
                bail!("mpv could not load the audio stream: {error}");
            }

            match poll_result {
                Ok(values) => {
                    self.status.apply_property_values(&values);
                    if reached_eof {
                        self.status.mark_eof();
                    }
                }
                Err(error) if IpcClient::is_read_timeout(&error) => {
                    // Loading a YouTube URL can briefly block mpv's command
                    // loop. A timed-out read may also have consumed part of a
                    // JSON frame, so reconnect with a fresh client next poll
                    // while preserving the healthy process and queue item.
                    self.ipc = None;
                    if reached_eof {
                        self.status.mark_eof();
                    }
                    return Ok(());
                }
                Err(error) => {
                    self.ipc = None;
                    if reached_eof {
                        self.status.mark_eof();
                        return Ok(());
                    }

                    // Transport failure is not media EOF. Keep the queue item
                    // available for retry instead of silently consuming it.
                    self.status.mark_transport_error();
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub fn is_eof(&mut self) -> bool {
        if self.status.eof_reached {
            self.status.eof_reached = false; // consume so the same EOF fires only once
            true
        } else {
            false
        }
    }
}

fn build_mpv_command(socket_path: &Path, options: &PlaybackOptions) -> Command {
    let mut command = Command::new("mpv");
    #[cfg(unix)]
    command.process_group(0);
    if let Some(directory) = socket_path.parent() {
        // Keep packaged yt-dlp extraction files inside the player's owned directory.
        command
            .env("TMPDIR", directory)
            .env("TMP", directory)
            .env("TEMP", directory);
    }
    command
        .arg("--idle")
        .arg(format!("--input-ipc-server={}", socket_path.display()))
        .arg(format!("--ytdl-format={}", options.format))
        .arg("--terminal=no")
        .arg("--input-terminal=no");

    if options.native_kitty {
        // Keep video decoded by the same player as audio, but render it to a
        // sink until the user opens the video pane. Switching `vo` to Kitty is
        // then immediate and never creates an OS window.
        command
            .arg("--vo=null")
            .arg("--profile=sw-fast")
            .arg("--osd-level=0")
            .arg(format!(
                "--vo-kitty-use-shm={}",
                if options.kitty_shm { "yes" } else { "no" }
            ))
            .arg("--vo-kitty-alt-screen=no")
            .arg("--vo-kitty-config-clear=no");
        if !options.kitty_shm {
            // Direct Kitty frames cross stdout as base64 RGB. Bound their
            // cadence as well as their geometry, then let the terminal scale
            // each placement to the full pane.
            command.arg("--vf=fps=24");
        }
    } else {
        // Portable video renderers use youtui's ffmpeg pipeline. Avoid warming
        // a second copy of the video in mpv when its Kitty VO cannot be used.
        command.arg("--no-video");
    }

    command.stdin(Stdio::null()).stderr(Stdio::null());
    if options.native_kitty {
        // Capture Kitty commands for frame coalescing and ordered writes
        // through Ratatui's stdout owner.
        command.stdout(Stdio::piped());
    } else {
        command.stdout(Stdio::null());
    }
    command
}

fn loadfile_command(url: &str, paused: bool) -> Vec<&str> {
    if paused {
        // mpv 0.38+ places the optional insertion index before load options.
        vec!["loadfile", url, "replace", "-1", "pause=yes"]
    } else {
        vec!["loadfile", url, "replace"]
    }
}

fn playlist_entry_id(data: Option<&Value>) -> Option<i64> {
    data.and_then(|value| value.get("playlist_entry_id"))
        .and_then(Value::as_i64)
}

fn is_current_eof_event(event: &Value, current_playlist_entry_id: Option<i64>) -> bool {
    let Some(current_playlist_entry_id) = current_playlist_entry_id else {
        return false;
    };

    event.get("event").and_then(Value::as_str) == Some("end-file")
        && event.get("reason").and_then(Value::as_str) == Some("eof")
        && event.get("playlist_entry_id").and_then(Value::as_i64) == Some(current_playlist_entry_id)
}

fn current_load_error(event: &Value, current_playlist_entry_id: Option<i64>) -> Option<String> {
    let current_playlist_entry_id = current_playlist_entry_id?;
    if event.get("event").and_then(Value::as_str) != Some("end-file")
        || event.get("reason").and_then(Value::as_str) != Some("error")
        || event.get("playlist_entry_id").and_then(Value::as_i64) != Some(current_playlist_entry_id)
    {
        return None;
    }

    Some(
        event
            .get("file_error")
            .and_then(Value::as_str)
            .unwrap_or("unknown media error")
            .to_string(),
    )
}

fn terminate_player(process: &mut Child) {
    #[cfg(unix)]
    if let Ok(process_group) = i32::try_from(process.id()) {
        // SAFETY: mpv runs in a fresh group whose ID is its PID. This also
        // terminates any yt-dlp descendants before their temp directory drops.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
    let _ = process.kill();
    let _ = process.wait();
}
impl Drop for PlayerManager {
    fn drop(&mut self) {
        // Closing IPC before terminating mpv avoids keeping the socket alive.
        self.ipc.take();
        terminate_player(&mut self.process);
        if let Some(output) = self.kitty_output.as_mut() {
            output.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::thread;

    fn command_args(config: &Config, native_kitty: bool, kitty_shm: bool) -> Vec<String> {
        let options = PlaybackOptions::from_config(config, native_kitty, kitty_shm);
        build_mpv_command(Path::new("/tmp/test.sock"), &options)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn mpv_shm_packet_restores_the_posix_name_slash() {
        let bare_name = b"mpv-kitty-0xa19d35cd0";
        let bare_payload = base64_simd::STANDARD.encode_to_string(bare_name);
        let packet =
            format!("\x1b[0;4f\x1b_Ga=T,t=s,f=24,s=1877,v=1056,C=1,q=2,m=1;{bare_payload}\x1b\\")
                .into_bytes();

        let repaired = repair_mpv_shm_name(packet);
        let expected_payload = base64_simd::STANDARD.encode_to_string(b"/mpv-kitty-0xa19d35cd0");

        assert!(
            repaired
                .windows(expected_payload.len())
                .any(|window| window == expected_payload.as_bytes())
        );
        assert!(is_mpv_kitty_frame(&repaired));
    }

    #[test]
    fn kitty_packet_repair_ignores_direct_and_non_mpv_payloads() {
        let direct = b"\x1b_Ga=T,t=d,f=24,s=1,v=1;AAAA\x1b\\".to_vec();
        let other_shm = format!(
            "\x1b_Ga=T,t=s,f=24,s=1,v=1;{}\x1b\\",
            base64_simd::STANDARD.encode_to_string(b"other-client")
        )
        .into_bytes();

        assert_eq!(repair_mpv_shm_name(direct.clone()), direct);
        assert_eq!(repair_mpv_shm_name(other_shm.clone()), other_shm);
    }

    #[test]
    fn kitty_output_retains_only_the_newest_complete_frame() {
        let first = b"\x1b_Ga=T,t=s,f=24,s=1,v=1;Zmlyc3Q=\x1b\\".to_vec();
        let second = b"\x1b_Ga=T,t=s,f=24,s=1,v=1;c2Vjb25k\x1b\\".to_vec();
        let output = MpvKittyOutput {
            latest_frame: Arc::new(Mutex::new(Some(first))),
            reader: None,
        };
        publish_latest_frame(&output.latest_frame, second.clone());

        assert_eq!(output.take_frame(), Some(second));
        assert!(output.take_frame().is_none());
    }

    #[test]
    fn direct_kitty_reader_publishes_one_complete_chunked_frame() {
        let initial = b"\x1b[?25l\x1b[0;4f\x1b_Ga=T,t=d,f=24,s=2,v=2,m=1;AAAA\x1b\\";
        let continuation = b"\x1b_Gm=0;BBBB\x1b\\";
        let stream = [initial.as_slice(), continuation.as_slice()].concat();
        let latest = Arc::new(Mutex::new(None));

        pump_mpv_kitty_output(std::io::Cursor::new(stream), &latest, false);

        let expected = [
            b"\x1b[0;4f".as_slice(),
            b"\x1b_Ga=T,t=d,f=24,s=2,v=2,m=1;AAAA\x1b\\".as_slice(),
            continuation.as_slice(),
        ]
        .concat();
        assert_eq!(latest.lock().unwrap().as_deref(), Some(expected.as_slice()));
    }

    #[test]
    fn direct_kitty_reader_accepts_an_unchunked_frame_without_m_parameter() {
        let frame = b"\x1b[0;0f\x1b_Ga=T,f=24,s=1,v=1;AAAA\x1b\\";
        let latest = Arc::new(Mutex::new(None));

        pump_mpv_kitty_output(std::io::Cursor::new(frame), &latest, false);

        assert_eq!(latest.lock().unwrap().as_deref(), Some(frame.as_slice()));
    }

    #[test]
    fn direct_kitty_frame_is_scaled_to_the_full_pane() {
        let packet =
            b"\x1b[0;4f\x1b_Ga=T,f=24,s=460,v=259,C=1,q=2,m=1;AAAA\x1b\\\x1b_Gm=0;BBBB\x1b\\"
                .to_vec();
        let geometry = MpvKittyGeometry {
            cols: 196,
            rows: 44,
            width_px: 480,
            height_px: 259,
        };

        let placed = add_direct_kitty_placement(packet, geometry);

        assert!(placed.windows(11).any(|window| window == b",c=188,r=44"));
    }

    #[test]
    fn direct_kitty_geometry_is_bounded_without_changing_cells() {
        let geometry = MpvKittyGeometry {
            cols: 196,
            rows: 44,
            width_px: 1_960,
            height_px: 1_056,
        };

        assert_eq!(
            bounded_direct_geometry(geometry),
            MpvKittyGeometry {
                cols: 196,
                rows: 44,
                width_px: 480,
                height_px: 259,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn player_shutdown_terminates_extractor_descendants() {
        let fixture = tempfile::tempdir().unwrap();
        let ready = fixture.path().join("ready");
        let escaped = fixture.path().join("escaped");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                r#"printf ready > "$1"
(sleep 1; printf escaped > "$2") &
wait"#,
                "player-fixture",
            ])
            .arg(&ready)
            .arg(&escaped)
            .process_group(0);
        let mut process = command.spawn().unwrap();
        let started = Instant::now();
        while !ready.exists() {
            if started.elapsed() >= Duration::from_secs(3) {
                terminate_player(&mut process);
                panic!("player fixture did not start");
            }
            thread::sleep(Duration::from_millis(10));
        }
        terminate_player(&mut process);
        assert!(process.try_wait().unwrap().is_some());
        thread::sleep(Duration::from_millis(1100));
        assert!(
            !escaped.exists(),
            "audio extractor survived player shutdown"
        );
    }

    #[test]
    fn absolute_seek_sends_absolute_ipc_command() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(server_stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            writeln!(
                &server_stream,
                "{}",
                json!({
                    "request_id": request["request_id"],
                    "error": "success",
                })
            )
            .unwrap();
            request["command"].clone()
        });
        let mut manager = PlayerManager::from_test_stream(client_stream);

        manager.seek_absolute(83.5).unwrap();

        assert_eq!(server.join().unwrap(), json!(["seek", "83.5", "absolute"]));
    }

    #[test]
    fn kitty_video_switch_batches_geometry_then_changes_the_live_vo() {
        let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
        let server_reader = server_stream.try_clone().unwrap();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(server_reader);
            let mut commands = Vec::new();

            // Geometry is one IPC batch, so read all four requests before
            // replying just as a real mpv command loop may do.
            let mut geometry_requests = Vec::new();
            for _ in 0..4 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                commands.push(request["command"].clone());
                geometry_requests.push(request);
            }
            for request in geometry_requests {
                writeln!(
                    server_stream,
                    "{}",
                    json!({
                        "request_id": request["request_id"],
                        "error": "success",
                    })
                )
                .unwrap();
            }

            for _ in 0..2 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                commands.push(request["command"].clone());
                writeln!(
                    server_stream,
                    "{}",
                    json!({
                        "request_id": request["request_id"],
                        "error": "success",
                    })
                )
                .unwrap();
            }
            commands
        });

        let mut manager = PlayerManager::from_test_stream(client_stream);
        manager.options.native_kitty = true;
        manager.options.kitty_shm = true;
        let geometry = MpvKittyGeometry {
            cols: 100,
            rows: 30,
            width_px: 800,
            height_px: 480,
        };

        manager.set_kitty_video(true, geometry).unwrap();
        manager.set_kitty_video(false, geometry).unwrap();

        assert_eq!(
            server.join().unwrap(),
            vec![
                json!(["set_property", "vo-kitty-cols", "100"]),
                json!(["set_property", "vo-kitty-rows", "30"]),
                json!(["set_property", "vo-kitty-width", "800"]),
                json!(["set_property", "vo-kitty-height", "480"]),
                json!(["set_property", "vo", "kitty"]),
                json!(["set_property", "vo", "null"]),
            ]
        );
        assert!(!manager.kitty_video);
        assert_eq!(manager.kitty_geometry, Some(geometry));
    }

    #[test]
    fn queue_mpv_is_audio_only_regardless_of_video_configuration() {
        let video = Config {
            bandwidth_limit: true,
            ..Config::default()
        };
        let args = command_args(&video, true, true);
        assert!(args.iter().any(|arg| arg == "--no-video"));
        assert!(!args.iter().any(|arg| arg == "--vo=null"));
        assert!(!args.iter().any(|arg| arg.starts_with("--vo-kitty")));
        assert!(
            args.iter()
                .any(|arg| arg == "--ytdl-format=bestaudio[abr<=128]/bestaudio/best")
        );

        let direct_args = command_args(&video, true, false);
        assert!(direct_args.iter().any(|arg| arg == "--no-video"));
        assert!(!direct_args.iter().any(|arg| arg == "--vf=fps=24"));

        let audio = Config {
            audio_only: true,
            custom_format: "custom-audio-format".to_string(),
            ..Config::default()
        };
        let args = command_args(&audio, false, false);
        assert!(args.iter().any(|arg| arg == "--no-video"));
        assert!(args.iter().any(|arg| arg == "--ytdl-format=bestaudio/best"));
    }

    #[test]
    fn playback_options_detect_runtime_configuration_changes() {
        let initial = PlaybackOptions::from(&Config::default());
        let audio = PlaybackOptions::from(&Config {
            audio_only: true,
            ..Config::default()
        });
        let custom = PlaybackOptions::from(&Config {
            custom_format: "best".to_string(),
            ..Config::default()
        });

        let limited = PlaybackOptions::from(&Config {
            bandwidth_limit: true,
            ..Config::default()
        });

        assert_eq!(initial, audio);
        assert_eq!(initial, custom);
        assert_ne!(initial, limited);
    }

    #[test]
    fn paused_load_uses_the_current_mpv_argument_order() {
        assert_eq!(
            loadfile_command("video-url", true),
            ["loadfile", "video-url", "replace", "-1", "pause=yes"]
        );
        assert_eq!(
            loadfile_command("video-url", false),
            ["loadfile", "video-url", "replace"]
        );
    }

    #[test]
    fn status_updates_clamp_values_and_preserve_known_duration() {
        let mut status = PlaybackStatus {
            duration: 120.0,
            ..PlaybackStatus::default()
        };

        status.apply_property_values(&[
            Some(json!(-0.5)),
            Some(json!(100.0)),
            Some(json!(true)),
            Some(json!(150.0)),
            Some(json!(false)),
        ]);

        assert_eq!(status.time_pos, 0.0);
        assert_eq!(status.duration, 120.0);
        assert!(status.paused);
        assert_eq!(status.volume, 100);
        assert!(!status.eof_reached);
    }

    #[test]
    fn status_updates_accept_larger_positive_duration() {
        let mut status = PlaybackStatus::default();
        status.apply_property_values(&[
            Some(json!(3.0)),
            Some(json!(240.0)),
            None,
            Some(json!(-10.0)),
            Some(json!(true)),
        ]);

        assert_eq!(status.time_pos, 3.0);
        assert_eq!(status.duration, 240.0);
        assert_eq!(status.volume, 0);
        assert!(status.eof_reached);
    }

    #[test]
    fn transport_error_stops_status_without_synthesizing_eof() {
        let mut status = PlaybackStatus {
            playing: true,
            paused: true,
            ..PlaybackStatus::default()
        };

        status.mark_transport_error();

        assert!(!status.playing);
        assert!(!status.paused);
        assert!(!status.eof_reached);
    }

    #[test]
    fn eof_events_are_correlated_to_the_current_playlist_entry() {
        let current_eof = json!({
            "event": "end-file",
            "reason": "eof",
            "playlist_entry_id": 42,
        });
        let old_eof = json!({
            "event": "end-file",
            "reason": "eof",
            "playlist_entry_id": 41,
        });
        let replaced = json!({
            "event": "end-file",
            "reason": "stop",
            "playlist_entry_id": 42,
        });

        assert!(is_current_eof_event(&current_eof, Some(42)));
        assert!(!is_current_eof_event(&old_eof, Some(42)));
        assert!(!is_current_eof_event(&replaced, Some(42)));
        assert!(!is_current_eof_event(&current_eof, None));
    }

    #[test]
    fn media_errors_are_correlated_and_expose_mpv_details() {
        let current_error = json!({
            "event": "end-file",
            "reason": "error",
            "file_error": "HTTP 403",
            "playlist_entry_id": 42,
        });

        assert_eq!(
            current_load_error(&current_error, Some(42)).as_deref(),
            Some("HTTP 403")
        );
        assert!(current_load_error(&current_error, Some(41)).is_none());
        assert!(current_load_error(&current_error, None).is_none());
    }

    #[test]
    fn loadfile_response_exposes_playlist_entry_identity() {
        let response = json!({ "playlist_entry_id": 17 });
        assert_eq!(playlist_entry_id(Some(&response)), Some(17));
        assert_eq!(playlist_entry_id(None), None);
    }

    #[test]
    fn status_timeout_preserves_track_and_recovers_with_a_fresh_connection() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let partial_server = thread::spawn(move || {
            let mut reader = BufReader::new(server_stream.try_clone().unwrap());
            let mut requests = Vec::new();
            for _ in 0..STATUS_PROPERTIES.len() {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                requests.push(serde_json::from_str::<Value>(&line).unwrap());
            }

            let mut writer = server_stream;
            write!(
                writer,
                "{{\"request_id\":{},\"error\":\"success\"",
                requests[0]["request_id"].as_u64().unwrap()
            )
            .unwrap();
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(250));
        });

        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("mpv.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let reconnect_server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            for _ in 0..STATUS_PROPERTIES.len() {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let property = request["command"][1].as_str().unwrap();
                let data = match property {
                    "time-pos" => json!(12.0),
                    "duration" => json!(100.0),
                    "pause" => json!(false),
                    "volume" => json!(55.0),
                    "eof-reached" => json!(false),
                    _ => Value::Null,
                };
                writeln!(
                    writer,
                    "{}",
                    json!({
                        "request_id": request["request_id"],
                        "error": "success",
                        "data": data,
                    })
                )
                .unwrap();
            }
        });

        let config = Config::default();
        let mut manager = PlayerManager {
            process: Command::new("sleep").arg("5").spawn().unwrap(),
            kitty_output: None,
            _socket_dir: socket_dir,
            socket_path,
            ipc: Some(IpcClient::from_stream(client_stream).unwrap()),
            options: PlaybackOptions::from(&config),
            status: PlaybackStatus {
                playing: true,
                title: "Current track".to_string(),
                ..PlaybackStatus::default()
            },
            current_video_id: Some("video-id".to_string()),
            current_playlist_entry_id: Some(7),
            kitty_video: false,
            kitty_geometry: None,
        };

        manager.update_status().unwrap();
        assert!(manager.ipc.is_none());
        assert_eq!(manager.current_video_id.as_deref(), Some("video-id"));
        assert!(manager.status.playing);
        assert!(!manager.status.eof_reached);
        assert!(!manager.is_eof());
        partial_server.join().unwrap();

        manager.update_status().unwrap();
        assert!(manager.ipc.is_some());
        assert_eq!(manager.status.time_pos, 12.0);
        assert_eq!(manager.status.duration, 100.0);
        assert_eq!(manager.status.volume, 55);
        assert_eq!(manager.current_video_id.as_deref(), Some("video-id"));
        reconnect_server.join().unwrap();
    }
}
