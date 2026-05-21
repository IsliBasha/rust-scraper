use scraper_metrics::{MetricsHub, MetricsSnapshot, ScrapeEvent};
use tokio::sync::watch;

/// Owns the TUI event loop. Call `run()` to block until the user quits.
pub struct TuiApp {
    pub metrics: MetricsHub,
}

impl TuiApp {
    pub fn new(metrics: MetricsHub) -> Self {
        Self { metrics }
    }

    /// Run the TUI until the user presses `q` or `Ctrl-C`.
    pub async fn run(self) -> anyhow::Result<()> {
        use crossterm::{
            event::{self, Event, KeyCode},
            execute,
            terminal::{
                disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
            },
        };
        use ratatui::{backend::CrosstermBackend, Terminal};
        use std::io;

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let snap_rx: watch::Receiver<MetricsSnapshot> = self.metrics.watch_snapshots();
        let mut event_rx = self.metrics.subscribe_events();

        loop {
            let snap = snap_rx.borrow().clone();
            terminal.draw(|f| crate::ui::draw(f, &snap))?;

            // Non-blocking keyboard check.
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                        break;
                    }
                }
            }

            // Check for shutdown event.
            if let Ok(ScrapeEvent::Shutdown) = event_rx.try_recv() {
                break;
            }
        }

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        Ok(())
    }
}
