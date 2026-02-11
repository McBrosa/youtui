use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use anyhow::{Context, Result, bail};
use crate::ipc::IpcClient;

pub struct PlayerManager {
    process: Child,
    socket_path: PathBuf,
    ipc: Option<IpcClient>,
    pub status: PlaybackStatus,
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

impl PlayerManager {
    pub fn new() -> Result<Self> {
        let socket_path = PathBuf::from(format!(
            "/tmp/youtui-{}.sock",
            std::process::id()
        ));

        let mut cmd = Command::new("mpv");
        cmd.arg("--idle")
            .arg(format!("--input-ipc-server={}", socket_path.display()))
            .arg("--no-video")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let process = cmd.spawn()
            .context("Failed to spawn mpv process")?;

        Ok(Self {
            process,
            socket_path,
            ipc: None,
            status: PlaybackStatus::default(),
        })
    }

    pub fn connect(&mut self) -> Result<()> {
        let start = Instant::now();
        while !self.socket_path.exists() {
            if start.elapsed() > Duration::from_secs(2) {
                bail!("mpv socket not created after 2 seconds");
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        self.ipc = Some(IpcClient::connect(&self.socket_path)?);
        Ok(())
    }

    pub fn play(&mut self, url: &str, title: &str) -> Result<()> {
        if self.ipc.is_none() {
            self.connect()?;
        }

        let ipc = self.ipc.as_mut().unwrap();
        ipc.send_command(&["loadfile", url])?;

        self.status.title = title.to_string();
        self.status.playing = true;
        self.status.paused = false;
        self.status.eof_reached = false;

        Ok(())
    }

    pub fn toggle_pause(&mut self) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["cycle", "pause"])?;
            self.status.paused = !self.status.paused;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["stop"])?;
            self.status = PlaybackStatus::default();
        }
        Ok(())
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["seek", &seconds.to_string(), "relative"])?;
        }
        Ok(())
    }

    pub fn set_volume(&mut self, volume: i32) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.send_command(&["set_property", "volume", &volume.to_string()])?;
            self.status.volume = volume.clamp(0, 100);
        }
        Ok(())
    }

    pub fn update_status(&mut self) -> Result<()> {
        if let Some(ipc) = self.ipc.as_mut() {
            if let Ok(val) = ipc.get_property("time-pos") {
                if let Some(time) = val.as_f64() {
                    self.status.time_pos = time;
                }
            }

            if let Ok(val) = ipc.get_property("duration") {
                if let Some(dur) = val.as_f64() {
                    self.status.duration = dur;
                }
            }

            if let Ok(val) = ipc.get_property("pause") {
                if let Some(paused) = val.as_bool() {
                    self.status.paused = paused;
                }
            }

            if let Ok(val) = ipc.get_property("volume") {
                if let Some(vol) = val.as_f64() {
                    self.status.volume = vol as i32;
                }
            }

            if let Ok(val) = ipc.get_property("eof-reached") {
                if let Some(eof) = val.as_bool() {
                    self.status.eof_reached = eof;
                }
            }
        }
        Ok(())
    }

    pub fn is_eof(&self) -> bool {
        self.status.eof_reached
    }
}

impl Drop for PlayerManager {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
