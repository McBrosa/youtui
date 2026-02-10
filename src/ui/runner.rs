use std::time::{Duration, Instant};
use crossterm::event::{self, Event};
use anyhow::Result;
use crate::ui::{App, AppEvent, handle_key_event, layout::render_ui, terminal::Tui};

const TICK_RATE: Duration = Duration::from_millis(250);

pub fn run_app(terminal: &mut Tui, mut app: App) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| render_ui(f, &app))?;

        let timeout = TICK_RATE
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(&mut app, key);
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
