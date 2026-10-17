//! A log file that never grows past a fixed size.
//!
//! Two files take turns: `<name>.log` is written to, and when the next write would take it past
//! the size limit it becomes `<name>.1.log` (replacing the previous one) and a new `<name>.log` is
//! started. Together they never hold more than twice the limit, and the newest lines are always at
//! the end of `<name>.log`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct RotatingLogFile {
    path: PathBuf,
    previous: PathBuf,
    max_bytes: u64,
    file: File,
    written: u64,
}

impl RotatingLogFile {
    /// Opens `<dir>/<name>.log` for appending, creating `dir` if needed. A file left by an earlier
    /// run is continued, and its size counts towards the limit.
    pub fn open(dir: &Path, name: &str, max_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("{name}.log"));
        let previous = dir.join(format!("{name}.1.log"));
        let file = append(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path,
            previous,
            max_bytes,
            file,
            written,
        })
    }

    /// The file being written to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        // Windows refuses to rename onto an existing file, so the old backup goes first.
        match fs::remove_file(&self.previous) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        fs::rename(&self.path, &self.previous)?;
        self.file = append(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

/// The formatter hands over one whole event per `write`, so rotating between writes never splits a
/// line across the two files.
impl Write for RotatingLogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written > 0 && self.written + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own per test, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("ds2000-log-file-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn writes_below_the_limit_stay_in_one_file() {
        let dir = TempDir::new("below");
        let mut log = RotatingLogFile::open(&dir.0, "app", 100).unwrap();
        log.write_all(b"one\n").unwrap();
        log.write_all(b"two\n").unwrap();

        assert_eq!(read(&dir.0.join("app.log")), "one\ntwo\n");
        assert!(!dir.0.join("app.1.log").exists());
    }

    #[test]
    fn a_write_past_the_limit_moves_the_file_aside_first() {
        let dir = TempDir::new("rotate");
        let mut log = RotatingLogFile::open(&dir.0, "app", 10).unwrap();
        log.write_all(b"12345678\n").unwrap();
        log.write_all(b"next\n").unwrap();

        assert_eq!(read(&dir.0.join("app.1.log")), "12345678\n");
        assert_eq!(read(&dir.0.join("app.log")), "next\n");
    }

    #[test]
    fn only_one_old_file_is_kept() {
        let dir = TempDir::new("one-backup");
        let mut log = RotatingLogFile::open(&dir.0, "app", 6).unwrap();
        for line in ["aaaaa\n", "bbbbb\n", "ccccc\n"] {
            log.write_all(line.as_bytes()).unwrap();
        }

        assert_eq!(read(&dir.0.join("app.1.log")), "bbbbb\n");
        assert_eq!(read(&dir.0.join("app.log")), "ccccc\n");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
    }

    #[test]
    fn an_event_larger_than_the_limit_is_still_written_whole() {
        let dir = TempDir::new("large");
        let mut log = RotatingLogFile::open(&dir.0, "app", 4).unwrap();
        log.write_all(b"a much longer line\n").unwrap();

        assert_eq!(read(&dir.0.join("app.log")), "a much longer line\n");
    }

    #[test]
    fn a_file_from_an_earlier_run_counts_towards_the_limit() {
        let dir = TempDir::new("reopen");
        RotatingLogFile::open(&dir.0, "app", 10)
            .unwrap()
            .write_all(b"earlier\n")
            .unwrap();

        let mut log = RotatingLogFile::open(&dir.0, "app", 10).unwrap();
        log.write_all(b"later\n").unwrap();

        assert_eq!(read(&dir.0.join("app.1.log")), "earlier\n");
        assert_eq!(read(&dir.0.join("app.log")), "later\n");
    }
}
