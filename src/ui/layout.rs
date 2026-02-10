use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Text,
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use crate::ui::app::App;

pub fn render_ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // Header
            Constraint::Min(0),     // Results
            Constraint::Length(2),  // Footer
        ])
        .split(f.size());

    render_header(f, app, chunks[0]);
    render_results(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let header = Paragraph::new(format!("yt-search-play │ Query: {}", app.query))
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(header, area);
}

fn render_results(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Search Results");
    let text = Text::from(format!("{} results", app.results.len()));
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, _app: &App, area: Rect) {
    let footer = Paragraph::new("q: Quit")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, area);
}
