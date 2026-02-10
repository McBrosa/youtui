use crate::cli::Cli;
use crate::player::PlayerType;

pub struct Config {
    pub num_results: usize,
    pub audio_only: bool,
    #[allow(dead_code)]
    pub limit_bandwidth: bool,
    pub download_mode: bool,
    pub keep_temp: bool,
    pub format: String,
    pub include_shorts: bool,
    pub query: String,
    pub player: PlayerType,
    pub download_dir: String,
}

impl Config {
    pub fn from_cli(cli: &Cli, player: PlayerType) -> Self {
        let format = match &cli.format {
            Some(f) => f.clone(),
            None => resolve_format(cli.audio_only, cli.limit),
        };

        let download_dir = dirs_home().join("Downloads").to_string_lossy().to_string();

        Config {
            num_results: cli.num,
            audio_only: cli.audio_only,
            limit_bandwidth: cli.limit,
            download_mode: cli.download,
            keep_temp: cli.keep,
            format,
            include_shorts: cli.include_shorts,
            query: cli.query.join(" "),
            player,
            download_dir,
        }
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn resolve_format(audio_only: bool, limit: bool) -> String {
    match (audio_only, limit) {
        (true, true) => "bestaudio[abr<=128]/bestaudio/best".to_string(),
        (true, false) => "bestaudio/best".to_string(),
        (false, true) => "bestvideo[height<=360]+bestaudio/best[height<=360]/best".to_string(),
        (false, false) => "bestvideo+bestaudio/best".to_string(),
    }
}
