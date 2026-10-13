//! Waiting for the repository write lock, and describing whoever holds it.
//!
//! Core's `write()` fails fast so a library caller never blocks. A command
//! line tool is different: two velo processes overlapping for a few tens of
//! milliseconds should not surface as an error, so the CLI waits a bounded
//! time with a jittered backoff before giving up.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use velo_core::{Repo, Result, WriteGuard};

/// How long to wait for the lock when `VELO_LOCK_WAIT_MS` is not set.
const DEFAULT_WAIT_MS: u64 = 3000;

fn wait_budget() -> Duration {
    let ms = std::env::var("VELO_LOCK_WAIT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_WAIT_MS);
    Duration::from_millis(ms)
}

/// The sleep before retry number `attempt` (0 for the first): a base that
/// doubles from 10 ms up to 80 ms, plus up to a quarter of it taken from
/// `jitter` so contending processes do not retry in lockstep. A pure function,
/// so the schedule is unit-testable; the 3 s cap is applied by the caller.
fn backoff_for(attempt: u32, jitter: u32) -> Duration {
    let base_ms = (10u64 << attempt.min(3)).min(80);
    let span = base_ms / 4 + 1;
    Duration::from_millis(base_ms + u64::from(jitter) % span)
}

fn backoff(attempt: u32) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    backoff_for(attempt, nanos)
}

/// Take the write lock, retrying for the configured budget. On timeout this
/// returns `repo.write()`'s error, which is the typed `Locked` naming the holder.
pub fn acquire(repo: &Repo) -> Result<WriteGuard<'_>> {
    let budget = wait_budget();
    let start = Instant::now();
    let mut attempt = 0u32;
    loop {
        if let Some(g) = repo.try_write()? {
            return Ok(g);
        }
        let left = budget.saturating_sub(start.elapsed());
        if left.is_zero() {
            return repo.write();
        }
        std::thread::sleep(backoff(attempt).min(left));
        attempt += 1;
    }
}

/// The command line recorded in the lock holder file: `velo ` plus the
/// arguments, whitespace-containing ones single-quoted, cut to 200 chars.
pub fn command_label() -> String {
    let mut s = String::from("velo");
    for a in std::env::args().skip(1) {
        s.push(' ');
        if a.chars().any(char::is_whitespace) {
            s.push('\'');
            s.push_str(&a);
            s.push('\'');
        } else {
            s.push_str(&a);
        }
    }
    s.chars().take(200).collect()
}

/// A short age such as "45s", "2m" or "3h".
pub fn age(started_at_ms: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(started_at_ms);
    let secs = ((now - started_at_ms) / 1000).max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// Whether a process with this id exists. `None` when it cannot be told.
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> Option<bool> {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let r = unsafe { kill(pid as i32, 0) };
    if r == 0 {
        return Some(true);
    }
    // EPERM: it exists but belongs to someone else.
    Some(std::io::Error::last_os_error().raw_os_error() == Some(1))
}

#[cfg(windows)]
pub fn pid_alive(pid: u32) -> Option<bool> {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn GetExitCodeProcess(h: *mut core::ffi::c_void, code: *mut u32) -> i32;
        fn CloseHandle(h: *mut core::ffi::c_void) -> i32;
    }
    unsafe {
        let h = OpenProcess(0x1000, 0, pid);
        if h.is_null() {
            // Access denied means the process exists.
            return Some(std::io::Error::last_os_error().raw_os_error() == Some(5));
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        Some(code == 259)
    }
}

#[cfg(not(any(unix, windows)))]
pub fn pid_alive(_pid: u32) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_plateaus() {
        let base = |a| backoff_for(a, 0).as_millis();
        assert_eq!(
            [base(0), base(1), base(2), base(3), base(4), base(1000)],
            [10, 20, 40, 80, 80, 80]
        );
    }

    #[test]
    fn jitter_stays_within_a_quarter_of_the_base() {
        for attempt in 0..8 {
            let lo = backoff_for(attempt, 0);
            for j in [1, 7, 999_999_999, u32::MAX] {
                let d = backoff_for(attempt, j);
                assert!(
                    d >= lo && d <= lo + lo / 4,
                    "attempt {attempt} jitter {j}: {d:?}"
                );
            }
        }
    }
}
