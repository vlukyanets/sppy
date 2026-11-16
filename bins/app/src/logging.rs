//! Logs go to stderr and to `sppy.log` in the data directory. The file is size-limited:
//! when it reaches [`MAX_FILE_BYTES`] it becomes `sppy.log.1` (older ones shift up to
//! `sppy.log.<KEEP_OLD>`), so logs never take more than `(KEEP_OLD + 1) * MAX_FILE_BYTES`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const FILE_NAME: &str = "sppy.log";
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const KEEP_OLD: usize = 3;

pub fn init(logs_dir: &Path) -> Option<WorkerGuard> {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr =
        tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_filter(filter());
    let file = std::fs::create_dir_all(logs_dir).and_then(|()| {
        remove_daily_logs(logs_dir);
        SizeLimitedFile::open(logs_dir, FILE_NAME, MAX_FILE_BYTES, KEEP_OLD)
    });
    match file {
        Ok(file) => {
            let (writer, guard) = tracing_appender::non_blocking(file);
            let file = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(filter());
            tracing_subscriber::registry().with(stderr).with(file).init();
            Some(guard)
        }
        Err(e) => {
            tracing_subscriber::registry().with(stderr).init();
            tracing::warn!("cannot write logs to {}: {e}", logs_dir.display());
            None
        }
    }
}

/// Earlier versions wrote one file per day (`sppy.log.2026-11-15`) and never removed them.
fn remove_daily_logs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(date) = name.to_str().and_then(|n| n.strip_prefix("sppy.log.")) else {
            continue;
        };
        let is_date = date.len() == 10
            && date
                .bytes()
                .enumerate()
                .all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
        if is_date {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// An append-only log file rotated by size.
pub struct SizeLimitedFile {
    path: PathBuf,
    max_bytes: u64,
    keep_old: usize,
    /// `None` only if reopening after a rotation failed; retried on the next write.
    file: Option<File>,
    size: u64,
}

impl SizeLimitedFile {
    pub fn open(dir: &Path, name: &str, max_bytes: u64, keep_old: usize) -> io::Result<Self> {
        let mut f =
            SizeLimitedFile { path: dir.join(name), max_bytes, keep_old, file: None, size: 0 };
        f.reopen(false)?;
        Ok(f)
    }

    fn reopen(&mut self, truncate: bool) -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.create(true);
        if truncate {
            options.write(true).truncate(true)
        } else {
            options.append(true)
        };
        let file = options.open(&self.path)?;
        self.size = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    fn old(&self, n: usize) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(format!(".{n}"));
        p.into()
    }

    /// `name.(k-1)` → `name.k`, …, `name` → `name.1`, then start an empty `name`.
    fn rotate(&mut self) -> io::Result<()> {
        // Close first: Windows cannot rename an open file.
        self.file = None;
        let mut renamed = false;
        if self.keep_old > 0 {
            let _ = std::fs::remove_file(self.old(self.keep_old));
            for n in (1..self.keep_old).rev() {
                let _ = std::fs::rename(self.old(n), self.old(n + 1));
            }
            renamed = std::fs::rename(&self.path, self.old(1)).is_ok();
        }
        // If renaming failed, start over in the same file rather than grow past the limit.
        self.reopen(!renamed)
    }
}

impl Write for SizeLimitedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.file.is_none() {
            self.reopen(false)?;
        }
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let file = self.file.as_mut().expect("opened above");
        let n = file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), File::flush)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes(dir: &Path) -> Vec<(String, u64)> {
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| (e.file_name().into_string().unwrap(), e.metadata().unwrap().len()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn rotates_by_size_and_keeps_a_bounded_number_of_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = SizeLimitedFile::open(dir.path(), "sppy.log", 100, 2).unwrap();
        for i in 0..50 {
            writeln!(f, "line {i:04} xxxxxxxxxxxxxxxxxxxxxxxxx").unwrap();
        }
        f.flush().unwrap();
        let files = sizes(dir.path());
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["sppy.log", "sppy.log.1", "sppy.log.2"]);
        assert!(files.iter().all(|(_, s)| *s <= 100), "{files:?}");
        let newest = std::fs::read_to_string(dir.path().join("sppy.log")).unwrap();
        assert!(newest.contains("line 0049"), "the newest lines stay in sppy.log");
        let older = std::fs::read_to_string(dir.path().join("sppy.log.1")).unwrap();
        assert!(!older.contains("line 0049") && older.contains("line 004"));
    }

    #[test]
    fn continues_an_existing_file_and_counts_its_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sppy.log"), vec![b'x'; 90]).unwrap();
        let mut f = SizeLimitedFile::open(dir.path(), "sppy.log", 100, 1).unwrap();
        f.write_all(b"0123456789abcdef\n").unwrap();
        assert_eq!(sizes(dir.path()), [("sppy.log".into(), 17), ("sppy.log.1".into(), 90)]);
    }

    #[test]
    fn without_old_files_the_log_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = SizeLimitedFile::open(dir.path(), "sppy.log", 20, 0).unwrap();
        f.write_all(b"first line......\n").unwrap();
        f.write_all(b"second line.....\n").unwrap();
        let text = std::fs::read_to_string(dir.path().join("sppy.log")).unwrap();
        assert_eq!(text, "second line.....\n");
        assert_eq!(sizes(dir.path()).len(), 1);
    }

    #[test]
    fn old_daily_logs_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["sppy.log.2026-11-13", "sppy.log.2026-11-15", "sppy.log", "sppy.log.1", "notes"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        remove_daily_logs(dir.path());
        let names: Vec<_> = sizes(dir.path()).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["notes", "sppy.log", "sppy.log.1"]);
    }
}
