# Automatic Dependency Installation Design

**Date:** 2026-02-11
**Status:** Design Complete
**Goal:** Automatically detect and install yt-dlp and mpv if missing when youtui launches

## Overview

youtui requires two external tools to function:
- **yt-dlp** - For searching and fetching YouTube video metadata/URLs
- **mpv** - For playing videos with background playback support

Currently, if these are missing, youtui errors out with manual installation instructions. This design adds automatic dependency management using system package managers.

**Platforms Supported:**
- macOS (Homebrew)
- Linux (apt, dnf, pacman)

**Installation Approach:**
- Use native package managers (Homebrew on macOS, apt/dnf/pacman on Linux)
- Prompt user for permission before installing
- Check dependencies on every startup (fast check, ~100-200ms)

## Architecture & Startup Flow

### Startup Sequence

1. Launch youtui
2. Run dependency check: `ensure_dependencies()?`
   - Detect platform (macOS/Linux)
   - Check if yt-dlp exists with `which::which("yt-dlp")`
   - Check if mpv exists with `which::which("mpv")`
   - If both found: continue to TUI
   - If missing: prompt user and install
3. Load config and launch TUI as normal

### New Module: `src/deps.rs`

```rust
pub fn ensure_dependencies() -> Result<()>;
fn detect_platform() -> Result<Platform>;
fn check_dependency(name: &str) -> bool;
fn install_dependencies(deps: &[&str], platform: Platform) -> Result<()>;
fn prompt_user(deps: &[&str], platform: &Platform) -> Result<bool>;
```

### Integration Point

In `src/main.rs`, before loading config:

```rust
fn main() -> Result<()> {
    // Check and install dependencies first
    crate::deps::ensure_dependencies()?;

    // Continue with normal startup
    let mut config = Config::load_or_create()?;
    // ...
}
```

### Error Handling

