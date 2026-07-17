use std::io;

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

pub fn init_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        // EnterAlternateScreen may have written part of its escape sequence
        // before returning an error, so make both rollback attempts.
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        return Err(error.into());
    }

    let backend = CrosstermBackend::new(stdout);
    match Terminal::new(backend) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            // No TerminalGuard exists yet on this path.
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            Err(error.into())
        }
    }
}

pub fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    // Evaluate every cleanup operation before propagating the first error. A
    // failed raw-mode call must not prevent us from restoring the screen and
    // cursor (and vice versa).
    let cursor_result = terminal.show_cursor();
    let screen_result = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let raw_mode_result = disable_raw_mode();

    cursor_result?;
    screen_result?;
    raw_mode_result?;
    Ok(())
}

pub struct TerminalGuard {
    terminal: Option<Tui>,
}

impl TerminalGuard {
    pub fn new(terminal: Tui) -> Self {
        Self {
            terminal: Some(terminal),
        }
    }

    pub fn get_mut(&mut self) -> &mut Tui {
        self.terminal.as_mut().unwrap()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(mut terminal) = self.terminal.take() {
            let _ = restore_terminal(&mut terminal);
        }
    }
}
