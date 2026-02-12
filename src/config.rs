use serde::{Deserialize, Serialize};
use std::fs;
use anyhow::{Context, Result};
use crate::player::PlayerType;

fn default_auto_play_queue() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip)]
    pub player: PlayerType,  // Auto-detected, not saved

    pub audio_only: bool,
    pub bandwidth_limit: bool,
    pub keep_temp: bool,
    pub include_shorts: bool,
    pub download_mode: bool,
    pub download_dir: String,
    pub results_per_page: usize,
    pub custom_format: String,
    #[serde(default = "default_auto_play_queue")]
    pub auto_play_queue: bool,
}

impl Config {
    pub fn load_or_create() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let contents = fs::read_to_string(&config_path)
                .context("Failed to read config file")?;
            let mut config: Config = toml::from_str(&contents)
                .context("Failed to parse config file")?;
            config.player = PlayerType::Mpv; // Placeholder, set in main
            Ok(config)
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        fs::create_dir_all(config_path.parent().unwrap())?;
        let toml_string = toml::to_string_pretty(self)?;
        fs::write(config_path, toml_string)?;
        Ok(())
    }

    fn config_path() -> Result<std::path::PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or(anyhow::anyhow!("No config directory found"))?;
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

    pub fn format(&self) -> String {
        if !self.custom_format.is_empty() {
            self.custom_format.clone()
        } else {
            resolve_format(self.audio_only, self.bandwidth_limit)
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
            custom_format: String::new(),
            auto_play_queue: true,
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

        assert_eq!(config.audio_only, false);
        assert_eq!(config.bandwidth_limit, false);
        assert_eq!(config.results_per_page, 20);
        assert!(config.download_dir.ends_with("Downloads"));
        assert!(config.custom_format.is_empty());
    }

    #[test]
    fn test_format_audio_only() {
        let mut config = Config::default();
        config.audio_only = true;
        config.bandwidth_limit = false;

        assert_eq!(config.format(), "bestaudio/best");
    }

    #[test]
    fn test_format_bandwidth_limit() {
        let mut config = Config::default();
        config.audio_only = false;
        config.bandwidth_limit = true;

        assert_eq!(config.format(), "bestvideo[height<=360]+bestaudio/best[height<=360]/best");
    }

    #[test]
    fn test_format_both_flags() {
        let mut config = Config::default();
        config.audio_only = true;
        config.bandwidth_limit = true;

        assert_eq!(config.format(), "bestaudio[abr<=128]/bestaudio/best");
    }

    #[test]
    fn test_format_custom() {
        let mut config = Config::default();
        config.custom_format = "custom/format".to_string();

        assert_eq!(config.format(), "custom/format");
    }

    #[test]
    fn test_toggle_methods() {
        let mut config = Config::default();

        assert_eq!(config.audio_only, false);
        let _ = config.toggle_audio_only();
        assert_eq!(config.audio_only, true);
    }
}