If installation fails or user declines:
- Show clear error message with manual installation instructions
- Exit cleanly with code 1 (not 0, since setup didn't complete)
- Don't show TUI (avoid confusing state)

## Platform Detection & Installation Commands

### Platform Detection

```rust
enum Platform {
    MacOS,
    Linux(LinuxDistro),
}

enum LinuxDistro {
    Debian,  // apt-based (Ubuntu, Debian, Mint)
    RedHat,  // yum/dnf-based (RHEL, Fedora, CentOS)
    Arch,    // pacman-based (Arch, Manjaro)
}
```

**Detection Strategy:**

**macOS:**
- Check `cfg!(target_os = "macos")`
- Verify Homebrew exists: `which brew`
- If Homebrew missing: Error with install link (https://brew.sh)

**Linux:**
- Check `/etc/os-release` file
- Parse `ID=` or `ID_LIKE=` fields:
  - `debian`, `ubuntu`, `mint` → Debian
  - `fedora`, `rhel`, `centos` → RedHat
  - `arch`, `manjaro` → Arch
- Default to Debian if uncertain (most common)

### Installation Commands

| Platform | Command |
|----------|---------|
| macOS | `brew install mpv yt-dlp` |
| Debian/Ubuntu | `sudo apt update && sudo apt install -y mpv yt-dlp` |
| RedHat/Fedora | `sudo dnf install -y mpv yt-dlp` |
| Arch | `sudo pacman -S --noconfirm mpv yt-dlp` |

**Command Execution:**
- Use `std::process::Command`
- Inherit stdout/stderr to show installation progress
- Wait for command to complete
- Check exit status (0 = success)

**Typical Installation Time:** 30-60 seconds

## User Interaction & Prompting

### Prompt Format

**macOS:**
```
Missing dependencies: yt-dlp, mpv

youtui requires these tools to search and play YouTube videos.

Install now using Homebrew? [Y/n]: _
```

**Linux:**
```
Missing dependencies: yt-dlp, mpv

youtui requires these tools to search and play YouTube videos.

Install now using apt? (requires sudo) [Y/n]: _
```

### Reading User Input

Use `std::io::stdin().read_line()`:
- `Y`, `y`, or empty (Enter) → Install
- `N`, `n` → Show manual instructions and exit
- Invalid input → Show error and ask again (max 3 attempts)

### During Installation

Show progress:
```
Installing dependencies...
Running: brew install mpv yt-dlp

[actual command output streams here]

✓ Installation complete!

Launching youtui...
```

### On Installation Success

Continue immediately to normal startup (load config, show TUI).

### On Installation Failure

```
✗ Installation failed

Please install manually:
  brew install mpv yt-dlp

Then relaunch youtui.
```

Exit with code 1.

### On User Decline

```
Installation declined.

To use youtui, please install the required dependencies:
  brew install mpv yt-dlp

Then relaunch youtui.
```

Exit with code 0 (user choice, not an error).

## Edge Cases & Error Handling

### 1. Partial Installation Success

**Scenario:** yt-dlp installs but mpv fails (or vice versa)

**Handling:**
- Check each dependency individually after installation
- Show which succeeded: "✓ yt-dlp installed"
- Show which failed: "✗ mpv installation failed"
- Exit with manual instructions for the failed one

### 2. Homebrew Not Installed (macOS)

**Scenario:** User on macOS without Homebrew

**Handling:**
- Check `which brew` before attempting installation
- If missing: Show error message:
  ```
  Homebrew is required to install dependencies.

  Install Homebrew from: https://brew.sh

  Then relaunch youtui.
  ```
- Don't try to auto-install Homebrew (too invasive, requires user agreement to Xcode license, etc.)

### 3. No Sudo Access (Linux)

**Scenario:** User lacks sudo privileges or cancels password prompt

**Handling:**
- Package manager will prompt for sudo password
- If user cancels or lacks permission, command exits non-zero
- Catch the error and show:
  ```
  ✗ Installation failed (permission denied)

  Please ask your system administrator to install:
    sudo apt install mpv yt-dlp

  Or install from source: https://github.com/yt-dlp/yt-dlp
  ```

### 4. Network Issues

**Scenario:** Package manager can't download due to network failure

**Handling:**
- Package managers show their own error messages (inherit stderr)
- Catch non-zero exit status
- Add helpful context:
  ```
  ✗ Installation failed

  Check your network connection and try again.

  Or install manually:
    brew install mpv yt-dlp
  ```

### 5. Dependencies Disappear After First Launch

**Scenario:** User uninstalls deps between runs

**Handling:**
- Since we check on every startup, this is caught automatically
- Re-prompt for installation
- No special handling needed

### 6. Ctrl+C During Installation

**Scenario:** User presses Ctrl+C while installation is running

**Handling:**
- `Command::status()` will be interrupted
- Return error from `ensure_dependencies()`
- Main catches error and exits cleanly
- Package managers handle their own cleanup (partial installs)

### 7. Already Installed by Different Method

**Scenario:** User has mpv/yt-dlp installed via pip, cargo, or manual build

**Handling:**
- `which::which()` finds any executable in PATH
- Doesn't matter how it was installed
- Skip installation, continue to TUI

### 8. Unsupported Linux Distro

**Scenario:** User on NixOS, Gentoo, or other unsupported distro

**Handling:**
- Platform detection defaults to Debian
- apt command will fail (command not found)
- Show manual installation instructions
- User installs via their distro's method

## Testing Strategy

### Manual Testing

**Test 1: Fresh Install (macOS)**
```bash
# Setup
brew uninstall mpv yt-dlp

# Test
./target/release/youtui

# Expected
- Shows prompt with both deps missing
- User accepts (Y)
- Installation runs with visible output
- Both tools install successfully
- youtui launches to TUI
```

**Test 2: Partial Missing**
```bash
# Setup
brew uninstall mpv
# (keep yt-dlp installed)

# Test
./target/release/youtui

# Expected
- Shows prompt with only mpv missing
- Installation only installs mpv
- youtui launches successfully
```

**Test 3: User Decline**
```bash
# Setup
brew uninstall mpv yt-dlp

# Test
./target/release/youtui
# Type: n

# Expected
- Shows manual installation instructions
- Exits cleanly with code 0
- Does not show TUI
```

**Test 4: Installation Failure**
```bash
# Setup (simulate by breaking Homebrew temporarily)
# On macOS: rename /usr/local/bin/brew

# Test
./target/release/youtui

# Expected
- Detects missing deps
- User accepts installation
- brew command fails
- Shows error with manual instructions
- Exits with code 1
```

**Test 5: No Homebrew (macOS)**
```bash
# Setup (on a Mac without Homebrew)
# Or temporarily hide brew from PATH

# Test
./target/release/youtui

# Expected
- Shows error: "Homebrew is required"
- Shows link to https://brew.sh
- Exits cleanly
```

**Test 6: Linux (Ubuntu VM)**
```bash
# Setup
sudo apt remove mpv yt-dlp

# Test
./target/release/youtui

# Expected
- Shows prompt with apt mentioned
- Mentions "requires sudo"
- User accepts
- Prompts for sudo password
- Installs successfully
- Launches TUI
```

**Test 7: Deps Already Installed**
```bash
# Setup
brew install mpv yt-dlp

# Test
./target/release/youtui

# Expected
- No prompt shown
- Goes directly to TUI
- Fast startup (<200ms overhead)
```

### Unit Testing

**Testable Functions:**

1. **Platform Detection:**
   ```rust
   #[test]
   fn test_parse_os_release() {
       let debian = "ID=ubuntu\nID_LIKE=debian";
       assert_eq!(parse_distro(debian), LinuxDistro::Debian);

       let fedora = "ID=fedora";
       assert_eq!(parse_distro(fedora), LinuxDistro::RedHat);
   }
   ```

2. **Dependency Checking:**
   ```rust
   #[test]
   fn test_check_dependency() {
       // Assumes 'ls' exists on test system
       assert!(check_dependency("ls"));
       assert!(!check_dependency("nonexistent-command-xyz"));
   }
   ```

3. **Command Building:**
   ```rust
   #[test]
   fn test_install_command() {
       let cmd = build_install_command(&["mpv", "yt-dlp"], Platform::MacOS);
       assert_eq!(cmd.program, "brew");
       assert_eq!(cmd.args, ["install", "mpv", "yt-dlp"]);
   }
   ```

### Integration Testing

**Challenges:**
- Requires actual package managers
- Requires sudo access on Linux
- Slow (30-60 seconds per test)
- Modifies system state

**Approach:**
- Focus on manual testing for installation flow
- Use unit tests for logic/parsing
- Consider Docker containers for automated Linux testing (future work)

## Implementation Notes

### Dependencies to Add

```toml
# Already have:
which = "7"
anyhow = "1"

# May need:
# (none - std library has everything needed)
```

### File Structure

```
src/
  deps.rs          # New module for dependency management
  main.rs          # Call ensure_dependencies() before config load
  player.rs        # Keep existing detect_player() for runtime checks
  search.rs        # Keep existing yt-dlp check (defensive)
```

### Startup Performance

- `which::which()` checks: ~1-2ms per check
- Total overhead: ~5ms (two checks)
- Acceptable for every-launch checking

### Future Enhancements (Out of Scope)

- Windows support (winget, chocolatey)
- Auto-update check for dependencies
- Version compatibility checking
- Download progress bars for installation
- Fallback to static binaries if package manager fails
- Async installation with spinner animation

## Success Criteria

1. ✅ Fresh user can launch youtui without pre-installing deps
2. ✅ Installation prompts clearly and waits for user consent
3. ✅ Works on macOS (Homebrew) and Linux (apt/dnf/pacman)
4. ✅ Handles all edge cases gracefully (no panics, clear errors)
5. ✅ Existing users with deps installed see no change (<200ms overhead)
6. ✅ Manual installation path always documented as fallback

## Summary

This design adds automatic dependency management to youtui using system package managers. On every launch, youtui checks for yt-dlp and mpv, prompts the user to install if missing, and handles installation via Homebrew (macOS) or apt/dnf/pacman (Linux). The approach is respectful (asks permission), robust (handles edge cases), and fast (minimal startup overhead).
