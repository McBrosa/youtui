use anyhow::{Result, bail, Context};
use std::process::Command;
use std::io::{self, Write};

#[derive(Debug, Clone, PartialEq)]
pub enum Platform {
    MacOS,
    Linux(LinuxDistro),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinuxDistro {
    Debian,  // apt-based (Ubuntu, Debian, Mint)
    RedHat,  // dnf-based (RHEL, Fedora, CentOS)
    Arch,    // pacman-based (Arch, Manjaro)
}

pub fn ensure_dependencies() -> Result<()> {
    // Check which dependencies are missing
    let missing: Vec<&str> = ["yt-dlp", "mpv"]
        .iter()
        .copied()
        .filter(|&dep| !check_dependency(dep))
        .collect();

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

fn parse_os_release(content: &str) -> LinuxDistro {
    let content_lower = content.to_lowercase();

    // Check ID= and ID_LIKE= fields
    if content_lower.contains("id=debian") || content_lower.contains("id=ubuntu")
        || content_lower.contains("id=mint") || content_lower.contains("id_like=debian") {
        return LinuxDistro::Debian;
    }

    if content_lower.contains("id=fedora") || content_lower.contains("id=rhel")
        || content_lower.contains("id=centos") {
        return LinuxDistro::RedHat;
    }

    if content_lower.contains("id=arch") || content_lower.contains("id=manjaro") {
        return LinuxDistro::Arch;
    }

    // Default to Debian (most common)
    LinuxDistro::Debian
}

fn get_install_command(platform: &Platform, deps: &[&str]) -> (&'static str, Vec<String>) {
    match platform {
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
            let mut args = vec!["pacman".to_string(), "-S".to_string(), "--noconfirm".to_string()];
            args.extend(deps.iter().map(|s| s.to_string()));
            ("sudo", args)
        }
    }
}

fn get_package_manager_name(platform: &Platform) -> &str {
    match platform {
        Platform::MacOS => "Homebrew",
        Platform::Linux(LinuxDistro::Debian) => "apt",
        Platform::Linux(LinuxDistro::RedHat) => "dnf",
        Platform::Linux(LinuxDistro::Arch) => "pacman",
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
    io::stdin().read_line(&mut input)?;

    let input = input.trim().to_lowercase();
    Ok(input.is_empty() || input == "y" || input == "yes")
}

fn install_dependencies(deps: &[&str], platform: &Platform) -> Result<()> {
    println!("\nInstalling dependencies...");

    let (program, args) = get_install_command(platform, deps);
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
    }
}

fn detect_platform() -> Result<Platform> {
    if cfg!(target_os = "macos") {
        // Verify Homebrew exists
        if !check_dependency("brew") {
            bail!("Homebrew is required to install dependencies.\n\nInstall Homebrew from: https://brew.sh\n\nThen relaunch youtui.");
        }
        return Ok(Platform::MacOS);
    }

    // Linux
    if cfg!(target_os = "linux") {
        let os_release = std::fs::read_to_string("/etc/os-release")
            .unwrap_or_default();
        let distro = parse_os_release(&os_release);
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
        assert_eq!(parse_os_release(content), LinuxDistro::Debian);
    }

    #[test]
    fn test_get_install_command_macos() {
        let (program, args) = get_install_command(&Platform::MacOS, &["mpv", "yt-dlp"]);
        assert_eq!(program, "brew");
        assert_eq!(args, vec!["install", "mpv", "yt-dlp"]);
    }

    #[test]
    fn test_get_install_command_debian() {
        let (program, args) = get_install_command(
            &Platform::Linux(LinuxDistro::Debian),
            &["mpv", "yt-dlp"]
        );
        assert_eq!(program, "sh");
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("apt update"));
        assert!(args[1].contains("apt install"));
    }

    #[test]
    fn test_get_install_command_redhat() {
        let (program, args) = get_install_command(
            &Platform::Linux(LinuxDistro::RedHat),
            &["mpv", "yt-dlp"]
        );
        assert_eq!(program, "sudo");
        assert_eq!(args, vec!["dnf", "install", "-y", "mpv", "yt-dlp"]);
    }

    #[test]
    fn test_get_install_command_arch() {
        let (program, args) = get_install_command(
            &Platform::Linux(LinuxDistro::Arch),
            &["mpv", "yt-dlp"]
        );
        assert_eq!(program, "sudo");
        assert_eq!(args, vec!["pacman", "-S", "--noconfirm", "mpv", "yt-dlp"]);
    }
}
