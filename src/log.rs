//! What the server says of itself: on stderr, as ever, and with `--logs DIR` in a file there
//! too, each line with when it was said. The file outlives the container (a deploy starts a new
//! one, and the old one's own log goes with it): `<DIR>/<name>.log`, set aside as `<name>.1.log`
//! once it is over [`MAX_BYTES`] (up to `.5`: the oldest goes).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// The most one file holds, and how many set aside are kept.
const MAX_BYTES: u64 = 8 << 20;
const KEPT: u32 = 5;

struct Kept {
    dir: PathBuf,
    name: String,
    file: File,
    bytes: u64,
    max: u64,
}

static LOG: OnceLock<Mutex<Kept>> = OnceLock::new();

impl Kept {
    fn open(dir: &Path, name: &str, max: u64) -> std::io::Result<Kept> {
        std::fs::create_dir_all(dir)?;
        let file = OpenOptions::new().create(true).append(true).open(dir.join(format!("{name}.log")))?;
        let bytes = file.metadata()?.len();
        Ok(Kept { dir: dir.to_path_buf(), name: name.to_string(), file, bytes, max })
    }

    fn path(&self, n: u32) -> PathBuf {
        self.dir.join(if n == 0 { format!("{}.log", self.name) } else { format!("{}.{n}.log", self.name) })
    }

    /// `text` as a line of its own; the file set aside first when it is full.
    fn say(&mut self, text: &str) -> std::io::Result<()> {
        if self.bytes >= self.max {
            for n in (0..KEPT).rev() {
                let _ = std::fs::rename(self.path(n), self.path(n + 1));
            }
            *self = Kept::open(&self.dir, &self.name, self.max)?;
        }
        let line = format!("{text}\n");
        self.file.write_all(line.as_bytes())?;
        self.bytes += line.len() as u64;
        Ok(())
    }
}

/// Keeps what is said from here on in `dir` too, as `<name>.log`.
pub(crate) fn keep_in(dir: &Path, name: &str) -> std::io::Result<()> {
    let kept = Kept::open(dir, name, MAX_BYTES)?;
    let _ = LOG.set(Mutex::new(kept));
    Ok(())
}

/// Says `text`: on stderr, and in the file if one is kept, with the time.
pub(crate) fn line(text: &str) {
    eprintln!("{text}");
    if let Some(log) = LOG.get() {
        let mut log = log.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = log.say(&format!("{} {text}", stamp(SystemTime::now())));
    }
}

/// `at` as `2026-11-12T08:00:23Z`.
fn stamp(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86400, secs % 86400);
    // Days since 1970 to a date of the calendar (Howard Hinnant's `civil_from_days`).
    let z = days as i64 + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Says a line as `stvtf-web: …`, formatted as `println!` would.
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::line(&format!("stvtf-web: {}", format_args!($($arg)*)))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use std::time::Duration;

    #[test]
    fn a_time_is_written_as_a_date_and_a_time_of_day() {
        assert_eq!(stamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(stamp(UNIX_EPOCH + Duration::from_secs(1_794_441_623)), "2026-11-12T00:00:23Z");
        assert_eq!(stamp(UNIX_EPOCH + Duration::from_secs(1_709_251_199)), "2024-02-29T23:59:59Z");
    }

    #[test]
    fn a_full_file_is_set_aside_and_only_so_many_are_kept() {
        let dir = TempDir::new("logs");
        let mut log = Kept::open(&dir, "web", 20).unwrap();
        for n in 0..40 {
            log.say(&format!("line number {n:02}")).unwrap();
        }
        let mut names: Vec<String> =
            std::fs::read_dir(&*dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["web.1.log", "web.2.log", "web.3.log", "web.4.log", "web.5.log", "web.log"]);
        assert!(std::fs::read_to_string(dir.join("web.log")).unwrap().ends_with("line number 39\n"));
        // What was there is added to, not written over, when the server starts again.
        Kept::open(&dir, "web", 1000).unwrap().say("after a restart").unwrap();
        assert!(std::fs::read_to_string(dir.join("web.log")).unwrap().contains("line number 39\nafter a restart\n"));
    }
}
