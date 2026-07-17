use anyhow::{Context, Result, bail};
use std::io::{self, Write};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum Platform {
    MacOS,
    Linux(LinuxDistro),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinuxDistro {
    Debian, // apt-based (Ubuntu, Debian, Mint)
    RedHat, // dnf-based (RHEL, Fedora, CentOS)
    Arch,   // pacman-based (Arch, Manjaro)
    Unknown,
}

pub fn ensure_dependencies() -> Result<()> {
    let missing = missing_dependencies(check_dependency);

    if missing.is_empty() {
        return Ok(());
    }

    // Detect platform
    let platform = detect_platform()?;

    // Prompt user
    if !prompt_user(&missing, &platform)? {
        println!("\nInstallation declined.");
        println!("\nTo use youtui, please install the required dependencies:");
        println!("  {}", format_manual_command(&platform, &missing));
        println!("\nThen relaunch youtui.");
        std::process::exit(0);
    }

    // Install
    install_dependencies(&missing, &platform)?;

    // Verify installation
    let still_missing: Vec<&str> = missing
        .iter()
        .copied()
        .filter(|&dep| !check_dependency(dep))
        .collect();

    if !still_missing.is_empty() {
        bail!(
            "✗ Installation verification failed\n\nThe following dependencies are still missing: {}\n\nPlease install manually:\n  {}",
            still_missing.join(", "),
            format_manual_command(&platform, &still_missing)
        );
    }

    println!("Launching youtui...\n");
    Ok(())
}

fn check_dependency(name: &str) -> bool {
    which::which(name).is_ok()
}

fn missing_dependencies(mut is_available: impl FnMut(&str) -> bool) -> Vec<&'static str> {
    let mut missing = Vec::new();

    if !is_available("yt-dlp") {
        missing.push("yt-dlp");
    }

    // mpv is preferred for background playback, but VLC and mplayer are
    // supported fallbacks. Only install mpv when no supported player exists.
    if !["mpv", "vlc", "mplayer"].into_iter().any(is_available) {
        missing.push("mpv");
    }

    missing
}

fn parse_os_release(content: &str) -> LinuxDistro {
    let mut id = "";
    let mut id_like = "";

    for line in content.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        match key.trim() {
            "ID" => id = value,
            "ID_LIKE" => id_like = value,
            _ => {}
        }
    }

    let id = id.to_ascii_lowercase();
    let id_like: Vec<String> = id_like
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    let is_or_is_like = |names: &[&str]| {
        names.contains(&id.as_str()) || id_like.iter().any(|value| names.contains(&value.as_str()))
    };

    if is_or_is_like(&["debian", "ubuntu", "linuxmint", "mint"]) {
        return LinuxDistro::Debian;
    }

    if is_or_is_like(&["fedora", "rhel", "centos", "rocky", "almalinux"]) {
        return LinuxDistro::RedHat;
    }

    if is_or_is_like(&["arch", "manjaro"]) {
        return LinuxDistro::Arch;
    }

    LinuxDistro::Unknown
}

fn get_install_command(platform: &Platform, deps: &[&str]) -> Result<(&'static str, Vec<String>)> {
    let command = match platform {
        Platform::MacOS => {
            let mut args = vec!["install".to_string()];
            args.extend(deps.iter().map(|s| s.to_string()));
            ("brew", args)
        }
        Platform::Linux(LinuxDistro::Debian) => {
            let deps_str = deps.join(" ");
            let command = format!("sudo apt update && sudo apt install -y {}", deps_str);
            ("sh", vec!["-c".to_string(), command])
        }
        Platform::Linux(LinuxDistro::RedHat) => {
            let mut args = vec!["dnf".to_string(), "install".to_string(), "-y".to_string()];
            args.extend(deps.iter().map(|s| s.to_string()));
            ("sudo", args)
        }
        Platform::Linux(LinuxDistro::Arch) => {
            let mut args = vec![
                "pacman".to_string(),
                "-S".to_string(),
                "--noconfirm".to_string(),
            ];
            args.extend(deps.iter().map(|s| s.to_string()));
            ("sudo", args)
        }
        Platform::Linux(LinuxDistro::Unknown) => {
            bail!("Cannot choose a package manager for an unsupported Linux distribution")
        }
    };
    Ok(command)
}

