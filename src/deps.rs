use anyhow::{Result, bail};
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
}
