use std::fs;
use std::io::Write;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::player::PlayerType;

pub(crate) const MIN_RESULTS_PER_PAGE: usize = 1;
// YouTube searches are intentionally capped at 500 entries. Keeping a page at
// or below that ceiling avoids configurations that can never be filled.
pub(crate) const MAX_RESULTS_PER_PAGE: usize = 500;
pub(crate) const MIN_SEEK_STEP: u64 = 1;
pub(crate) const MAX_SEEK_STEP: u64 = 3600;

pub(crate) fn clamp_results_per_page(value: usize) -> usize {
    value.clamp(MIN_RESULTS_PER_PAGE, MAX_RESULTS_PER_PAGE)
}

pub(crate) fn clamp_seek_step(value: u64) -> u64 {
    value.clamp(MIN_SEEK_STEP, MAX_SEEK_STEP)
}

fn default_auto_play_queue() -> bool {
    true
}

/// How the terminal video view draws frames. `Auto` picks `Pixels` when the
/// terminal supports a graphics protocol (Kitty/iTerm2/Sixel), otherwise
/// falls back to half-block cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoRenderMode {
    #[default]
    Auto,
    Pixels,
    Blocks,
}

impl VideoRenderMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Auto => Self::Pixels,
            Self::Pixels => Self::Blocks,
            Self::Blocks => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Pixels => "pixels",
            Self::Blocks => "blocks",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip)]
    pub player: PlayerType, // Auto-detected, not saved

    pub audio_only: bool,
    pub bandwidth_limit: bool,
    pub keep_temp: bool,
    pub include_shorts: bool,
    pub download_mode: bool,
    pub download_dir: String,
    pub results_per_page: usize,
    pub seek_step: u64,
    pub seek_step_large: u64,
    pub custom_format: String,
    #[serde(default = "default_auto_play_queue")]
    pub auto_play_queue: bool,
    #[serde(default)]
    pub video_render: VideoRenderMode,
}

impl Config {
    pub fn load_or_create() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            Self::load_from_path(&config_path)
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }

    fn load_from_path(config_path: &std::path::Path) -> Result<Self> {
        let contents = fs::read_to_string(config_path).context("Failed to read config file")?;
        let mut config: Config =
            toml::from_str(&contents).context("Failed to parse config file")?;
        config.normalize();
        config.player = PlayerType::Mpv; // Placeholder, set in main
        Ok(config)
    }

    #[cfg(not(test))]
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        self.save_to_path(&config_path)
    }

    /// Unit tests exercise persistence through `save_to_path` with an isolated
    /// temporary directory. UI tests may call normal settings methods, but must
    /// never mutate the developer's real configuration file.
    #[cfg(test)]
    pub fn save(&self) -> Result<()> {
        Ok(())
    }

    fn save_to_path(&self, config_path: &std::path::Path) -> Result<()> {
        let parent = config_path
            .parent()
            .ok_or_else(|| anyhow!("Config path has no parent directory"))?;
        fs::create_dir_all(parent).context("Failed to create config directory")?;

        // Serialize a normalized clone so an invalid value assigned through a
        // public field never becomes persistent configuration.
        let mut normalized = self.clone();
        normalized.normalize();
        let toml_string = toml::to_string_pretty(&normalized)?;

        // Write beside the destination and atomically rename into place. This
        // prevents an interruption from leaving a truncated config file.
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .context("Failed to create temporary config file")?;
        temp.write_all(toml_string.as_bytes())
            .context("Failed to write temporary config file")?;
        temp.as_file()
            .sync_all()
            .context("Failed to flush temporary config file")?;
        temp.persist(config_path)
            .map_err(|error| error.error)
            .context("Failed to replace config file")?;
        Ok(())
    }

    fn config_path() -> Result<std::path::PathBuf> {
        let config_dir = dirs::config_dir().ok_or(anyhow::anyhow!("No config directory found"))?;
        Ok(config_dir.join("youtui/config.toml"))
    }

    pub fn toggle_audio_only(&mut self) -> Result<()> {
        self.audio_only = !self.audio_only;
        self.save()
    }

    pub fn toggle_bandwidth_limit(&mut self) -> Result<()> {
        self.bandwidth_limit = !self.bandwidth_limit;
        self.save()
    }

    pub fn toggle_keep_temp(&mut self) -> Result<()> {
        self.keep_temp = !self.keep_temp;
        self.save()
    }

    pub fn toggle_include_shorts(&mut self) -> Result<()> {
        self.include_shorts = !self.include_shorts;
        self.save()
    }

    pub fn toggle_download_mode(&mut self) -> Result<()> {
        self.download_mode = !self.download_mode;
        self.save()
    }

    pub fn toggle_auto_play_queue(&mut self) -> Result<()> {
        self.auto_play_queue = !self.auto_play_queue;
        self.save()
    }

    pub fn cycle_video_render(&mut self) -> Result<()> {
        self.video_render = self.video_render.cycle();
        self.save()
    }

    pub fn format(&self) -> String {
        if !self.custom_format.is_empty() {
            self.custom_format.clone()
        } else {
            resolve_format(self.audio_only, self.bandwidth_limit)
        }
    }

    fn normalize(&mut self) {
        self.results_per_page = clamp_results_per_page(self.results_per_page);
        self.seek_step = clamp_seek_step(self.seek_step);
        self.seek_step_large = clamp_seek_step(self.seek_step_large);

        if self.download_dir.trim().is_empty() {
            self.download_dir = Self::default().download_dir;
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            player: PlayerType::Mpv,
            audio_only: false,
            bandwidth_limit: false,
            keep_temp: false,
            include_shorts: false,
            download_mode: false,
            download_dir: dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("Downloads")
                .to_string_lossy()
                .to_string(),
            results_per_page: 20,
            seek_step: 5,
            seek_step_large: 60,
            custom_format: String::new(),
            auto_play_queue: true,
            video_render: VideoRenderMode::Auto,
        }
    }
}

