use crossterm::{cursor, execute, terminal::*};
use ratatui::prelude::*;
use std::io::{self, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicU8, Ordering};

use super::{Term, TermStderr};

// Which TTY currently hosts an alternate-screen TUI: 0 none, 1 stdout, 2 stderr.
// Read by the panic hook so a panic mid-render still leaves the terminal usable
// (raw mode off, alternate screen left, cursor shown). Without this, a panic
// anywhere inside a picker's render/event loop would unwind past the guard's
// Drop only if it ran — the hook is the belt to the guard's suspenders.
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

/// Enter raw mode and the alternate screen on `writer`, tagging `ACTIVE_TTY`
/// (1 = stdout, 2 = stderr) so the panic hook knows which stream to restore.
fn enter<W: Write>(mut writer: W, tag: u8) -> io::Result<Terminal<CrosstermBackend<W>>> {
    ensure_panic_hook();
    enable_raw_mode()?;
    execute!(writer, EnterAlternateScreen)?;
    ACTIVE_TTY.store(tag, Ordering::SeqCst);
    Terminal::new(CrosstermBackend::new(writer))
}

/// RAII guard owning an active TUI terminal; restores cooked mode and the main
/// screen on drop. Because teardown lives in `Drop`, it can't be forgotten or
/// mis-ordered relative to an early `?`/panic — the failure mode the previous
/// manual setup/restore call pairs risked.
struct TerminalGuard<W: Write> {
    terminal: Option<Terminal<CrosstermBackend<W>>>,
}

impl<W: Write> TerminalGuard<W> {
    fn new(terminal: Terminal<CrosstermBackend<W>>) -> Self {
        Self {
            terminal: Some(terminal),
        }
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<W>> {
        self.terminal
            .as_mut()
            .expect("terminal guard missing terminal before restore")
    }

    fn restore(&mut self) -> io::Result<()> {
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(());
        };

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)?;
        ACTIVE_TTY.store(0, Ordering::SeqCst);
        self.terminal = None;
        Ok(())
    }
}

impl<W: Write> Drop for TerminalGuard<W> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Run `f` with a TUI terminal rendering to stdout, restoring the terminal
/// before returning no matter how `f` exits. The returned `io::Result` reports
/// terminal setup or restore failure; `f`'s own value (e.g. a picker's
/// `miette::Result`) is returned untouched inside the `Ok`.
pub fn with_terminal<R>(f: impl FnOnce(&mut Term) -> R) -> io::Result<R> {
    let mut guard = TerminalGuard::new(enter(io::stdout(), 1)?);
    let result = f(guard.terminal_mut());
    guard.restore()?;
    Ok(result)
}

/// Like [`with_terminal`], but renders to stderr so the TUI's final result can
/// be printed to stdout and captured by a shell wrapper (e.g.
/// `cd "$(gx workspace go)"`).
pub fn with_terminal_stderr<R>(f: impl FnOnce(&mut TermStderr) -> R) -> io::Result<R> {
    let mut guard = TerminalGuard::new(enter(io::stderr(), 2)?);
    let result = f(guard.terminal_mut());
    guard.restore()?;
    Ok(result)
}

/// Temporarily leave a stdout TUI — cooked mode, main screen, cursor shown — to
/// run `f` (e.g. spawning `$EDITOR` with inherited stdio), then re-enter the
/// alternate screen and force a full redraw. Used by the review comment popup's
/// `$EDITOR` pop-out. The panic hook still covers an unwinding `f`.
pub fn suspend<R>(terminal: &mut Term, f: impl FnOnce() -> R) -> io::Result<R> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, cursor::Show)?;
    ACTIVE_TTY.store(0, Ordering::SeqCst);

    let result = f();

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    ACTIVE_TTY.store(1, Ordering::SeqCst);
    terminal.clear()?;
    Ok(result)
}
