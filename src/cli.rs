use clap::Parser;

/// Search and play YouTube videos using yt-dlp
#[derive(Parser, Debug)]
#[command(name = "yt-search-play", version, about)]
pub struct Cli {
    /// Number of search results to show
    #[arg(short = 'n', long = "num", default_value_t = 20)]
    pub num: usize,

    /// Play audio only (no video)
    #[arg(short = 'a', long = "audio-only")]
    pub audio_only: bool,

    /// Limit bandwidth usage (audio 128k, video 360p)
    #[arg(short = 'l', long = "limit")]
    pub limit: bool,

    /// Download permanently instead of temporary streaming
    #[arg(short = 'd', long = "download")]
    pub download: bool,

    /// Keep temporary files after playback
    #[arg(short = 'k', long = "keep")]
    pub keep: bool,

    /// Specify custom yt-dlp format string
    #[arg(short = 'f', long = "format")]
    pub format: Option<String>,

    /// Include YouTube Shorts and videos under 3 minutes
    #[arg(short = 'i', long = "include-shorts")]
    pub include_shorts: bool,

    /// Search query
    #[arg(required = true, trailing_var_arg = true)]
    pub query: Vec<String>,
}
