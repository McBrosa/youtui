use std::sync::atomic::{AtomicBool, Ordering};

use colored::Colorize;

pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub struct ManagedTempDir {
    inner: Option<tempfile::TempDir>,
    keep: bool,
}

impl ManagedTempDir {
    pub fn new(keep: bool) -> anyhow::Result<Self> {
        let dir = tempfile::TempDir::with_prefix("yt-search-play-")?;
        Ok(ManagedTempDir {
            inner: Some(dir),
            keep,
        })
    }

    pub fn path(&self) -> &std::path::Path {
        self.inner.as_ref().unwrap().path()
    }
}

impl Drop for ManagedTempDir {
    fn drop(&mut self) {
        if let Some(dir) = self.inner.take() {
            if self.keep {
                let path = dir.keep();
                println!(
                    "{} {}",
                    "Temporary files kept at:".green(),
                    path.display()
                );
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
    let _ = ctrlc::set_handler(move || {
        if INTERRUPTED.load(Ordering::SeqCst) {
            // Second Ctrl-C: force exit
            std::process::exit(1);
        }
        INTERRUPTED.store(true, Ordering::SeqCst);
        eprintln!("\n{}", "Interrupted. Press Ctrl-C again to force exit.".yellow());
    });
}

pub fn is_interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}