fn resolve_format(audio_only: bool, limit: bool) -> String {
    match (audio_only, limit) {
        (true, true) => "bestaudio[abr<=128]/bestaudio/best".to_string(),
        (true, false) => "bestaudio/best".to_string(),
        (false, true) => "bestvideo[height<=360]+bestaudio/best[height<=360]/best".to_string(),
        (false, false) => "bestvideo+bestaudio/best".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();

        assert!(!config.audio_only);
        assert!(!config.bandwidth_limit);
        assert_eq!(config.results_per_page, 20);
        assert_eq!(config.seek_step, 5);
        assert_eq!(config.seek_step_large, 60);
        assert!(config.download_dir.ends_with("Downloads"));
        assert!(config.custom_format.is_empty());
    }

    #[test]
    fn missing_fields_use_defaults_for_backward_compatible_configs() {
        let config: Config = toml::from_str("audio_only = true").unwrap();

        assert!(config.audio_only);
        assert_eq!(config.results_per_page, 20);
        assert_eq!(config.seek_step, 5);
        assert_eq!(config.seek_step_large, 60);
        assert!(config.auto_play_queue);
        assert!(!config.download_dir.is_empty());
    }

    #[test]
    fn normalization_clamps_page_size_and_repairs_empty_download_directory() {
        let mut too_small = Config {
            results_per_page: 0,
            download_dir: "   ".to_string(),
            ..Config::default()
        };
        too_small.normalize();
        assert_eq!(too_small.results_per_page, MIN_RESULTS_PER_PAGE);
        assert!(!too_small.download_dir.trim().is_empty());

        let mut too_large = Config {
            results_per_page: usize::MAX,
            ..Config::default()
        };
        too_large.normalize();
        assert_eq!(too_large.results_per_page, MAX_RESULTS_PER_PAGE);
    }

    #[test]
    fn saved_config_is_normalized_and_can_atomically_replace_an_existing_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("config.toml");
        let mut config = Config {
            results_per_page: 0,
            ..Config::default()
        };

        config.save_to_path(&path).unwrap();
        let first: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(first.results_per_page, MIN_RESULTS_PER_PAGE);

        config.results_per_page = usize::MAX;
        config.save_to_path(&path).unwrap();
        let second: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(second.results_per_page, MAX_RESULTS_PER_PAGE);
    }

    #[test]
    fn seek_steps_round_trip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("config.toml");
        let config = Config {
            seek_step: 15,
            seek_step_large: 120,
            ..Config::default()
        };

        config.save_to_path(&path).unwrap();
        let saved = Config::load_from_path(&path).unwrap();

        assert_eq!(saved.seek_step, 15);
        assert_eq!(saved.seek_step_large, 120);
    }

    #[test]
    fn seek_steps_are_clamped_when_saved() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("config.toml");
        let config = Config {
            seek_step: 0,
            seek_step_large: u64::MAX,
            ..Config::default()
        };

        config.save_to_path(&path).unwrap();
        let saved: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();

        assert_eq!(saved.seek_step, MIN_SEEK_STEP);
        assert_eq!(saved.seek_step_large, MAX_SEEK_STEP);
    }

    #[test]
    fn seek_steps_are_clamped_when_loaded() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("config.toml");
        fs::write(&path, "seek_step = 0\nseek_step_large = 3601\n").unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.seek_step, MIN_SEEK_STEP);
        assert_eq!(config.seek_step_large, MAX_SEEK_STEP);
    }

    #[test]
    fn video_render_mode_cycles_and_round_trips() {
        assert_eq!(VideoRenderMode::Auto.cycle(), VideoRenderMode::Pixels);
        assert_eq!(VideoRenderMode::Pixels.cycle(), VideoRenderMode::Blocks);
        assert_eq!(VideoRenderMode::Blocks.cycle(), VideoRenderMode::Auto);

        let config: Config = toml::from_str("video_render = \"blocks\"").unwrap();
        assert_eq!(config.video_render, VideoRenderMode::Blocks);
        // Older configs without the field default to auto.
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.video_render, VideoRenderMode::Auto);
    }

    #[test]
    fn test_format_audio_only() {
        let config = Config {
            audio_only: true,
            bandwidth_limit: false,
            ..Config::default()
        };

        assert_eq!(config.format(), "bestaudio/best");
    }

    #[test]
    fn test_format_bandwidth_limit() {
        let config = Config {
            audio_only: false,
            bandwidth_limit: true,
            ..Config::default()
        };

        assert_eq!(
            config.format(),
            "bestvideo[height<=360]+bestaudio/best[height<=360]/best"
        );
    }

    #[test]
    fn test_format_both_flags() {
        let config = Config {
            audio_only: true,
            bandwidth_limit: true,
            ..Config::default()
        };

        assert_eq!(config.format(), "bestaudio[abr<=128]/bestaudio/best");
    }

    #[test]
    fn test_format_custom() {
        let config = Config {
            custom_format: "custom/format".to_string(),
            ..Config::default()
        };

        assert_eq!(config.format(), "custom/format");
    }
}