fn get_package_manager_name(platform: &Platform) -> &str {
    match platform {
        Platform::MacOS => "Homebrew",
        Platform::Linux(LinuxDistro::Debian) => "apt",
        Platform::Linux(LinuxDistro::RedHat) => "dnf",
        Platform::Linux(LinuxDistro::Arch) => "pacman",
        Platform::Linux(LinuxDistro::Unknown) => "your system package manager",
    }
}

fn prompt_user(deps: &[&str], platform: &Platform) -> Result<bool> {
    let deps_str = deps.join(", ");
    let pm_name = get_package_manager_name(platform);

    println!("\nMissing dependencies: {}", deps_str);
    println!("\nyoutui requires these tools to search and play YouTube videos.");

    let sudo_note = if matches!(platform, Platform::MacOS) {
        ""
    } else {
        " (requires sudo)"
    };

    print!("\nInstall now using {}?{} [Y/n]: ", pm_name, sudo_note);
    io::stdout().flush()?;

    let mut input = String::new();
    let bytes_read = io::stdin().read_line(&mut input)?;

    Ok(install_confirmed((bytes_read > 0).then_some(&input)))
}

fn install_confirmed(input: Option<&str>) -> bool {
    let Some(input) = input else {
        // EOF usually means a non-interactive invocation. Never interpret it
        // as permission to run a package manager (and potentially sudo).
        return false;
    };
    matches!(input.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes")
}

fn install_dependencies(deps: &[&str], platform: &Platform) -> Result<()> {
    println!("\nInstalling dependencies...");

    let (program, args) = get_install_command(platform, deps)?;
    let args_display: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    println!("Running: {} {}", program, args_display.join(" "));
    println!();

    let status = Command::new(program)
        .args(&args)
        .status()
        .context(format!("Failed to execute {} command", program))?;

    if !status.success() {
        bail!(
            "✗ Installation failed\n\nPlease install manually:\n  {}\n\nThen relaunch youtui.",
            format_manual_command(platform, deps)
        );
    }

    println!("\n✓ Installation complete!\n");
    Ok(())
}

fn format_manual_command(platform: &Platform, deps: &[&str]) -> String {
    let deps_str = deps.join(" ");
    match platform {
        Platform::MacOS => format!("brew install {}", deps_str),
        Platform::Linux(LinuxDistro::Debian) => format!("sudo apt install {}", deps_str),
        Platform::Linux(LinuxDistro::RedHat) => format!("sudo dnf install {}", deps_str),
        Platform::Linux(LinuxDistro::Arch) => format!("sudo pacman -S {}", deps_str),
        Platform::Linux(LinuxDistro::Unknown) => {
            format!(
                "install {} using your distribution's package manager",
                deps_str
            )
        }
    }
}

fn detect_platform() -> Result<Platform> {
    if cfg!(target_os = "macos") {
        // Verify Homebrew exists
        if !check_dependency("brew") {
            bail!(
                "Homebrew is required to install dependencies.\n\nInstall Homebrew from: https://brew.sh\n\nThen relaunch youtui."
            );
        }
        return Ok(Platform::MacOS);
    }

    // Linux
    if cfg!(target_os = "linux") {
        let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let distro = parse_os_release(&os_release);
        if distro == LinuxDistro::Unknown {
            bail!(
                "Unsupported Linux distribution; automatic dependency installation is unavailable.\n\nInstall yt-dlp and one supported player (mpv, VLC, or mplayer) using your distribution's package manager, then relaunch youtui."
            );
        }
        return Ok(Platform::Linux(distro));
    }

    bail!("Unsupported platform (not macOS or Linux)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_dependency_existing() {
        // Test with 'ls' which exists on all Unix systems
        assert!(check_dependency("ls"));
    }

    #[test]
    fn test_check_dependency_missing() {
        assert!(!check_dependency("nonexistent-command-xyz-123"));
    }

    #[test]
    fn test_missing_dependencies_accepts_supported_fallback_players() {
        let available = |name: &str| matches!(name, "yt-dlp" | "vlc");
        assert!(missing_dependencies(available).is_empty());

        let available = |name: &str| name == "mplayer";
        assert_eq!(missing_dependencies(available), vec!["yt-dlp"]);
    }

    #[test]
    fn test_missing_dependencies_requests_mpv_only_without_any_player() {
        let available = |name: &str| name == "yt-dlp";
        assert_eq!(missing_dependencies(available), vec!["mpv"]);

        assert_eq!(missing_dependencies(|_| false), vec!["yt-dlp", "mpv"]);
    }

    #[test]
    fn test_detect_platform_macos() {
        if cfg!(target_os = "macos") {
            let platform = detect_platform().unwrap();
            assert_eq!(platform, Platform::MacOS);
        }
    }

    #[test]
    fn test_parse_os_release_debian() {
        let content = "ID=ubuntu\nID_LIKE=debian\n";
        assert_eq!(parse_os_release(content), LinuxDistro::Debian);
    }

    #[test]
    fn test_parse_os_release_fedora() {
        let content = "ID=fedora\n";
        assert_eq!(parse_os_release(content), LinuxDistro::RedHat);
    }

    #[test]
    fn test_parse_os_release_arch() {
        let content = "ID=arch\n";
        assert_eq!(parse_os_release(content), LinuxDistro::Arch);
    }

    #[test]
    fn test_parse_os_release_default() {
        let content = "ID=unknown\n";
        assert_eq!(parse_os_release(content), LinuxDistro::Unknown);
    }

    #[test]
    fn test_parse_os_release_quoted_id_like() {
        let content = "ID=rocky\nID_LIKE=\"rhel centos fedora\"\n";
        assert_eq!(parse_os_release(content), LinuxDistro::RedHat);
    }

    #[test]
    fn test_parse_os_release_does_not_match_id_prefixes() {
        let content = "ID=archcraft\n";
        assert_eq!(parse_os_release(content), LinuxDistro::Unknown);
    }

    #[test]
    fn test_install_confirmation_distinguishes_default_yes_from_eof() {
        assert!(install_confirmed(Some("\n")));
        assert!(install_confirmed(Some("YES\n")));
        assert!(!install_confirmed(Some("no\n")));
        assert!(!install_confirmed(None));
    }

    #[test]
    fn test_get_install_command_macos() {
        let (program, args) = get_install_command(&Platform::MacOS, &["mpv", "yt-dlp"]).unwrap();
        assert_eq!(program, "brew");
        assert_eq!(args, vec!["install", "mpv", "yt-dlp"]);
    }

    #[test]
    fn test_get_install_command_debian() {
        let (program, args) =
            get_install_command(&Platform::Linux(LinuxDistro::Debian), &["mpv", "yt-dlp"]).unwrap();
        assert_eq!(program, "sh");
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("apt update"));
        assert!(args[1].contains("apt install"));
    }

    #[test]
    fn test_get_install_command_redhat() {
        let (program, args) =
            get_install_command(&Platform::Linux(LinuxDistro::RedHat), &["mpv", "yt-dlp"]).unwrap();
        assert_eq!(program, "sudo");
        assert_eq!(args, vec!["dnf", "install", "-y", "mpv", "yt-dlp"]);
    }

    #[test]
    fn test_get_install_command_arch() {
        let (program, args) =
            get_install_command(&Platform::Linux(LinuxDistro::Arch), &["mpv", "yt-dlp"]).unwrap();
        assert_eq!(program, "sudo");
        assert_eq!(args, vec!["pacman", "-S", "--noconfirm", "mpv", "yt-dlp"]);
    }

    #[test]
    fn test_get_install_command_rejects_unknown_linux() {
        let result =
            get_install_command(&Platform::Linux(LinuxDistro::Unknown), &["mpv", "yt-dlp"]);
        assert!(result.is_err());
    }
}
