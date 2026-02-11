use colored::Colorize;

use crate::player::PlayerType;

pub fn show_controls(player: PlayerType) {
    println!("\n{}", "Keyboard Controls:".cyan());
    match player {
        PlayerType::Mpv => {
            println!("  Space       - Play/Pause");
            println!("  q           - Quit playback");
            println!("  m           - Mute/Unmute");
            println!("  Left/Right  - Seek backward/forward");
            println!("  [/]         - Decrease/Increase playback speed");
            println!("  r           - Return to search results");
        }
        PlayerType::Vlc => {
            println!("  Space       - Play/Pause");
            println!("  Ctrl+Q      - Quit playback");
            println!("  m           - Mute/Unmute");
            println!("  Ctrl+Left/Right - Seek backward/forward");
            println!("  [/]         - Decrease/Increase playback speed");
            println!("  (Automatically returns to search results after playback)");
        }
        PlayerType::Mplayer => {
            println!("  Space       - Play/Pause");
            println!("  q           - Quit playback");
            println!("  m           - Mute/Unmute");
            println!("  Left/Right  - Seek backward/forward");
            println!("  {{/}}         - Decrease/Increase playback speed");
            println!("  (Automatically returns to search results after playback)");
        }
    }
}
