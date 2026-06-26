use crossterm::{cursor, execute, terminal::*};
use ratatui::prelude::*;
use std::io::{self, Stderr, Stdout};
use std::sync::Once;
use std::sync::atomic::{AtomicU8, Ordering};

// Which TTY currently hosts an alternate-screen TUI: 0 none, 1 stdout, 2 stderr.
// Read by the panic hook so a panic mid-render still leaves the terminal usable
// (raw mode off, alternate screen left, cursor shown). Without this, a panic
// anywhere inside a picker's render/event loop would unwind past the explicit
// restore and strand the user's terminal.
static ACTIVE_TTY: AtomicU8 = AtomicU8::new(0);
static PANIC_HOOK: Once = Once::new();

fn ensure_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            match ACTIVE_TTY.load(Ordering::SeqCst) {
                1 => {
                    let _ = disable_raw_mode();
                    let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
                }
                2 => {
                    let _ = disable_raw_mode();
                    let _ = execute!(io::stderr(), LeaveAlternateScreen, cursor::Show);
                }
                _ => {}
            }
            original(info);
        }));
    });
}

pub fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    ensure_panic_hook();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    ACTIVE_TTY.store(1, Ordering::SeqCst);
    Terminal::new(CrosstermBackend::new(stdout))
}

pub fn restore_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    ACTIVE_TTY.store(0, Ordering::SeqCst);
    Ok(())
}

/// Terminal that renders to stderr. Used by TUIs whose final result is
/// printed to stdout so it can be captured by a shell wrapper (e.g.
/// `cd "$(gx workspace go)"`).
pub fn setup_terminal_stderr() -> io::Result<Terminal<CrosstermBackend<Stderr>>> {
    ensure_panic_hook();
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;
    ACTIVE_TTY.store(2, Ordering::SeqCst);
    Terminal::new(CrosstermBackend::new(stderr))
}

pub fn restore_terminal_stderr(mut terminal: Terminal<CrosstermBackend<Stderr>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    ACTIVE_TTY.store(0, Ordering::SeqCst);
    Ok(())
}
