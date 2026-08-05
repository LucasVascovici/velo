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
//!   unless *stderr* is a terminal as a second line of defence.
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
    /// `None` unless stderr is a terminal, which makes every method a no-op.
    state: Option<Mutex<State>>,
}

struct State {
    phase: Option<Phase>,
    total: Option<u64>,
    done: u64,
    last_drawn: Instant,
    /// Whether a line is currently on screen and needs clearing.
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
        // Truncate rather than wrap: a wrapped bar leaves debris behind.
        let width = self.term.size().1 as usize;
        let line: String = line(phase, st.done, st.total)
            .chars()
            .take(width.saturating_sub(1))
            .collect();
        let _ = self.term.clear_line();
        let _ = write!(&self.term, "  {}\r", line);
        let _ = std::io::stderr().flush();
        st.last_drawn = Instant::now();
        st.dirty = true;
    }
}

/// The text of the bar, as a pure function of what is known.
///
/// Separated from drawing so it can be tested without a terminal — which is the
/// only part of a progress bar where a bug can hide.
fn line(phase: Phase, done: u64, total: Option<u64>) -> String {
    match total {
        Some(total) if total > 0 => {
            let pct = (done.min(total) * 100) / total;
            if is_bytes(phase) {
                format!(
                    "{} {}/{} ({}%)",
                    phase,
                    super::gc::human_size(done),
                    super::gc::human_size(total),
                    pct
                )
            } else {
                format!("{} {}/{} {} ({}%)", phase, done, total, phase.unit(), pct)
            }
        }
        // Indeterminate: a count with no denominator. `Transferring` and `gc`
        // land here — the pack is framed by EOF and the object store is streamed,
        // so neither size is known in advance.
        _ => format!("{} {}…", phase, amount(phase, done)),
    }
}

/// A byte count reads as "4.6 MB"; anything else keeps its unit noun.
fn amount(phase: Phase, n: u64) -> String {
    if is_bytes(phase) {
        super::gc::human_size(n)
    } else {
        format!("{} {}", n, phase.unit())
    }
}

fn is_bytes(phase: Phase) -> bool {
    phase.unit() == "bytes"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_total_shows_a_percentage() {
        assert_eq!(
            line(Phase::Hashing, 12, Some(40)),
            "Hashing 12/40 files (30%)"
        );
        assert_eq!(
            line(Phase::Replaying, 2, Some(2)),
            "Replaying 2/2 commits (100%)"
        );
    }

    #[test]
    fn an_unknown_total_shows_a_running_count() {
        assert_eq!(
            line(Phase::Collecting, 812, None),
            "Collecting 812 objects…"
        );
    }

    #[test]
    fn byte_counts_are_human_readable() {
        // The transfer phase counts bytes, and "Transferring 4823901 bytes…" is
        // not something anyone can read at a glance.
        assert_eq!(
            line(Phase::Transferring, 4_823_901, None),
            "Transferring 4.6 MB…"
        );
        assert_eq!(line(Phase::Transferring, 512, None), "Transferring 512 B…");
        assert_eq!(
            line(Phase::Transferring, 5_242_880, Some(10_485_760)),
            "Transferring 5.0 MB/10.0 MB (50%)"
        );
    }

    #[test]
    fn a_zero_total_does_not_divide_by_zero() {
        // An empty phase is announced with Some(0) — hashing a clean tree, say.
        assert_eq!(line(Phase::Hashing, 0, Some(0)), "Hashing 0 files…");
    }

    #[test]
    fn overshooting_the_total_is_clamped() {
        // Belt and braces: a miscounted total must not print 140%.
        assert_eq!(
            line(Phase::Writing, 14, Some(10)),
            "Writing 14/10 files (100%)"
        );
    }

    #[test]
    fn a_bar_is_inert_without_a_terminal() {
        // The tests never run on a TTY, so this exercises the real gate.
        let bar = Bar::new();
        assert!(
            bar.state.is_none(),
            "piped output must not get a bar — serve-* speaks a protocol on stdout"
        );
        // Every method must be a no-op rather than panicking.
        bar.begin(Phase::Hashing, Some(3));
        bar.advance(Phase::Hashing, 1);
        bar.finish(Phase::Hashing);
    }
}
