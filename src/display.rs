use std::io::{self, Write};

use colored::Colorize;

use crate::player::PlayerType;
use crate::search::SearchResult;

pub enum UserAction {
    Play(usize),
    Quit,
    NewSearch(String),
    NextPage,
    PrevPage,
}

pub fn show_results(results: &[SearchResult], page: usize, page_size: usize, total: usize, exhausted: bool) {
    let total_pages = (total + page_size - 1) / page_size;
    let page_info = if exhausted {
        format!("(page {}/{})", page + 1, total_pages)
    } else {
        format!("(page {})", page + 1)
    };
    let total_info = if exhausted {
        format!("[{} total]", total)
    } else {
        format!("[{}+ loaded]", total)
    };
    println!(
        "\n{} {} {}",
        "Search Results".green(),
        page_info.cyan(),
        total_info.cyan(),
    );
    let start = page * page_size;
    for (i, result) in results.iter().enumerate() {
        let global_num = start + i + 1;
        let num = format!("{:>3}. ", global_num);
        println!(
            "{}{}",
            num.yellow(),
            result.title.yellow()
        );
        println!(
            "     Duration: {} | Channel: {} | Views: {} | ID: {}",
            result.duration, result.channel, result.views, result.id
        );
    }
}

pub fn get_selection(page_start: usize, page_len: usize, has_next: bool, has_prev: bool) -> UserAction {
    loop {
        let mut hints = vec!["'q' quit", "'s' new search"];
        if has_next { hints.push("'n' next page"); }
        if has_prev { hints.push("'p' prev page"); }

        println!(
            "\n{}",
            format!("Enter video # to play ({}): ", hints.join(", ")).green()
        );

        let mut input = String::new();
        print!("> ");
        let _ = io::stdout().flush();
        if io::stdin().read_line(&mut input).is_err() {
            continue;
        }
        let input = input.trim();

        match input {
            "q" => {
                println!("Exiting.");
                return UserAction::Quit;
            }
            "n" if has_next => return UserAction::NextPage,
            "p" if has_prev => return UserAction::PrevPage,
            "s" => {
                println!("\n{}", "Enter new search query:".cyan());
                let mut query = String::new();
                print!("> ");
                let _ = io::stdout().flush();
                if io::stdin().read_line(&mut query).is_err() {
                    continue;
                }
                let query = query.trim().to_string();
                if !query.is_empty() {
                    return UserAction::NewSearch(query);
                }
                continue;
            }
            _ => {}
        }

        // Parse as a global result number (1-indexed)
        match input.parse::<usize>() {
            Ok(n) if n >= page_start + 1 && n <= page_start + page_len => {
                return UserAction::Play(n - 1); // 0-indexed into full results vec
            }
            Ok(_) => {
                eprintln!("{}", format!("Pick a number between {} and {}.", page_start + 1, page_start + page_len).red());
            }
            _ => {
                eprintln!("{}", "Invalid choice.".red());
            }
        }
    }
}

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
