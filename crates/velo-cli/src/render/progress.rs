//! A terminal progress bar, as an [`Observer`].
//!
//! Hand-rolled on `console` rather than pulling in a bar crate: the whole thing
//! is one redrawn line, and `console` (already a dependency) supplies the
//! terminal detection and width.
//!
//! Two rules this must respect:
//!
//! * **Nothing on a non-TTY.** Piped output stays clean, and `serve-upload` /
//!   `serve-receive` speak a binary protocol on stdout — a stray bar would
//!   corrupt the wire. Those paths get no observer at all, and this type is inert
//!   when stdout isn't a terminal as a second line of defence.
//! * **Redraw at most every [`REDRAW`].** Core calls `advance` once per item and
//!   deliberately does not throttle, because how often to repaint is a
//!   presentation decision. Hashing 20 000 files must not mean 20 000 repaints.

use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use console::Term;
use velo_core::progress::{Observer, Phase};

/// Minimum gap between repaints.
const REDRAW: Duration = Duration::from_millis(80);

/// A one-line progress bar on stderr.
///
/// **stderr, not stdout**: a command's real output is data the user may be
/// piping, and progress is not part of it.
pub struct Bar {
    term: Term,
    /// `None` when stdout isn't a terminal, which makes every method a no-op.
    state: Option<Mutex<State>>,
}

struct State {
    phase: Option<Phase>,
    total: Option<u64>,
    done: u64,
    last_drawn: Instant,
    /// Width of the last line written, so it can be cleared exactly.
    dirty: bool,
}

impl Bar {
    pub fn new() -> Self {
        let term = Term::stderr();
        // Gate on stderr being a terminal — that is where the bar goes.
        let state = term.is_term().then(|| {
            Mutex::new(State {
                phase: None,
                total: None,
                done: 0,
                last_drawn: Instant::now() - REDRAW,
                dirty: false,
            })
        });
        Bar { term, state }
    }

    fn draw(&self, st: &mut State, force: bool) {
        if !force && st.last_drawn.elapsed() < REDRAW {
            return;
        }
        let Some(phase) = st.phase else { return };
        let line = match st.total {
            Some(total) if total > 0 => {
                let pct = (st.done.min(total) * 100) / total;
                format!(
                    "{} {}/{} {} ({}%)",
                    phase,
                    st.done,
                    total,
                    phase.unit(),
                    pct
                )
            }
            // Indeterminate: a count with no denominator. `Transferring` and `gc`
            // land here.
            _ => format!("{} {} {}…", phase, st.done, phase.unit()),
        };
        // Truncate rather than wrap: a wrapped bar leaves debris behind.
        let width = self.term.size().1 as usize;
        let line: String = line.chars().take(width.saturating_sub(1)).collect();
        let _ = self.term.clear_line();
        let _ = write!(&self.term, "  {}\r", line);
        let _ = std::io::stderr().flush();
        st.last_drawn = Instant::now();
        st.dirty = true;
    }
}

impl Default for Bar {
    fn default() -> Self {
        Self::new()
    }
}

impl Observer for Bar {
    fn begin(&self, phase: Phase, total: Option<u64>) {
        let Some(state) = &self.state else { return };
        let Ok(mut st) = state.lock() else { return };
        st.phase = Some(phase);
        st.total = total;
        st.done = 0;
        // Force the first paint so a slow phase announces itself immediately
        // rather than after the first REDRAW window.
        self.draw(&mut st, true);
    }

    fn advance(&self, _phase: Phase, by: u64) {
        let Some(state) = &self.state else { return };
        let Ok(mut st) = state.lock() else { return };
        st.done += by;
        self.draw(&mut st, false);
    }

    fn finish(&self, _phase: Phase) {
        let Some(state) = &self.state else { return };
        let Ok(mut st) = state.lock() else { return };
        // Leave the line blank: the command's own summary is the record of what
        // happened, and a leftover bar above it just adds noise.
        if st.dirty {
            let _ = self.term.clear_line();
            st.dirty = false;
        }
        st.phase = None;
    }
}
