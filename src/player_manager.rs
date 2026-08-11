use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tempfile::TempDir;

use crate::{config::Config, ipc::IpcClient};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(25);
const STATUS_PROPERTIES: [&str; 5] = ["time-pos", "duration", "pause", "volume", "eof-reached"];

pub struct PlayerManager {
    process: Child,
    _socket_dir: TempDir,
    socket_path: PathBuf,
    ipc: Option<IpcClient>,
    options: PlaybackOptions,
    pub status: PlaybackStatus,
    pub current_video_id: Option<String>,
    current_playlist_entry_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlaybackOptions {
    audio_only: bool,
    format: String,
}

impl From<&Config> for PlaybackOptions {
    fn from(config: &Config) -> Self {
        Self {
            audio_only: config.audio_only,
            format: config.format(),
        }
    }
}

#[derive(Debug, Clone)]
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

        let mut cmd = build_mpv_command(&socket_path, config);

        let process = cmd.spawn().context("Failed to spawn mpv process")?;

        Ok(Self {
            process,
            _socket_dir: socket_dir,
            socket_path,
            ipc: None,
            options: PlaybackOptions::from(config),
            status: PlaybackStatus::default(),
            current_video_id: None,
            current_playlist_entry_id: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_test_stream(stream: std::os::unix::net::UnixStream) -> Self {
        let socket_dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        Self {
            process: Command::new("sleep").arg("5").spawn().unwrap(),
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

fn build_mpv_command(socket_path: &Path, config: &Config) -> Command {
    let mut command = Command::new("mpv");
    command
        .arg("--idle")
        .arg(format!("--input-ipc-server={}", socket_path.display()))
        .arg(format!("--ytdl-format={}", config.format()));

    if config.audio_only {
        command.arg("--no-video");
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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

impl Drop for PlayerManager {
    fn drop(&mut self) {
        // Closing IPC before terminating mpv avoids keeping the socket alive.
        self.ipc.take();
        if !matches!(self.process.try_wait(), Ok(Some(_))) {
            let _ = self.process.kill();
        }
        // Always reap the child so repeated player creation cannot accumulate zombies.
        let _ = self.process.wait();
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

    fn command_args(config: &Config) -> Vec<String> {
        build_mpv_command(Path::new("/tmp/test.sock"), config)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
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
    fn mpv_command_respects_video_and_format_configuration() {
        let video = Config {
            bandwidth_limit: true,
            ..Config::default()
        };
        let args = command_args(&video);
        assert!(!args.iter().any(|arg| arg == "--no-video"));
        assert!(args.iter().any(|arg| {
            arg == "--ytdl-format=bestvideo[height<=360]+bestaudio/best[height<=360]/best"
        }));

        let audio = Config {
            audio_only: true,
            custom_format: "custom-audio-format".to_string(),
            ..Config::default()
        };
        let args = command_args(&audio);
        assert!(args.iter().any(|arg| arg == "--no-video"));
        assert!(
            args.iter()
                .any(|arg| arg == "--ytdl-format=custom-audio-format")
        );
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

        assert_ne!(initial, audio);
        assert_ne!(initial, custom);
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
