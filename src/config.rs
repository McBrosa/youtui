use serde::{Deserialize, Serialize};
use std::fs;
use anyhow::{Context, Result};
use crate::player::PlayerType;

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

    // Temporary fields for backwards compatibility - will be removed in Task #19
    #[serde(skip)]
    pub query: String,
    #[serde(skip)]
    pub num_results: usize,
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
        Ok(config_dir.join("yt-search-play/config.toml"))
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

    pub fn format(&self) -> String {
        if !self.custom_format.is_empty() {
            self.custom_format.clone()
        } else {
            resolve_format(self.audio_only, self.bandwidth_limit)
        }
    }

    // Temporary backwards compatibility shim - will be removed in Task #19
    pub fn from_cli(cli: &crate::cli::Cli, player: PlayerType) -> Self {
        let mut config = Self::default();
        config.player = player;
        config.results_per_page = cli.num;
        config.num_results = cli.num;
        config.audio_only = cli.audio_only;
        config.bandwidth_limit = cli.limit;
        config.download_mode = cli.download;
        config.keep_temp = cli.keep;
        config.include_shorts = cli.include_shorts;
        config.query = cli.query.join(" ");
        if let Some(ref format) = cli.format {
            config.custom_format = format.clone();
        }
        config
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
            query: String::new(),
            num_results: 20,
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
