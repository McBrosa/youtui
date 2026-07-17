use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use colored::Colorize;

use crate::config::Config;

// Common Unix filesystems limit a single path component to 255 bytes. Reserve
// five bytes for media extensions such as `.webm`.
const MAX_DOWNLOAD_BASE_BYTES: usize = 250;
const MAX_VIDEO_ID_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum PlayerType {
    #[default]
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
        bail!(
            "No supported media player found (mpv, vlc, mplayer)\nPlease install one of these players to continue"
        )
    }
}

pub fn play_video(
    config: &Config,
    video_id: &str,
    video_title: &str,
    safe_title: &str,
    temp_dir: &Path,
) -> Result<PlaybackResult> {
    let url = format!("https://www.youtube.com/watch?v={video_id}");

    if config.download_mode {
        return download_permanently(config, video_title, &url);
    }

    match config.player {
        PlayerType::Mpv => play_with_mpv(config, &url, temp_dir),
        PlayerType::Vlc => play_with_download(
            config,
            video_id,
            video_title,
            safe_title,
            &url,
            temp_dir,
            "vlc",
        ),
        PlayerType::Mplayer => play_with_download(
            config,
            video_id,
            video_title,
            safe_title,
            &url,
            temp_dir,
            "mplayer",
        ),
    }
}

