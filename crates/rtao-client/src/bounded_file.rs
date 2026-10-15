//! Blocking, bounded acquisition of regular persisted files.
//!
//! Call these functions from an I/O worker. Regular-file symlinks are followed;
//! the opened descriptor is validated, and Unix special files are opened with
//! nonblocking mode so a FIFO cannot wait for a writer before validation.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

fn read_limit(maximum: u64) -> io::Result<u64> {
    maximum
        .checked_add(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file limit exceeds u64 range"))
}

fn oversized(maximum: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("persisted file exceeds {maximum} byte limit"),
    )
}

fn open(path: &Path, maximum: u64) -> io::Result<File> {
    read_limit(maximum)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "persisted input must be a regular file",
        ));
    }
    if metadata.len() > maximum {
        return Err(oversized(maximum));
    }
    Ok(file)
}

/// Read a regular file with both descriptor and streaming size checks.
///
/// Missing files retain their original `NotFound` error. Growth after the
/// descriptor check cannot make acquisition exceed `maximum + 1` bytes.
pub fn read(path: &Path, maximum: u64) -> io::Result<Vec<u8>> {
    let file = open(path, maximum)?;
    read_limited(file, maximum)
}

fn read_limited(input: impl Read, maximum: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(read_limit(maximum)?).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(oversized(maximum));
    }
    Ok(bytes)
}

/// Copy a regular file while enforcing the byte limit during acquisition.
///
/// An oversized or growing source may leave at most `maximum + 1` bytes in
/// `output` before returning an error. Use a disposable destination and publish
/// it only after this function and any required synchronization succeed.
pub fn copy(path: &Path, output: &mut impl Write, maximum: u64) -> io::Result<u64> {
    let input = open(path, maximum)?;
    copy_limited(input, output, maximum)
}

fn copy_limited(input: impl Read, output: &mut impl Write, maximum: u64) -> io::Result<u64> {
    let copied = io::copy(&mut input.take(read_limit(maximum)?), output)?;
    if copied > maximum {
        return Err(oversized(maximum));
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> io::Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"))
                .join("tmp");
            fs::create_dir_all(&root)?;
            loop {
                let path = root.join(format!(
                    "bounded-file-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn regular_reads_and_copies_enforce_limits_before_touching_the_destination() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let path = fixture.0.join("data");
        fs::write(&path, b"abcd")?;
        assert_eq!(read(&path, 4)?, b"abcd");
        let mut output = Vec::new();
        assert_eq!(copy(&path, &mut output, 4)?, 4);
        assert_eq!(output, b"abcd");
        assert!(read(&path, 3).is_err());
        assert!(copy(&path, &mut output, 3).is_err());
        assert_eq!(output, b"abcd");
        assert!(read(&fixture.0, 4).is_err());
        assert_eq!(
            read(&fixture.0.join("absent"), 4)
                .err()
                .ok_or_else(|| io::Error::other("missing file accepted"))?
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(fs::read(path)?, b"abcd");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn regular_symlinks_preserve_bounded_reads_and_special_targets_are_refused() -> io::Result<()> {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new()?;
        let source = fixture.0.join("source");
        let linked = fixture.0.join("linked");
        fs::write(&source, b"abcd")?;
        symlink(&source, &linked)?;
        assert_eq!(read(&linked, 4)?, b"abcd");
        assert!(read(&linked, 3).is_err());
        assert!(read(Path::new("/dev/null"), 4).is_err());
        Ok(())
    }

    #[test]
    fn streaming_limit_handles_growth_and_exact_boundary_without_unbounded_reads() -> io::Result<()>
    {
        assert_eq!(read_limited(&b"abcd"[..], 4)?, b"abcd");
        assert_eq!(read_limited(&b""[..], 0)?, b"");
        assert_eq!(
            read_limited(&b"abcde ignored tail"[..], 4)
                .err()
                .ok_or_else(|| io::Error::other("oversized stream accepted"))?
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(read_limited(&b"x"[..], u64::MAX).is_err());
        Ok(())
    }

    #[test]
    fn file_growth_after_descriptor_validation_stops_after_one_excess_byte() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let path = fixture.0.join("growing");
        fs::write(&path, b"abcd")?;
        let reader = open(&path, 4)?;
        let copier = open(&path, 4)?;
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"e unread tail")?;
        assert_eq!(
            read_limited(reader, 4)
                .err()
                .ok_or_else(|| io::Error::other("growing file accepted"))?
                .kind(),
            io::ErrorKind::InvalidData
        );
        let mut output = Vec::new();
        assert_eq!(
            copy_limited(copier, &mut output, 4)
                .err()
                .ok_or_else(|| io::Error::other("growing copy accepted"))?
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(output, b"abcde");
        assert_eq!(fs::read(path)?, b"abcde unread tail");
        Ok(())
    }

    // Isolate the acquisition so a regression to blocking FIFO opens can be
    // killed and reaped before the fixture is removed.
    #[cfg(unix)]
    #[test]
    fn fifo_child_process() -> io::Result<()> {
        let Some(path) = std::env::var_os("RTAO_BOUNDED_FIFO_CHILD") else {
            return Ok(());
        };
        let path = PathBuf::from(path);
        let read_error = read(&path, 4)
            .err()
            .ok_or_else(|| io::Error::other("FIFO read accepted"))?;
        assert_eq!(read_error.kind(), io::ErrorKind::InvalidData);
        let mut output = Vec::new();
        let copy_error = copy(&path, &mut output, 4)
            .err()
            .ok_or_else(|| io::Error::other("FIFO copy accepted"))?;
        assert_eq!(copy_error.kind(), io::ErrorKind::InvalidData);
        assert!(output.is_empty());
        fs::write(path.with_extension("checked"), b"read-and-copy-refused")?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn fifos_without_writers_are_refused_without_blocking() -> io::Result<()> {
        use std::{
            os::unix::fs::FileTypeExt,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };

        let fixture = Fixture::new()?;
        let path = fixture.0.join("fifo");
        assert!(Command::new("mkfifo").arg(&path).status()?.success());
        let mut child = Command::new(std::env::current_exe()?)
            .args(["--exact", "bounded_file::tests::fifo_child_process"])
            .env("RTAO_BOUNDED_FIFO_CHILD", &path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let result = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Err(error) => break Err(error),
                Ok(None) if Instant::now() >= deadline => {
                    break Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "bounded file acquisition waited for a FIFO writer",
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let _ = child.kill();
        let reaped = child.wait();
        let status = result?;
        reaped?;
        assert!(status.success());
        assert_eq!(
            fs::read(path.with_extension("checked"))?,
            b"read-and-copy-refused"
        );
        assert!(fs::symlink_metadata(path)?.file_type().is_fifo());
        Ok(())
    }
}
