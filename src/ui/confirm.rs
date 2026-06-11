use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use ratatui::{TerminalOptions, Viewport};
use std::io::{self, Write};

pub fn run(message: &str) -> miette::Result<bool> {
    let mut stdout = io::stdout();
    run_inner(message, &mut stdout)
}

/// Confirm prompt rendered on stderr, keeping stdout clean for
/// machine-readable output (e.g. paths consumed by the shell wrapper).
pub fn run_on_stderr(message: &str) -> miette::Result<bool> {
    let mut stderr = io::stderr();
    run_inner(message, &mut stderr)
}

fn run_inner<W: Write>(message: &str, writer: &mut W) -> miette::Result<bool> {
    let backend = CrosstermBackend::new(writer);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(1),
        },
    )
    .into_diagnostic()?;

    enable_raw_mode().into_diagnostic()?;

    let mut selected = 0; // 0 = Yes, 1 = No

    loop {
        terminal
            .draw(|f| {
                let area = f.area();

                let yes_style = if selected == 0 {
                    Style::default().fg(Color::Black).bg(Color::Green)
                } else {
                    Style::default().fg(Color::Green)
                };

                let no_style = if selected == 1 {
                    Style::default().fg(Color::Black).bg(Color::Red)
                } else {
                    Style::default().fg(Color::Red)
                };

                let line = Line::from(vec![
                    Span::styled(format!("{} ", message), Style::default().fg(Color::Yellow)),
                    Span::styled(" y ", yes_style),
                    Span::raw(" "),
                    Span::styled(" n ", no_style),
                    Span::styled("  (y/n)", Style::default().fg(Color::DarkGray)),
                ]);

                f.render_widget(Paragraph::new(line), area);
            })
            .into_diagnostic()?;

        if let Event::Key(key) = event::read().into_diagnostic()? {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') => selected = 0,
                KeyCode::Right | KeyCode::Char('l') => selected = 1,
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    disable_raw_mode().ok();
                    eprintln!();
                    return Ok(true);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    disable_raw_mode().ok();
                    eprintln!();
                    return Ok(false);
                }
                KeyCode::Enter => {
                    disable_raw_mode().ok();
                    eprintln!();
                    return Ok(selected == 0);
                }
                _ => {}
            }
        }
    }
}
