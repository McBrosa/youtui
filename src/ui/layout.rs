use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
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
    let results = app.current_page_results();
    let start_idx = app.page * app.page_size;

    let items: Vec<String> = results
        .iter()
        .enumerate()
        .map(|(i, result)| {
            let num = start_idx + i + 1;
            format!(
                "{:>3}. {}\n     Duration: {} | Channel: {} | Views: {} | ID: {}",
                num, result.title, result.duration, result.channel, result.views, result.id
            )
        })
        .collect();

    let page_info = if app.exhausted {
        format!("Page {}/{} • {} total", app.page + 1, (app.total_results + app.page_size - 1) / app.page_size, app.total_results)
    } else {
        format!("Page {} • {}+ loaded", app.page + 1, app.total_results)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Search Results ({})", page_info))
        .border_style(Style::default().fg(Color::DarkGray));

    let text = items.join("\n\n");
    let paragraph = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(Color::White));

    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, _app: &App, area: Rect) {
    let footer = Paragraph::new("q: Quit")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchResult;

    #[test]
    fn test_result_formatting() {
        let mut app = App::new("test query".to_string(), 10);

        // Add some test results
        app.results = vec![
            SearchResult {
                title: "Test Video 1".to_string(),
                duration: "10:30".to_string(),
                channel: "Test Channel".to_string(),
                views: "1.2M".to_string(),
                id: "abc123".to_string(),
            },
            SearchResult {
                title: "Test Video 2".to_string(),
                duration: "5:45".to_string(),
                channel: "Another Channel".to_string(),
                views: "500K".to_string(),
                id: "def456".to_string(),
            },
        ];
        app.total_results = 2;
        app.exhausted = true;

        let results = app.current_page_results();
        let start_idx = app.page * app.page_size;

        let items: Vec<String> = results
            .iter()
            .enumerate()
            .map(|(i, result)| {
                let num = start_idx + i + 1;
                format!(
                    "{:>3}. {}\n     Duration: {} | Channel: {} | Views: {} | ID: {}",
                    num, result.title, result.duration, result.channel, result.views, result.id
                )
            })
            .collect();

        // Verify first item formatting
        assert_eq!(
            items[0],
            "  1. Test Video 1\n     Duration: 10:30 | Channel: Test Channel | Views: 1.2M | ID: abc123"
        );

        // Verify second item formatting
        assert_eq!(
            items[1],
            "  2. Test Video 2\n     Duration: 5:45 | Channel: Another Channel | Views: 500K | ID: def456"
        );

        // Verify items joined with blank lines
        let text = items.join("\n\n");
        assert!(text.contains("\n\n"));
    }

    #[test]
    fn test_page_info_exhausted() {
        let mut app = App::new("test query".to_string(), 10);
        app.total_results = 25;
        app.exhausted = true;
        app.page = 1;

        let page_info = format!(
            "Page {}/{} • {} total",
            app.page + 1,
            (app.total_results + app.page_size - 1) / app.page_size,
            app.total_results
        );

        assert_eq!(page_info, "Page 2/3 • 25 total");
    }

    #[test]
    fn test_page_info_not_exhausted() {
        let mut app = App::new("test query".to_string(), 10);
        app.total_results = 30;
        app.exhausted = false;
        app.page = 2;

        let page_info = format!("Page {} • {}+ loaded", app.page + 1, app.total_results);

        assert_eq!(page_info, "Page 3 • 30+ loaded");
    }

    #[test]
    fn test_pagination_offset() {
        let mut app = App::new("test query".to_string(), 10);

        // Add 15 results
        for i in 1..=15 {
            app.results.push(SearchResult {
                title: format!("Video {}", i),
                duration: "5:00".to_string(),
                channel: "Channel".to_string(),
                views: "1K".to_string(),
                id: format!("id{}", i),
            });
        }
        app.total_results = 15;
        app.page = 1; // Second page

        let results = app.current_page_results();
        let start_idx = app.page * app.page_size;

        // First result on page 2 should be numbered 11
        let item = format!(
            "{:>3}. {}\n     Duration: {} | Channel: {} | Views: {} | ID: {}",
            start_idx + 1,
            results[0].title,
            results[0].duration,
            results[0].channel,
            results[0].views,
            results[0].id
        );

        assert!(item.starts_with(" 11. Video 11"));
    }
}
