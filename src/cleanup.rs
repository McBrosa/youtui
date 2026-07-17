use std::sync::atomic::{AtomicBool, Ordering};

use colored::Colorize;

pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub struct ManagedTempDir {
    inner: Option<tempfile::TempDir>,
    path: std::path::PathBuf,
    keep: bool,
}

impl ManagedTempDir {
    pub fn new(keep: bool) -> anyhow::Result<Self> {
        let dir = tempfile::TempDir::with_prefix("youtui-")?;
        let path = dir.path().to_path_buf();
        Ok(ManagedTempDir {
            inner: Some(dir),
            path,
            keep,
        })
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn set_keep(&mut self, keep: bool) {
        self.keep = keep;
    }
}

impl Drop for ManagedTempDir {
    fn drop(&mut self) {
        if let Some(dir) = self.inner.take() {
            if self.keep {
                let path = dir.keep();
                println!("{} {}", "Temporary files kept at:".green(), path.display());
            } else {
                println!("{}", "Cleaning up temporary files...".yellow());
                // TempDir::drop handles the actual deletion
                drop(dir);
                println!("{}", "Temporary files deleted.".green());
            }
        }
    }
}

pub fn setup_signal_handler() {
    if let Err(error) = ctrlc::set_handler(move || {
        if INTERRUPTED.load(Ordering::SeqCst) {
            // Second Ctrl-C: force exit
            std::process::exit(1);
        }
        INTERRUPTED.store(true, Ordering::SeqCst);
        eprintln!(
            "\n{}",
            "Interrupted. Press Ctrl-C again to force exit.".yellow()
        );
    }) {
        eprintln!("Failed to install Ctrl-C handler: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_temp_path_remains_available_for_its_lifetime() {
        let mut temp_dir = ManagedTempDir::new(false).unwrap();
        assert!(temp_dir.path().is_dir());
        temp_dir.set_keep(true);
        assert!(temp_dir.keep);
        // Keep the test self-cleaning.
        temp_dir.set_keep(false);
    }
}
