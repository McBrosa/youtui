use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use colored::Colorize;

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerType {
    Mpv,
    Vlc,
    Mplayer,
}

impl std::fmt::Display for PlayerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlayerType::Mpv => write!(f, "mpv"),
            PlayerType::Vlc => write!(f, "vlc"),
            PlayerType::Mplayer => write!(f, "mplayer"),
        }
    }
}

#[allow(dead_code)]
pub enum PlaybackResult {
    Finished,
    ReturnToMenu,
    Error(String),
}

pub fn supports_background_playback(player: PlayerType) -> bool {
    matches!(player, PlayerType::Mpv)
}

pub fn detect_player() -> Result<PlayerType> {
    if which::which("mpv").is_ok() {
        Ok(PlayerType::Mpv)
    } else if which::which("vlc").is_ok() {
        Ok(PlayerType::Vlc)
    } else if which::which("mplayer").is_ok() {
        Ok(PlayerType::Mplayer)
    } else {
        bail!("No supported media player found (mpv, vlc, mplayer)\nPlease install one of these players to continue")
    }
}

pub fn play_video(
    config: &Config,
    video_id: &str,
    video_title: &str,
    safe_title: &str,
    temp_dir: &Path,
) -> Result<PlaybackResult> {
    let url = format!("https://www.youtube.com/watch?v={}", video_id);

    if config.download_mode {
        return download_permanently(config, video_title, &url);
    }

    match config.player {
        PlayerType::Mpv => play_with_mpv(config, &url, temp_dir),
        PlayerType::Vlc => play_with_download(config, video_title, safe_title, &url, temp_dir, "vlc"),
        PlayerType::Mplayer => play_with_download(config, video_title, safe_title, &url, temp_dir, "mplayer"),
    }
}

fn download_permanently(config: &Config, video_title: &str, url: &str) -> Result<PlaybackResult> {
    println!("{} {}", "Downloading:".blue(), video_title);

    let output_template = format!("{}/%(title)s.%(ext)s", config.download_dir);

    let mut cmd = Command::new("yt-dlp");
    cmd.arg("-f").arg(&config.format);

    if config.audio_only {
        cmd.arg("-x")
            .arg("--audio-format").arg("mp3")
            .arg("--audio-quality").arg("0");
    }

    cmd.arg("-o").arg(&output_template).arg(url);

    let status = cmd.status().context("Failed to run yt-dlp for download")?;

    if !status.success() {
        eprintln!("{} Download failed.", "Error:".red());
        return Ok(PlaybackResult::ReturnToMenu);
    }

    println!("{} {}", "Downloaded to:".green(), config.download_dir);
    Ok(PlaybackResult::ReturnToMenu)
}

fn play_with_mpv(config: &Config, url: &str, temp_dir: &Path) -> Result<PlaybackResult> {
    println!("{}", "Playing with mpv...".blue());

    let input_conf = temp_dir.join("mpv-input.conf");
    fs::write(&input_conf, "r quit 42\n")?;

    let mut cmd = Command::new("mpv");

    if config.audio_only {
        cmd.arg("--no-video");
    }

    cmd.arg(format!("--ytdl-format={}", config.format))
        .arg(format!("--input-conf={}", input_conf.display()))
        .arg(url);

    let status = cmd.status().context("Failed to run mpv")?;
    let code = status.code().unwrap_or(-1);

    match code {
        42 => Ok(PlaybackResult::ReturnToMenu),
        0 => {
            println!("{}", "Video finished. Returning to search results...".green());
            Ok(PlaybackResult::ReturnToMenu)
        }
        other => {
            println!(
                "{}",
                format!("Player exited with code {}. Returning to search results...", other).yellow()
            );
            Ok(PlaybackResult::ReturnToMenu)
        }
    }
}

fn play_with_download(
    config: &Config,
    video_title: &str,
    safe_title: &str,
    url: &str,
    temp_dir: &Path,
    player_name: &str,
) -> Result<PlaybackResult> {
    println!("{} {}", "Downloading temporarily:".blue(), video_title);

    let ext = if config.audio_only { "mp3" } else { "mp4" };
    let output_path = temp_dir.join(format!("{}.{}", safe_title, ext));

    let mut cmd = Command::new("yt-dlp");
    cmd.arg("-f").arg(&config.format);

    if config.audio_only {
        cmd.arg("-x")
            .arg("--audio-format").arg("mp3")
            .arg("--audio-quality").arg("0");
    }

    cmd.arg("-o").arg(output_path.to_string_lossy().as_ref()).arg(url);

    let status = cmd.status().context("Failed to run yt-dlp for temporary download")?;

    if !status.success() {
        eprintln!("{} yt-dlp download failed with exit code {}", "Error:".red(), status.code().unwrap_or(-1));
        return Ok(PlaybackResult::ReturnToMenu);
    }

    // Find the actual downloaded file (yt-dlp may rename it)
    let downloaded_file = if output_path.exists() {
        output_path.clone()
    } else {
        find_downloaded_file(temp_dir)?
    };

    if !downloaded_file.exists() {
        eprintln!("{} Failed to download the video.", "Error:".red());
        return Ok(PlaybackResult::ReturnToMenu);
    }

    println!("{}", format!("Playing with {}...", player_name).blue());
    println!("{}", "File will be deleted after playback unless --keep was specified".yellow());

    let status = match player_name {
        "vlc" => Command::new("vlc")
            .arg("--play-and-exit")
            .arg("--no-video-title-show")
            .arg(&downloaded_file)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context("Failed to run vlc")?,
        "mplayer" => Command::new("mplayer")
            .arg("-quiet")
            .arg(&downloaded_file)
            .status()
            .context("Failed to run mplayer")?,
        _ => unreachable!(),
    };

    let _ = status; // VLC/mplayer always return to menu

    println!("{}", "Video finished. Returning to search results...".green());

    if !config.keep_temp && downloaded_file.exists() {
        let _ = fs::remove_file(&downloaded_file);
    }

    Ok(PlaybackResult::ReturnToMenu)
}

fn find_downloaded_file(temp_dir: &Path) -> Result<std::path::PathBuf> {
    let mut best: Option<std::path::PathBuf> = None;
    if let Ok(entries) = fs::read_dir(temp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext != "part" && ext != "conf" {
                        best = Some(path);
                        break;
                    }
                }
            }
        }
    }
    Ok(best.unwrap_or_else(|| temp_dir.join("notfound")))
}