fn download_permanently(config: &Config, video_title: &str, url: &str) -> Result<PlaybackResult> {
    ensure_download_capabilities(config)?;
    println!("{} {}", "Downloading:".blue(), video_title);

    fs::create_dir_all(&config.download_dir).with_context(|| {
        format!(
            "Failed to create download directory {}",
            config.download_dir
        )
    })?;
    let output_template = Path::new(&config.download_dir).join("%(title)s.%(ext)s");

    let mut cmd = Command::new("yt-dlp");
    cmd.arg("-f").arg(config.format());

    if config.audio_only {
        cmd.arg("-x")
            .arg("--audio-format")
            .arg("mp3")
            .arg("--audio-quality")
            .arg("0");
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

    cmd.arg(format!("--ytdl-format={}", config.format()))
        .arg(format!("--input-conf={}", input_conf.display()))
        .arg(url);

    let status = cmd.status().context("Failed to run mpv")?;
    let code = status.code().unwrap_or(-1);

    match code {
        42 => Ok(PlaybackResult::ReturnToMenu),
        0 => {
            println!(
                "{}",
                "Video finished. Returning to search results...".green()
            );
            Ok(PlaybackResult::ReturnToMenu)
        }
        other => {
            println!(
                "{}",
                format!("Player exited with code {other}. Returning to search results...").yellow()
            );
            Ok(PlaybackResult::ReturnToMenu)
        }
    }
}

fn play_with_download(
    config: &Config,
    video_id: &str,
    video_title: &str,
    safe_title: &str,
    url: &str,
    temp_dir: &Path,
    player_name: &str,
) -> Result<PlaybackResult> {
    ensure_download_capabilities(config)?;
    println!("{} {}", "Downloading temporarily:".blue(), video_title);

    let ext = if config.audio_only { "mp3" } else { "mp4" };
    let output_base = temporary_download_base(safe_title, video_id);
    let output_path = temp_dir.join(format!("{output_base}.{ext}"));

    let mut cmd = Command::new("yt-dlp");
    cmd.arg("-f").arg(config.format());

    if config.audio_only {
        cmd.arg("-x")
            .arg("--audio-format")
            .arg("mp3")
            .arg("--audio-quality")
            .arg("0");
    }

    cmd.arg("-o").arg(&output_path).arg(url);

    let status = cmd
        .status()
        .context("Failed to run yt-dlp for temporary download")?;

    if !status.success() {
        eprintln!(
            "{} yt-dlp download failed with exit code {}",
            "Error:".red(),
            status.code().unwrap_or(-1)
        );
        return Ok(PlaybackResult::ReturnToMenu);
    }

    // yt-dlp can select a different final extension after post-processing.
    // Restrict fallback discovery to this title so an older temp file can
    // never be played (and deleted) by mistake.
    let downloaded_file = if output_path.exists() {
        output_path
    } else {
        match find_downloaded_file(temp_dir, &output_base)? {
            Some(path) => path,
            None => {
                eprintln!("{} Failed to locate the downloaded video.", "Error:".red());
                return Ok(PlaybackResult::ReturnToMenu);
            }
        }
    };

    let _download_cleanup = DeleteOnDrop::new(downloaded_file.clone(), !config.keep_temp);

    println!("{}", format!("Playing with {player_name}...").blue());
    println!(
        "{}",
        "File will be deleted after playback unless Keep Temporary Files is enabled".yellow()
    );

    // The cleanup guard above also runs when spawning a player fails.
    match player_name {
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
        _ => bail!("Unsupported player: {player_name}"),
    };

    println!(
        "{}",
        "Video finished. Returning to search results...".green()
    );

    Ok(PlaybackResult::ReturnToMenu)
}

fn ensure_download_capabilities(config: &Config) -> Result<()> {
    if download_requires_ffmpeg(config) && which::which("ffmpeg").is_err() {
        bail!(
            "ffmpeg is required to merge separate video/audio streams or extract audio. Install ffmpeg (which normally includes the recommended ffprobe tool), then try again."
        );
    }
    Ok(())
}

fn download_requires_ffmpeg(config: &Config) -> bool {
    config.audio_only || config.format().contains('+')
}

fn temporary_download_base(safe_title: &str, video_id: &str) -> String {
    let mut id: String = video_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .collect();
    if id.is_empty() {
        id = "unknown".to_string();
    } else if id.len() > MAX_VIDEO_ID_BYTES {
        let mut hasher = DefaultHasher::new();
        video_id.hash(&mut hasher);
        let suffix = format!("-{:016x}", hasher.finish());
        id.truncate(MAX_VIDEO_ID_BYTES - suffix.len());
        id.push_str(&suffix);
    }

    let title = if safe_title.trim().is_empty() {
        "video"
    } else {
        safe_title
    };
    let max_title_bytes = MAX_DOWNLOAD_BASE_BYTES.saturating_sub(id.len() + 1);
    let title = truncate_utf8(title, max_title_bytes);
    format!("{title}-{id}")
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }

    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn find_downloaded_file(temp_dir: &Path, base_name: &str) -> Result<Option<PathBuf>> {
    let expected_prefix = format!("{base_name}.");
    let mut candidates = Vec::new();

    for entry in fs::read_dir(temp_dir).context("Failed to inspect temporary download directory")? {
        let entry = entry.context("Failed to inspect temporary download")?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let extension = path.extension().and_then(|extension| extension.to_str());

        if path.is_file()
            && file_name.starts_with(&expected_prefix)
            && !matches!(extension, Some("part" | "conf"))
        {
            candidates.push(path);
        }
    }

    candidates.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    Ok(candidates.pop())
}

struct DeleteOnDrop {
    path: PathBuf,
    enabled: bool,
}

impl DeleteOnDrop {
    fn new(path: PathBuf, enabled: bool) -> Self {
        Self { path, enabled }
    }
}

impl Drop for DeleteOnDrop {
    fn drop(&mut self) {
        if self.enabled {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloaded_file_search_only_returns_the_requested_title() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("other.mp4"), b"other").unwrap();
        fs::write(temp_dir.path().join("wanted.part"), b"partial").unwrap();
        let wanted = temp_dir.path().join("wanted.webm");
        fs::write(&wanted, b"wanted").unwrap();

        assert_eq!(
            find_downloaded_file(temp_dir.path(), "wanted").unwrap(),
            Some(wanted)
        );
        assert!(
            find_downloaded_file(temp_dir.path(), "missing")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn temporary_download_cleanup_runs_on_early_return_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("video.mp4");
        fs::write(&path, b"video").unwrap();

        {
            let _cleanup = DeleteOnDrop::new(path.clone(), true);
        }

        assert!(!path.exists());
    }

    #[test]
    fn temporary_download_names_include_video_identity() {
        assert_eq!(
            temporary_download_base("Same title", "first-id"),
            "Same title-first-id"
        );
        assert_ne!(
            temporary_download_base("Same title", "first-id"),
            temporary_download_base("Same title", "second-id")
        );
        assert_eq!(temporary_download_base("", "../"), "video-unknown");
    }

    #[test]
    fn temporary_download_name_caps_unicode_title_on_a_utf8_boundary() {
        let title = "🎵".repeat(200);
        let base = temporary_download_base(&title, "video-id");
        let file_name = format!("{base}.webm");

        assert!(file_name.len() <= 255);
        assert!(base.ends_with("-video-id"));
        assert!(base.is_char_boundary(base.len()));
    }

    #[test]
    fn ffmpeg_is_only_required_for_merge_or_audio_extraction() {
        assert!(download_requires_ffmpeg(&Config::default()));
        assert!(download_requires_ffmpeg(&Config {
            audio_only: true,
            custom_format: "best".to_string(),
            ..Config::default()
        }));
        assert!(!download_requires_ffmpeg(&Config {
            custom_format: "best".to_string(),
            ..Config::default()
        }));
    }
}
