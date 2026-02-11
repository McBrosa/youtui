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

fn detect_platform() -> Result<Platform> {
    if cfg!(target_os = "macos") {
        // Verify Homebrew exists
        if !check_dependency("brew") {
            bail!("Homebrew is required to install dependencies.\n\nInstall Homebrew from: https://brew.sh\n\nThen relaunch youtui.");
        }
        return Ok(Platform::MacOS);
    }

    // Linux detection will be added in next task
    bail!("Unsupported platform")
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
}
