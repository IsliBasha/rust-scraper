use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Gauge, Paragraph, Row, Table},
    Frame,
};
use scraper_metrics::MetricsSnapshot;

pub fn draw(f: &mut Frame, snap: &MetricsSnapshot) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(0),
        ])
        .split(f.area());

    let header = Paragraph::new("rust-scraper — press q to quit")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    f.render_widget(header, chunks[0]);

    let total = snap.urls_done + snap.urls_failed + snap.urls_pending + snap.urls_in_progress;
    let ratio = if total == 0 {
        0.0
    } else {
        snap.urls_done as f64 / total as f64
    };
    let label = format!("{}/{} done", snap.urls_done, total);
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Progress "))
        .gauge_style(Style::default().fg(Color::Green))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label);
    f.render_widget(gauge, chunks[1]);

    // Bind all temporary strings to variables so they outlive the Row borrows.
    let pending = snap.urls_pending.to_string();
    let in_prog = snap.urls_in_progress.to_string();
    let done = snap.urls_done.to_string();
    let failed = snap.urls_failed.to_string();
    let skipped = snap.urls_skipped.to_string();
    let bytes = format!("{:.1} KB", snap.bytes_downloaded as f64 / 1024.0);
    let rps = format!("{:.1}", snap.requests_per_second);
    let workers = snap.active_workers.to_string();

    let rows = vec![
        Row::new(["Pending", pending.as_str()]),
        Row::new(["In Progress", in_prog.as_str()]),
        Row::new(["Done", done.as_str()]),
        Row::new(["Failed", failed.as_str()]),
        Row::new(["Skipped", skipped.as_str()]),
        Row::new(["Bytes", bytes.as_str()]),
        Row::new(["Req/s", rps.as_str()]),
        Row::new(["Workers", workers.as_str()]),
    ];

    let header_row = Row::new(vec![
        Span::styled("Metric", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("Value", Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let table = Table::new(rows, [Constraint::Length(16), Constraint::Min(10)])
        .header(header_row)
        .block(Block::default().borders(Borders::ALL).title(" Metrics "))
        .column_spacing(2);

    f.render_widget(table, chunks[2]);
}
