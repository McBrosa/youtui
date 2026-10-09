mod cleanup;
mod config;
mod deps;
mod display;
mod ipc;
mod kitty_shm;
mod player;
mod player_manager;
mod queue;
mod search;
mod ui;
mod video;
mod web_backend;
mod web_launcher;

use std::ffi::OsString;

use anyhow::{Result, bail};

use cleanup::{ManagedTempDir, setup_signal_handler};
use config::Config;
use player::detect_player;
use search::{PaginatedSearch, check_ytdlp};
use ui::FocusedPanel;
use web_launcher::WebOptions;

fn main() -> Result<()> {
    match parse_cli_args(std::env::args_os().skip(1))? {
        CliAction::Run => {}
        CliAction::Web(options) => return web_launcher::run(options),
        CliAction::Backend => return web_backend::run_stdio(),
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

    // Detect terminal graphics support (Kitty/iTerm2) for the video view's
    // pixel renderer. Env/ioctl only — never reads stdin (see detect_picker).
    let picker = video::detect_picker();

    // Search manager with no initial query
    let mut search = PaginatedSearch::new("", config.results_per_page, !config.include_shorts);

    // App is the sole runtime owner of configuration.
    let mut app = ui::App::new(String::new(), config.results_per_page, config);
    app.focused_panel = FocusedPanel::SearchBar;
    if let Some(picker) = picker {
        app.video.set_picker(picker);
    }

    // Run TUI loop
    let result = ui::run_app(terminal_guard.get_mut(), app, &mut search, &mut temp_dir);

    drop(terminal_guard);
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CliAction {
    Run,
    Web(WebOptions),
    Backend,
    Help,
    Version,
}

fn parse_cli_args(args: impl IntoIterator<Item = OsString>) -> Result<CliAction> {
    let args: Vec<OsString> = args.into_iter().collect();
    match args.as_slice() {
        [] => return Ok(CliAction::Run),
        [arg] if arg == "-h" || arg == "--help" => return Ok(CliAction::Help),
        [arg] if arg == "-V" || arg == "--version" => return Ok(CliAction::Version),
        [arg] if arg == "--backend-stdio" => return Ok(CliAction::Backend),
        _ => {}
    }

    let mut web = false;
    let mut no_open = false;
    let mut port = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--web") if !web => web = true,
            Some("--no-open") if !no_open => no_open = true,
            Some("--port") if port.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--port requires a value"))?;
                let number = value
                    .to_str()
                    .and_then(|value| value.parse::<u16>().ok())
                    .filter(|number| *number > 0)
                    .ok_or_else(|| anyhow::anyhow!("--port requires a number from 1 to 65535"))?;
                port = Some(number);
            }
            _ => bail!(
                "unknown or repeated argument `{}`\n\nTry `youtui --help` for usage.",
                arg.to_string_lossy()
            ),
        }
    }
    if !web {
        bail!("--port and --no-open require --web\n\nTry `youtui --help` for usage.");
    }
    Ok(CliAction::Web(WebOptions {
        port: port.unwrap_or(4000),
        open_browser: !no_open,
    }))
}

fn print_help() {
    println!(
        "youtui {version}\n{description}\n\nUsage: youtui [--web [--port PORT] [--no-open]]\n\nOptions:\n      --web        Start the local browser interface\n      --port PORT  Set the web port (default: 4000)\n      --no-open    Print the web URL without opening a browser\n  -h, --help       Print help\n  -V, --version    Print version",
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

    #[test]
    fn cli_selects_web_and_accepts_its_options() {
        let args = ["--web", "--port", "4010", "--no-open"].map(OsString::from);
        assert_eq!(
            parse_cli_args(args).unwrap(),
            CliAction::Web(WebOptions {
                port: 4010,
                open_browser: false,
            })
        );
        assert_eq!(
            parse_cli_args([OsString::from("--web")]).unwrap(),
            CliAction::Web(WebOptions {
                port: 4000,
                open_browser: true
            })
        );
    }

    #[test]
    fn cli_rejects_invalid_web_options() {
        for args in [
            vec!["--port", "4000"],
            vec!["--no-open"],
            vec!["--web", "--port"],
            vec!["--web", "--port", "0"],
            vec!["--web", "--port", "65536"],
            vec!["--web", "--port", "abc"],
            vec!["--web", "--web"],
            vec!["--web", "--backend-stdio"],
        ] {
            assert!(parse_cli_args(args.into_iter().map(OsString::from)).is_err());
        }
    }
}
