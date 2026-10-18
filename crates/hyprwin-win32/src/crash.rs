//! Crash reports: small text files in `<state_dir>\crashes`, written when the
//! daemon panics, when the guard sees it die unexpectedly, and when the
//! emergency key is used. The Settings window's Feedback tab sends them.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ledger::state_dir;

/// Reports kept; older ones are deleted.
const KEEP: usize = 20;
/// Log lines copied into a report.
const LOG_TAIL: usize = 150;

pub fn crashes_dir() -> PathBuf {
    state_dir().join("crashes")
}

/// `YYYY-MM-DD HH:MM:SS` (UTC) for `t`, without a date library.
pub fn utc_stamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, (rem / 60) % 60, rem % 60)
}

/// The last `n` lines of the daemon's log (current file, plus the rotated
/// one if the current file is short).
pub fn log_tail(n: usize) -> String {
    let dir = state_dir();
    let mut text = std::fs::read(dir.join("hyprwin.log")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    if text.lines().count() < n {
        if let Ok(old) = std::fs::read(dir.join("hyprwin.log.old")) {
            text = String::from_utf8_lossy(&old).into_owned() + &text;
        }
    }
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Writes a report and returns its path. `kind` is a short title such as
/// "crash" or "stopped unexpectedly"; `details` is free text.
pub fn write_report(kind: &str, details: &str) -> Option<PathBuf> {
    let dir = crashes_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let now = SystemTime::now();
    let stamp = utc_stamp(now);
    let file = dir.join(format!("{}-{}.txt", stamp.replace([':', ' '], "-"), kind.replace(|c: char| !c.is_ascii_alphanumeric(), "-")));
    let mut f = std::fs::File::create(&file).ok()?;
    let _ = writeln!(f, "hyprwin {} - {kind}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(f, "time: {stamp} UTC");
    let _ = writeln!(f, "process: {}", std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default());
    let _ = writeln!(f, "\n{details}\n\n---- last {LOG_TAIL} log lines ----\n{}", log_tail(LOG_TAIL));
    drop(f);
    prune();
    Some(file)
}

/// Saved reports, newest first.
pub fn reports() -> Vec<PathBuf> {
    let mut v: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(crashes_dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
                .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), e.path()))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, p)| p).collect()
}

fn prune() {
    for old in reports().into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(old);
    }
}

/// Reports written since the last call (used to tell the user once, at the
/// next start, that something went wrong). Remembers the time in a marker.
pub fn new_since_last_check() -> usize {
    let marker = crashes_dir().join(".seen");
    let seen = std::fs::metadata(&marker).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
    let n = std::fs::read_dir(crashes_dir())
        .map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "txt")).filter(|e| e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t > seen)).count())
        .unwrap_or(0);
    if n > 0 {
        let _ = std::fs::write(&marker, b"");
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn stamps_are_correct_dates() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "1970-01-01 00:00:00");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29 00:00:00");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(1_792_367_999)), "2026-10-18 23:59:59");
    }
}
