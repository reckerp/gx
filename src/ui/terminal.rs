use crossterm::{execute, terminal::*};
use ratatui::prelude::*;
use std::io::{self, Stderr, Stdout};

pub fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

pub fn restore_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Terminal that renders to stderr. Used by TUIs whose final result is
/// printed to stdout so it can be captured by a shell wrapper (e.g.
/// `cd "$(gx workspace go)"`).
pub fn setup_terminal_stderr() -> io::Result<Terminal<CrosstermBackend<Stderr>>> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stderr))
}

pub fn restore_terminal_stderr(mut terminal: Terminal<CrosstermBackend<Stderr>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
