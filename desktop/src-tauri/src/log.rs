//! The app's log: what it did, for bug reports. It holds what the app opened and what failed
//! (no audio, no parameter turns), so it stays short.
//!
//! The recent lines are kept in memory for the Help panel's report, and written to
//! `klirr.log` in the app's log folder as they come, so they outlive a crash. Each start
//! moves the last run's file to `klirr.previous.log`; a run that ended cleanly says so in its
//! last line, so the report includes the previous run only when it did not (a crash, or the
//! process was killed).
//!
//! Lines show the user's home folder as `~`, so a report does not carry their user name.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Lines kept in memory for the report.
const KEEP: usize = 300;
/// Lines written to the file per run, so a loop of errors cannot fill the disk.
const FILE_LIMIT: usize = 5000;
/// The last line of a run that ended cleanly.
const CLEAN_EXIT: &str = "exit: clean";
/// Lines of the previous run included in a report.
const PREVIOUS_TAIL: usize = 60;

struct Log {
    lines: VecDeque<String>,
    file: Option<File>,
    written: usize,
    home: Option<String>,
    path: Option<PathBuf>,
    /// The previous run's last lines, if it did not end cleanly.
    previous: Option<Vec<String>>,
}

static LOG: Mutex<Log> = Mutex::new(Log {
    lines: VecDeque::new(),
    file: None,
    written: 0,
    home: None,
    path: None,
    previous: None,
});

/// Opens the log file in `dir` (keeping the last run's as the previous one) and logs panics.
/// Lines logged before this are kept in memory and written once the file is open.
pub fn init(dir: Option<&Path>, home: Option<&Path>) {
    {
        let Ok(mut log) = LOG.lock() else { return };
        log.home = home
            .map(|h| h.display().to_string())
            .filter(|h| h.len() > 1);
        if let Some(dir) = dir {
            let path = dir.join("klirr.log");
            let previous = dir.join("klirr.previous.log");
            let _ = fs::create_dir_all(dir);
            if path.exists() {
                let _ = fs::rename(&path, &previous);
                log.previous = unclean_tail(&previous);
            }
            log.file = File::create(&path).ok();
            log.path = Some(path);
            let pending: Vec<String> = log.lines.iter().cloned().collect();
            for line in pending {
                log.write(&line);
            }
        }
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        error(format!("panic: {info}"));
        default(info);
    }));
}

/// The last lines of a log file that did not end cleanly; None if it did, or is unreadable.
fn unclean_tail(path: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.last().is_some_and(|l| l.ends_with(CLEAN_EXIT)) || lines.is_empty() {
        return None;
    }
    let start = lines.len().saturating_sub(PREVIOUS_TAIL);
    Some(lines[start..].iter().map(|&l| l.to_owned()).collect())
}

impl Log {
    fn push(&mut self, level: &str, message: &str) {
        let mut message = message.to_owned();
        if let Some(home) = &self.home {
            message = message.replace(home.as_str(), "~");
        }
        let line = format!("{} {level} {message}", timestamp(SystemTime::now()));
        self.write(&line);
        if self.lines.len() == KEEP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    fn write(&mut self, line: &str) {
        let Some(file) = &mut self.file else { return };
        if self.written < FILE_LIMIT {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        } else if self.written == FILE_LIMIT {
            let _ = writeln!(
                file,
                "(log limit reached: later lines are kept in memory only)"
            );
        }
        self.written += 1;
    }
}

fn push(level: &str, message: &str) {
    if let Ok(mut log) = LOG.lock() {
        log.push(level, message);
    }
}

/// Logs what the app did.
pub fn info(message: impl AsRef<str>) {
    push("INFO ", message.as_ref());
}

/// Logs what failed.
pub fn error(message: impl AsRef<str>) {
    push("ERROR", message.as_ref());
}

/// Logs a line from the UI.
pub fn ui(message: impl AsRef<str>) {
    push("UI   ", message.as_ref());
}

/// Marks the run as ended cleanly: the last line of the file.
pub fn exit() {
    push("INFO ", CLEAN_EXIT);
}

/// What a report includes.
pub struct Snapshot {
    pub lines: Vec<String>,
    pub previous: Option<Vec<String>>,
    pub path: Option<String>,
}

/// The recent lines, the previous run's if it did not end cleanly, and where the file is
/// (with the home folder as `~`).
pub fn snapshot() -> Snapshot {
    let Ok(log) = LOG.lock() else {
        return Snapshot {
            lines: Vec::new(),
            previous: None,
            path: None,
        };
    };
    let path = log.path.as_ref().map(|p| {
        let path = p.display().to_string();
        match &log.home {
            Some(home) => path.replace(home.as_str(), "~"),
            None => path,
        }
    });
    Snapshot {
        lines: log.lines.iter().cloned().collect(),
        previous: log.previous.clone(),
        path,
    }
}

/// `2026-11-24 18:03:07Z`: UTC, to the second.
fn timestamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days);
    let mut out = String::with_capacity(20);
    let _ = write!(
        out,
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    );
    out
}

/// The calendar date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_utc_dates() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01 00:00:00Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_795_545_787);
        assert_eq!(timestamp(t), "2026-11-24 18:43:07Z");
        // A leap day.
        let t = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(timestamp(t), "2000-02-29 00:00:00Z");
    }

    #[test]
    fn only_a_run_that_did_not_end_cleanly_is_reported() {
        let dir = std::env::temp_dir().join(format!("klirr-log-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("run.log");
        fs::write(&file, format!("a\nb\nx INFO  {CLEAN_EXIT}\n")).unwrap();
        assert_eq!(unclean_tail(&file), None);
        let many: String = (0..100).map(|i| format!("line {i}\n")).collect();
        fs::write(&file, many).unwrap();
        let tail = unclean_tail(&file).unwrap();
        assert_eq!(tail.len(), PREVIOUS_TAIL);
        assert_eq!(tail.last().unwrap(), "line 99");
        fs::write(&file, "").unwrap();
        assert_eq!(unclean_tail(&file), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_home_folder_is_hidden() {
        let mut log = Log {
            lines: VecDeque::new(),
            file: None,
            written: 0,
            home: Some("C:\\Users\\someone".to_owned()),
            path: None,
            previous: None,
        };
        log.push("INFO ", "settings in C:\\Users\\someone\\AppData");
        assert!(log.lines[0].ends_with("INFO  settings in ~\\AppData"));
        for i in 0..KEEP + 5 {
            log.push("INFO ", &format!("{i}"));
        }
        assert_eq!(log.lines.len(), KEEP);
    }
}
