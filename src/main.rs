mod cleanup;
mod config;
mod deps;
mod display;
mod ipc;
mod player;
mod player_manager;
mod queue;
mod search;
mod ui;
mod video;

use std::ffi::OsString;

use anyhow::{Result, bail};

use cleanup::{ManagedTempDir, setup_signal_handler};
use config::Config;
use player::detect_player;
use search::{PaginatedSearch, check_ytdlp};
use ui::FocusedPanel;

fn main() -> Result<()> {
    match parse_cli_args(std::env::args_os().skip(1))? {
        CliAction::Run => {}
        CliAction::Help => {
            print_help();
            return Ok(());
        }
        CliAction::Version => {
            println!("youtui {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
    }

    // Check and install dependencies if needed
    deps::ensure_dependencies()?;

    // Load or create config (no CLI parsing)
    let mut config = Config::load_or_create()?;

    // Check dependencies (now defensive only)
    check_ytdlp()?;
    let player = detect_player()?;
    config.player = player;

    // Create managed temp dir
    let mut temp_dir = ManagedTempDir::new(config.keep_temp)?;
    setup_signal_handler();

    // Initialize TUI with empty query
    let terminal = ui::init_terminal()?;
    let mut terminal_guard = ui::TerminalGuard::new(terminal);

    // Search manager with no initial query
    let mut search = PaginatedSearch::new("", config.results_per_page, !config.include_shorts);

    // App is the sole runtime owner of configuration.
    let mut app = ui::App::new(String::new(), config.results_per_page, config);
    app.focused_panel = FocusedPanel::SearchBar;

    // Run TUI loop
    let result = ui::run_app(terminal_guard.get_mut(), app, &mut search, &mut temp_dir);

    drop(terminal_guard);
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliAction {
    Run,
    Help,
    Version,
}

fn parse_cli_args(args: impl IntoIterator<Item = OsString>) -> Result<CliAction> {
    let args: Vec<OsString> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(CliAction::Run),
        [arg] if arg == "-h" || arg == "--help" => Ok(CliAction::Help),
        [arg] if arg == "-V" || arg == "--version" => Ok(CliAction::Version),
        [arg] => bail!(
            "unknown argument `{}`\n\nTry `youtui --help` for usage.",
            arg.to_string_lossy()
        ),
        _ => bail!("youtui does not accept positional arguments\n\nTry `youtui --help` for usage."),
    }
}

fn print_help() {
    println!(
        "youtui {version}\n{description}\n\nUsage: youtui\n\nOptions:\n  -h, --help       Print help\n  -V, --version    Print version",
        version = env!("CARGO_PKG_VERSION"),
        description = env!("CARGO_PKG_DESCRIPTION"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_no_arguments() {
        assert_eq!(parse_cli_args([]).unwrap(), CliAction::Run);
    }

    #[test]
    fn cli_handles_help_and_version_before_startup() {
        assert_eq!(
            parse_cli_args([OsString::from("--help")]).unwrap(),
            CliAction::Help
        );
        assert_eq!(
            parse_cli_args([OsString::from("-V")]).unwrap(),
            CliAction::Version
        );
    }

    #[test]
    fn cli_rejects_unknown_and_extra_arguments() {
        assert!(parse_cli_args([OsString::from("--wat")]).is_err());
        assert!(parse_cli_args([OsString::from("--help"), OsString::from("extra")]).is_err());
    }
}
