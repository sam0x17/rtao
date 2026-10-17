// Derived from RaoFoundation/subtensor sdk/python/bittensor/masked_input.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Star-echo secret editor with injected input and output boundaries.

#[cfg(any(unix, test))]
use super::TerminalCancel;
#[cfg(any(unix, test))]
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::io::{self, BufRead, Write};
use zeroize::{Zeroize, Zeroizing};

#[cfg(any(unix, test))]
const ERASE: &str = "\u{8} \u{8}";

#[cfg(any(unix, test))]
trait MaskedIo {
    fn check_cancelled(&self) -> io::Result<()> {
        Ok(())
    }
    fn cancel(&self) {}
    fn begin(&mut self) -> io::Result<()>;
    fn restore(&mut self) -> io::Result<()>;
    fn read_byte(&mut self, byte: &mut [u8; 1]) -> io::Result<usize>;
    fn escape_byte_ready(&mut self) -> io::Result<bool>;
    fn echo(&mut self, text: &str) -> io::Result<()>;
}

#[derive(Clone, Copy)]
#[cfg(any(unix, test))]
enum Feedback {
    Stars,
    Hidden,
}

#[cfg(any(unix, test))]
impl Feedback {
    fn echo(self, io: &mut impl MaskedIo, text: &str) -> io::Result<()> {
        match self {
            Self::Stars => io.echo(text),
            Self::Hidden => Ok(()),
        }
    }
}

// Each entry is one Python incremental-decoder result. For malformed UTF-8 a
// result can contain a replacement character followed by an ASCII character;
// Python echoes one star and removes that whole result on backspace.
#[derive(Default)]
#[cfg(any(unix, test))]
struct Entry {
    chunks: Vec<SecretString>,
}
#[cfg(any(unix, test))]
impl Entry {
    fn push(&mut self, mut chunk: Zeroizing<String>) {
        self.chunks
            .push(SecretString::new(std::mem::take(&mut *chunk)));
    }

    fn finish(self) -> SecretString {
        let size = self
            .chunks
            .iter()
            .map(|chunk| chunk.expose_secret().len())
            .sum();
        // Allocate once so growth never abandons an unzeroized secret buffer.
        let mut result = Zeroizing::new(String::with_capacity(size));
        for chunk in &self.chunks {
            result.push_str(chunk.expose_secret());
        }
        SecretString::new(std::mem::take(&mut *result))
    }
}

#[derive(Default)]
#[cfg(any(unix, test))]
struct Utf8Decoder {
    pending: Zeroizing<[u8; 4]>,
    len: usize,
}
#[cfg(any(unix, test))]
impl Utf8Decoder {
    fn push(&mut self, byte: u8) -> Zeroizing<String> {
        self.pending[self.len] = byte;
        self.len += 1;
        let mut result = Zeroizing::new(String::with_capacity(7));
        loop {
            match std::str::from_utf8(&self.pending[..self.len]) {
                Ok(text) => {
                    result.push_str(text);
                    self.pending.zeroize();
                    self.len = 0;
                    return result;
                }
                Err(error) => match error.error_len() {
                    None => return result,
                    Some(invalid) => {
                        result.push('\u{fffd}');
                        self.pending.copy_within(invalid..self.len, 0);
                        self.len -= invalid;
                        self.pending[self.len..].zeroize();
                    }
                },
            }
        }
    }
}

#[cfg(any(unix, test))]
fn read_key_byte(io: &mut impl MaskedIo, byte: &mut [u8; 1]) -> io::Result<usize> {
    io.check_cancelled()?;
    let count = io.read_byte(byte)?;
    io.check_cancelled()?;
    // ISIG is disabled, so interruption must apply to every consumed byte,
    // including a byte swallowed while recognizing an escape sequence.
    if count != 0 && byte[0] == b'\x03' {
        return Err(io::ErrorKind::Interrupted.into());
    }
    Ok(count)
}

#[cfg(any(unix, test))]
fn escape_ready(io: &mut impl MaskedIo) -> io::Result<bool> {
    io.check_cancelled()?;
    let ready = io.escape_byte_ready()?;
    io.check_cancelled()?;
    Ok(ready)
}

#[cfg(any(unix, test))]
fn discard_escape(io: &mut impl MaskedIo) -> io::Result<()> {
    if !escape_ready(io)? {
        return Ok(());
    }
    let mut byte = Zeroizing::new([0; 1]);
    if read_key_byte(io, &mut byte)? == 0 || !matches!(byte[0], b'[' | b'O') {
        return Ok(());
    }
    while escape_ready(io)? {
        if read_key_byte(io, &mut byte)? == 0 {
            return Ok(());
        }
        if byte[0] != b'[' && (b'@'..=b'~').contains(&byte[0]) {
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(any(unix, test))]
fn edit(io: &mut impl MaskedIo, feedback: Feedback) -> io::Result<SecretString> {
    let mut entry = Entry::default();
    let mut decoder = Utf8Decoder::default();
    let mut byte = Zeroizing::new([0; 1]);
    loop {
        if read_key_byte(io, &mut byte)? == 0 || matches!(byte[0], b'\r' | b'\n') {
            return Ok(entry.finish());
        }
        match byte[0] {
            b'\x7f' | b'\x08' => {
                if entry.chunks.pop().is_some() {
                    feedback.echo(io, ERASE)?;
                }
            }
            b'\x15' => {
                for _ in &entry.chunks {
                    feedback.echo(io, ERASE)?;
                }
                entry.chunks.clear();
            }
            b'\x04' if entry.chunks.is_empty() => {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            b'\x04' => {}
            b'\x1b' => discard_escape(io)?,
            byte => {
                let decoded = decoder.push(byte);
                // Python compares the whole decoder result lexicographically.
                if !decoded.is_empty() && decoded.as_str() >= " " {
                    entry.push(decoded);
                    feedback.echo(io, "*")?;
                }
            }
        }
    }
}

#[cfg(any(unix, test))]
struct MaskedFailure {
    error: io::Error,
    // A failed restore must be reported; retrying would save the already
    // modified terminal mode as the new original state.
    may_fallback: bool,
}

#[cfg(any(unix, test))]
fn run_masked(io: &mut impl MaskedIo, prompt: &str) -> Result<SecretString, MaskedFailure> {
    run_terminal(io, prompt, Feedback::Stars)
}

#[cfg(any(unix, test))]
fn run_terminal(
    io: &mut impl MaskedIo,
    prompt: &str,
    feedback: Feedback,
) -> Result<SecretString, MaskedFailure> {
    io.check_cancelled().map_err(|error| MaskedFailure {
        error,
        may_fallback: false,
    })?;
    io.echo(prompt).map_err(|error| MaskedFailure {
        error,
        may_fallback: true,
    })?;
    let result = io.begin().and_then(|()| edit(io, feedback));
    let restored = io.restore();
    if matches!(&result, Err(error) if error.kind() == io::ErrorKind::Interrupted) {
        // Physical Ctrl-C is latched only after owned restoration was attempted,
        // including when its error must remain the primary diagnostic.
        io.cancel();
    }
    let newline = io.echo("\n");
    if let Err(error) = restored {
        return Err(MaskedFailure {
            error,
            may_fallback: false,
        });
    }
    io.check_cancelled().map_err(|error| MaskedFailure {
        error,
        may_fallback: false,
    })?;
    result
        .and_then(|secret| newline.map(|()| secret))
        .map_err(|error| {
            let may_fallback = !matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::UnexpectedEof
            );
            MaskedFailure {
                error,
                may_fallback,
            }
        })
}

#[cfg(any(unix, test))]
fn fallback_with<T>(
    prompt: &str,
    open_terminal: impl FnOnce() -> io::Result<T>,
    hidden: impl FnOnce(T, &str) -> io::Result<SecretString>,
    plain: impl FnOnce(&str) -> io::Result<SecretString>,
) -> io::Result<SecretString> {
    match open_terminal() {
        Ok(terminal) => hidden(terminal, prompt),
        Err(_) => plain(prompt),
    }
}

fn read_plain_line(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt: &str,
) -> io::Result<SecretString> {
    // Match getpass's visible warning before falling back to echoing input.
    output.write_all(b"Warning: Password input may be echoed.\n")?;
    output.write_all(prompt.as_bytes())?;
    output.flush()?;
    let mut bytes = Zeroizing::new(Vec::new());
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            break;
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |newline| newline + 1);
        let newline = available[count - 1] == b'\n';
        let required = bytes
            .len()
            .checked_add(count)
            .ok_or(io::ErrorKind::OutOfMemory)?;
        if required > bytes.capacity() {
            let capacity = required.max(bytes.capacity().saturating_mul(2));
            let mut replacement = Zeroizing::new(Vec::with_capacity(capacity));
            replacement.extend_from_slice(&bytes);
            // Wipe the old allocation before abandoning it. BufRead::read_line
            // into a growing String cannot provide this guarantee.
            bytes = replacement;
        }
        bytes.extend_from_slice(&available[..count]);
        input.consume(count);
        if newline {
            break;
        }
    }
    finish_plain_line(bytes)
}

fn finish_plain_line(mut bytes: Zeroizing<Vec<u8>>) -> io::Result<SecretString> {
    if bytes.last() == Some(&b'\n') {
        if let Some(byte) = bytes.last_mut() {
            byte.zeroize();
        }
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            if let Some(byte) = bytes.last_mut() {
                byte.zeroize();
            }
            bytes.pop();
        }
    }
    line_from_bytes(bytes).map(SecretString::new)
}

fn line_from_bytes(mut bytes: Zeroizing<Vec<u8>>) -> io::Result<String> {
    match String::from_utf8(std::mem::take(&mut *bytes)) {
        Ok(line) => Ok(line),
        Err(error) => {
            let _discarded = Zeroizing::new(error.into_bytes());
            Err(io::ErrorKind::InvalidData.into())
        }
    }
}

// The cancellable path consumes exactly one byte per call, so an ordinary
// prompt cannot leave a stdio read-ahead buffer ahead of a later secret read.
// Input scratch buffers, including a discarded UTF-8 error, are zeroizing.
#[cfg(any(unix, test))]
fn read_line_bytes(
    cancel: &TerminalCancel,
    mut read_byte: impl FnMut(&mut [u8; 1]) -> io::Result<usize>,
) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut byte = Zeroizing::new([0; 1]);
    loop {
        cancel.check_io()?;
        let count = read_byte(&mut byte)?;
        cancel.check_io()?;
        if count == 0 {
            return if bytes.is_empty() {
                Err(io::ErrorKind::UnexpectedEof.into())
            } else {
                Ok(bytes)
            };
        }
        let required = bytes
            .len()
            .checked_add(1)
            .ok_or(io::ErrorKind::OutOfMemory)?;
        if required > bytes.capacity() {
            let capacity = required.max(bytes.capacity().saturating_mul(2));
            let mut replacement = Zeroizing::new(Vec::with_capacity(capacity));
            replacement.extend_from_slice(&bytes);
            bytes = replacement;
        }
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(bytes);
        }
    }
}

#[cfg(unix)]
mod posix {
    use super::*;
    use nix::{
        errno::Errno,
        poll::{PollFd, PollFlags, poll},
        sys::termios::{
            LocalFlags, SetArg, SpecialCharacterIndices, Termios, tcgetattr, tcsetattr,
        },
    };
    use std::{
        fs::OpenOptions,
        io::IsTerminal,
        os::fd::AsFd,
        time::{Duration, Instant},
    };

    struct PosixIo<'a, Fd: AsFd> {
        input: &'a Fd,
        output: &'a mut dyn Write,
        original: Option<Termios>,
        cancel: Option<&'a TerminalCancel>,
    }
    impl<Fd: AsFd> PosixIo<'_, Fd> {
        fn mode_action(&self) -> SetArg {
            // Opt-in cancellation must not wait for pending output to drain.
            if self.cancel.is_some() {
                SetArg::TCSANOW
            } else {
                SetArg::TCSADRAIN
            }
        }
        fn restore_original(&mut self) -> io::Result<()> {
            if let Some(original) = &self.original {
                tcsetattr(self.input, self.mode_action(), original)?;
                self.original = None;
            }
            Ok(())
        }
    }
    impl<Fd: AsFd> MaskedIo for PosixIo<'_, Fd> {
        fn check_cancelled(&self) -> io::Result<()> {
            check_cancelled(self.cancel)
        }
        fn cancel(&self) {
            if let Some(cancel) = self.cancel {
                cancel.cancel();
            }
        }
        fn begin(&mut self) -> io::Result<()> {
            self.check_cancelled()?;
            let original = tcgetattr(self.input)?;
            let mut mode = original.clone();
            self.original = Some(original);
            mode.local_flags.remove(
                LocalFlags::ECHO | LocalFlags::ECHONL | LocalFlags::ICANON | LocalFlags::ISIG,
            );
            mode.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
            mode.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
            // read_key_byte handles the interrupt key in every parser state;
            // restore before returning rather than terminating before Drop.
            tcsetattr(self.input, self.mode_action(), &mode)?;
            Ok(())
        }
        fn restore(&mut self) -> io::Result<()> {
            self.restore_original()
        }
        fn read_byte(&mut self, byte: &mut [u8; 1]) -> io::Result<usize> {
            // Bypass StdinLock's read-ahead buffer: poll must see every byte
            // remaining after ESC, including an immediately following Enter.
            read_byte(self.input, self.cancel, byte)
        }
        fn escape_byte_ready(&mut self) -> io::Result<bool> {
            readable(self.input, self.cancel)
        }
        fn echo(&mut self, text: &str) -> io::Result<()> {
            self.output.write_all(text.as_bytes())?;
            self.output.flush()
        }
    }
    impl<Fd: AsFd> Drop for PosixIo<'_, Fd> {
        fn drop(&mut self) {
            // Normal paths report restoration errors. Drop also attempts it
            // after unwinding or a failed restoration.
            let _ = self.restore_original();
        }
    }

    fn check_cancelled(cancel: Option<&TerminalCancel>) -> io::Result<()> {
        cancel.map_or(Ok(()), TerminalCancel::check_io)
    }

    pub(super) fn readable(input: &impl AsFd, cancel: Option<&TerminalCancel>) -> io::Result<bool> {
        let started = Instant::now();
        wait_readable(
            cancel,
            || Duration::from_millis(50).saturating_sub(started.elapsed()),
            |timeout| {
                // PollFd carries the borrowed descriptor directly. Unlike
                // FdSet, it supports descriptors at or above FD_SETSIZE.
                let mut descriptors = [PollFd::new(input.as_fd(), PollFlags::POLLIN)];
                match poll(&mut descriptors, timeout) {
                    Ok(0) => Ok(None),
                    Ok(_) => descriptors[0].revents().map(Some).ok_or(Errno::EIO),
                    Err(error) => Err(error),
                }
            },
        )
    }

    pub(super) fn wait_readable(
        cancel: Option<&TerminalCancel>,
        mut remaining: impl FnMut() -> Duration,
        mut poll_once: impl FnMut(u16) -> nix::Result<Option<PollFlags>>,
    ) -> io::Result<bool> {
        loop {
            check_cancelled(cancel)?;
            let remaining = remaining();
            if remaining.is_zero() {
                return Ok(false);
            }
            // Round up so sub-millisecond time remains a bounded wait. Every
            // EINTR retry uses the same monotonic budget, including ESC waits.
            let timeout = u16::try_from(remaining.as_nanos().div_ceil(1_000_000))
                .unwrap_or(50)
                .min(50);
            let events = poll_once(timeout);
            check_cancelled(cancel)?;
            let events = match events {
                Err(Errno::EINTR) => continue,
                result => result?,
            };
            let Some(events) = events else {
                return Ok(false);
            };
            if events.contains(PollFlags::POLLNVAL) {
                return Err(Errno::EBADF.into());
            }
            // HUP can accompany buffered input. Let read drain it and then
            // observe EOF; treating HUP as a timeout would spin indefinitely.
            if events.intersects(PollFlags::POLLIN | PollFlags::POLLHUP) {
                return Ok(true);
            }
            if events.is_empty() {
                return Ok(false);
            }
            return Err(Errno::EIO.into());
        }
    }

    fn read_byte(
        input: &impl AsFd,
        cancel: Option<&TerminalCancel>,
        byte: &mut [u8; 1],
    ) -> io::Result<usize> {
        loop {
            check_cancelled(cancel)?;
            if cancel.is_some() {
                while !readable(input, cancel)? {}
            }
            let result = nix::unistd::read(input, byte);
            check_cancelled(cancel)?;
            match result {
                Err(Errno::EINTR) if cancel.is_some() => continue,
                result => return result.map_err(Into::into),
            }
        }
    }

    pub(super) fn read_line(prompt: &str, cancel: &TerminalCancel) -> io::Result<String> {
        cancel.check_io()?;
        let stdin = io::stdin().lock();
        let mut stderr = io::stderr().lock();
        stderr.write_all(prompt.as_bytes())?;
        stderr.flush()?;
        drop(stderr);
        let bytes = read_line_bytes(cancel, |byte| read_byte(&stdin, Some(cancel), byte))?;
        cancel.check_io()?;
        line_from_bytes(bytes)
    }

    pub(super) fn read(prompt: &str, cancel: Option<&TerminalCancel>) -> io::Result<SecretString> {
        check_cancelled(cancel)?;
        if io::stdin().is_terminal() {
            let stdin = io::stdin().lock();
            let mut stderr = io::stderr().lock();
            let mut terminal = PosixIo {
                input: &stdin,
                output: &mut stderr,
                original: None,
                cancel,
            };
            match run_masked(&mut terminal, prompt) {
                Ok(secret) => return Ok(secret),
                Err(error) if !error.may_fallback => return Err(error.error),
                Err(_) => {}
            }
        }
        fallback(prompt, cancel)
    }

    fn fallback(prompt: &str, cancel: Option<&TerminalCancel>) -> io::Result<SecretString> {
        check_cancelled(cancel)?;
        // Python's getpass first tries the controlling terminal, even when
        // standard input is a pipe. Only an unavailable terminal permits the
        // explicit warned plain-line fallback.
        fallback_with(
            prompt,
            || {
                check_cancelled(cancel)?;
                let terminal = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
                if !terminal.is_terminal() {
                    return Err(io::ErrorKind::Unsupported.into());
                }
                Ok(terminal)
            },
            |terminal, prompt| {
                // Keep one controlling-terminal descriptor for the entire
                // operation. Our guard restores before cancellation returns;
                // no helper may raise SIGINT before that guard is dropped.
                let mut output = &terminal;
                let mut io = PosixIo {
                    input: &terminal,
                    output: &mut output,
                    original: None,
                    cancel,
                };
                run_terminal(&mut io, prompt, Feedback::Hidden).map_err(|failure| failure.error)
            },
            |prompt| {
                check_cancelled(cancel)?;
                match cancel {
                    None => {
                        read_plain_line(&mut io::stdin().lock(), &mut io::stderr().lock(), prompt)
                    }
                    Some(cancel) => {
                        let stdin = io::stdin().lock();
                        let mut stderr = io::stderr().lock();
                        stderr.write_all(b"Warning: Password input may be echoed.\n")?;
                        stderr.write_all(prompt.as_bytes())?;
                        stderr.flush()?;
                        let bytes =
                            read_line_bytes(cancel, |byte| read_byte(&stdin, Some(cancel), byte))?;
                        cancel.check_io()?;
                        finish_plain_line(bytes)
                    }
                }
            },
        )
    }
}

#[cfg(unix)]
pub(super) fn read(prompt: &str) -> io::Result<SecretString> {
    posix::read(prompt, None)
}

#[cfg(unix)]
pub(super) fn read_cancellable(prompt: &str, cancel: &TerminalCancel) -> io::Result<SecretString> {
    posix::read(prompt, Some(cancel))
}

#[cfg(unix)]
pub(super) fn read_line_cancellable(prompt: &str, cancel: &TerminalCancel) -> io::Result<String> {
    posix::read_line(prompt, cancel)
}

#[cfg(not(unix))]
pub(super) fn read(prompt: &str) -> io::Result<SecretString> {
    // Non-Unix targets remain outside the supported platform set in PLAN
    // section 9.8. Preserve the existing hidden-entry behavior there.
    if std::io::IsTerminal::is_terminal(&io::stdin()) {
        rpassword::prompt_password(prompt).map(SecretString::new)
    } else {
        read_plain_line(&mut io::stdin().lock(), &mut io::stderr().lock(), prompt)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        io::{BufReader, Cursor},
    };

    #[derive(Default)]
    struct FakeIo {
        input: VecDeque<u8>,
        output: String,
        ready: VecDeque<bool>,
        operations: Vec<&'static str>,
        begin_error: bool,
        read_error: Option<io::ErrorKind>,
        restore_error: bool,
        fail_star: bool,
        cancel: TerminalCancel,
        cancel_after_reads: Option<usize>,
        cancel_on_ready: bool,
        reads: usize,
    }
    impl FakeIo {
        fn with_input(bytes: &[u8]) -> Self {
            Self {
                input: bytes.iter().copied().collect(),
                ..Self::default()
            }
        }
    }
    impl MaskedIo for FakeIo {
        fn check_cancelled(&self) -> io::Result<()> {
            self.cancel.check_io()
        }
        fn cancel(&self) {
            assert!(
                self.operations.contains(&"restore"),
                "physical abort published before restoration"
            );
            self.cancel.cancel();
        }
        fn begin(&mut self) -> io::Result<()> {
            self.operations.push("begin");
            if self.begin_error {
                Err(io::ErrorKind::Unsupported.into())
            } else {
                Ok(())
            }
        }
        fn restore(&mut self) -> io::Result<()> {
            self.operations.push("restore");
            if self.restore_error {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        }
        fn read_byte(&mut self, byte: &mut [u8; 1]) -> io::Result<usize> {
            self.operations.push("read");
            self.reads += 1;
            if self.cancel_after_reads == Some(self.reads) {
                self.cancel.cancel();
            }
            if let Some(error) = self.read_error.take() {
                return Err(error.into());
            }
            match self.input.pop_front() {
                Some(value) => {
                    byte[0] = value;
                    Ok(1)
                }
                None => Ok(0),
            }
        }
        fn escape_byte_ready(&mut self) -> io::Result<bool> {
            self.operations.push("ready");
            if self.cancel_on_ready {
                self.cancel.cancel();
            }
            Ok(self.ready.pop_front().unwrap_or(!self.input.is_empty()))
        }
        fn echo(&mut self, text: &str) -> io::Result<()> {
            self.operations
                .push(if text == "\n" { "newline" } else { "echo" });
            if text == "*" && self.fail_star {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            self.output.push_str(text);
            Ok(())
        }
    }

    fn success(input: &mut FakeIo) -> SecretString {
        match run_masked(input, "Password: ") {
            Ok(secret) => secret,
            Err(error) => panic!("masked prompt failed: {}", error.error),
        }
    }

    fn failure(input: &mut FakeIo) -> MaskedFailure {
        match run_masked(input, "Password: ") {
            Ok(_) => panic!("expected a prompt failure"),
            Err(error) => error,
        }
    }

    #[test]
    fn unicode_entry_echoes_one_star_per_decoded_character_and_no_plaintext() {
        let mut terminal = FakeIo::with_input("é汉🦀\n".as_bytes());
        let secret = success(&mut terminal);
        assert_eq!(secret.expose_secret(), "é汉🦀");
        assert_eq!(terminal.output, "Password: ***\n");
        assert!(terminal.operations.ends_with(&["restore", "newline"]));
    }

    #[test]
    fn backspace_ctrl_u_and_ignored_controls_match_the_python_editor() {
        let mut terminal = FakeIo::with_input(b"\x7f\x08ab\x7fc\x08d\x15xy\x17\t\x04\r");
        let secret = success(&mut terminal);
        assert_eq!(secret.expose_secret(), "xy");
        assert_eq!(
            terminal.output,
            format!("Password: **{ERASE}*{ERASE}*{ERASE}{ERASE}**\n")
        );
    }

    #[test]
    fn editing_does_not_reset_an_incomplete_incremental_decode() {
        let mut terminal = FakeIo::with_input(b"a\xe2\x15\x08\x82\xac\n");
        let secret = success(&mut terminal);
        assert_eq!(secret.expose_secret(), "€");
        assert_eq!(terminal.output, format!("Password: *{ERASE}*\n"));
    }

    #[test]
    fn malformed_utf8_preserves_python_decoder_chunks_for_echo_and_backspace() {
        let mut terminal = FakeIo::with_input(b"a\xe2X\x7fY\xff\n");
        let secret = success(&mut terminal);
        assert_eq!(secret.expose_secret(), "aY\u{fffd}");
        assert_eq!(terminal.output, format!("Password: **{ERASE}**\n"));

        let mut truncated = FakeIo::with_input(b"a\xe2\n");
        assert_eq!(success(&mut truncated).expose_secret(), "a");
        assert_eq!(truncated.output, "Password: *\n");
    }

    #[test]
    fn ansi_navigation_leaves_the_following_enter_for_the_main_editor() {
        let mut terminal = FakeIo::with_input(b"a\x1b[A\x1b[1;5D\x1bOH\x1b[[A\nnext");
        assert_eq!(success(&mut terminal).expose_secret(), "a");
        assert_eq!(terminal.output, "Password: *\n");
        assert_eq!(terminal.input.into_iter().collect::<Vec<_>>(), b"next");
    }

    #[test]
    fn lone_escape_timeout_preserves_later_input_and_alt_prefix_consumes_one_byte() {
        let mut terminal = FakeIo::with_input(b"a\x1bb\x1bx\n");
        terminal.ready = [false, true].into_iter().collect();
        assert_eq!(success(&mut terminal).expose_secret(), "ab");
        assert_eq!(terminal.output, "Password: **\n");
    }

    #[test]
    fn eof_and_ctrl_d_have_the_python_empty_entry_distinction() {
        let mut terminal = FakeIo::with_input(b"");
        assert!(success(&mut terminal).expose_secret().is_empty());

        let mut terminal = FakeIo::with_input(b"\x04unused");
        let error = failure(&mut terminal);
        assert_eq!(error.error.kind(), io::ErrorKind::UnexpectedEof);
        assert!(!error.may_fallback);
        assert_eq!(terminal.output, "Password: \n");

        let mut terminal = FakeIo::with_input(b"a\x04b\n");
        assert_eq!(success(&mut terminal).expose_secret(), "ab");
    }

    #[test]
    fn cancellation_restores_before_returning_and_never_starts_a_fallback() {
        for read_error in [None, Some(io::ErrorKind::Interrupted)] {
            let mut terminal = FakeIo::with_input(b"a\x03unused");
            terminal.read_error = read_error;
            let error = failure(&mut terminal);
            assert!(terminal.cancel.is_cancelled());
            assert_eq!(error.error.kind(), io::ErrorKind::Interrupted);
            assert!(!error.may_fallback);
            assert!(terminal.operations.ends_with(&["restore", "newline"]));
            assert!(matches!(
                terminal.output.as_str(),
                "Password: \n" | "Password: *\n"
            ));
        }
        // The same handle can be set while a read returns a private byte,
        // including while the UTF-8 decoder retains an incomplete character.
        for (input, after, stars) in [
            (b"secret-unused\n".as_slice(), 4, 3),
            (b"a\xe2\x82unused\n".as_slice(), 3, 1),
        ] {
            for feedback in [Feedback::Stars, Feedback::Hidden] {
                let mut terminal = FakeIo {
                    cancel_after_reads: Some(after),
                    ..FakeIo::with_input(input)
                };
                let error = run_terminal(&mut terminal, "Password: ", feedback)
                    .err()
                    .unwrap();
                assert_eq!(error.error.kind(), io::ErrorKind::Interrupted);
                assert!(!error.may_fallback);
                assert!(terminal.cancel.is_cancelled());
                assert_eq!(terminal.reads, after);
                assert_eq!(terminal.input.len(), input.len() - after);
                assert!(terminal.operations.ends_with(&["restore", "newline"]));
                assert_eq!(
                    terminal.output,
                    format!(
                        "Password: {}\n",
                        match feedback {
                            Feedback::Stars => "*".repeat(stars),
                            Feedback::Hidden => String::new(),
                        }
                    )
                );
                assert!(!terminal.output.contains("secret"));
            }
        }
        let mut terminal = FakeIo::with_input(b"unread\n");
        terminal.cancel.cancel();
        assert!(!failure(&mut terminal).may_fallback);
        assert!(terminal.operations.is_empty());
    }

    #[test]
    fn cancellation_during_escape_parsing_restores_in_masked_and_hidden_modes() {
        #[cfg(unix)]
        {
            use nix::{errno::Errno, poll::PollFlags};
            use std::time::Duration;

            // EINTR must not renew the ESC/cancellation polling window.
            let mut remaining = [50, 20, 1, 0].into_iter();
            let mut timeouts = Vec::new();
            assert!(
                !posix::wait_readable(
                    None,
                    || Duration::from_millis(remaining.next().unwrap()),
                    |timeout| {
                        timeouts.push(timeout);
                        Err(Errno::EINTR)
                    },
                )
                .unwrap()
            );
            assert_eq!(timeouts, [50, 20, 1]);
            for events in [
                PollFlags::POLLIN,
                PollFlags::POLLHUP,
                PollFlags::POLLIN | PollFlags::POLLHUP,
            ] {
                assert!(
                    posix::wait_readable(
                        None,
                        || Duration::from_micros(1),
                        |timeout| {
                            assert_eq!(timeout, 1);
                            Ok(Some(events))
                        },
                    )
                    .unwrap()
                );
            }
            for (events, expected) in [
                (PollFlags::POLLERR, Errno::EIO),
                (PollFlags::POLLNVAL, Errno::EBADF),
                (PollFlags::POLLNVAL | PollFlags::POLLIN, Errno::EBADF),
            ] {
                let error =
                    posix::wait_readable(None, || Duration::from_millis(50), |_| Ok(Some(events)))
                        .err()
                        .unwrap();
                assert_eq!(error.raw_os_error(), Some(expected as i32));
            }
            for interrupted in [false, true] {
                let cancel = TerminalCancel::default();
                let coordinator = cancel.clone();
                let error = posix::wait_readable(
                    Some(&cancel),
                    || Duration::from_millis(50),
                    |_| {
                        coordinator.cancel();
                        if interrupted {
                            Err(Errno::EINTR)
                        } else {
                            Ok(Some(PollFlags::POLLIN))
                        }
                    },
                )
                .err()
                .unwrap();
                assert_eq!(error.kind(), io::ErrorKind::Interrupted);
                assert_eq!(
                    posix::wait_readable(
                        Some(&cancel),
                        || panic!("cancellation must precede another wait"),
                        |_| panic!("cancellation must precede another poll"),
                    )
                    .err()
                    .unwrap()
                    .kind(),
                    io::ErrorKind::Interrupted
                );
            }
            // Exercise the actual poll boundary with buffered data followed by
            // hangup. A closed peer is readable until read observes EOF.
            let (input, output) = nix::unistd::pipe().unwrap();
            nix::unistd::write(&output, b"x").unwrap();
            drop(output);
            assert!(posix::readable(&input, None).unwrap());
            let mut byte = [0; 1];
            assert_eq!(nix::unistd::read(&input, &mut byte).unwrap(), 1);
            assert_eq!(byte, *b"x");
            assert!(posix::readable(&input, None).unwrap());
            assert_eq!(nix::unistd::read(&input, &mut byte).unwrap(), 0);
        }
        for input in [
            b"a\x1b\x03unused\n".as_slice(),
            b"a\x1b[\x03unused\n",
            b"a\x1b[1;\x03unused\n",
            b"a\x1bO\x03unused\n",
        ] {
            for feedback in [Feedback::Stars, Feedback::Hidden] {
                let mut terminal = FakeIo::with_input(input);
                let error = run_terminal(&mut terminal, "Password: ", feedback)
                    .err()
                    .unwrap();
                assert_eq!(error.error.kind(), io::ErrorKind::Interrupted);
                assert!(!error.may_fallback);
                assert!(terminal.operations.ends_with(&["restore", "newline"]));
                assert_eq!(
                    terminal.output,
                    match feedback {
                        Feedback::Stars => "Password: *\n",
                        Feedback::Hidden => "Password: \n",
                    }
                );
                assert_eq!(terminal.input.into_iter().collect::<Vec<_>>(), b"unused\n");
            }
        }
        for feedback in [Feedback::Stars, Feedback::Hidden] {
            let mut terminal = FakeIo {
                cancel_on_ready: true,
                ..FakeIo::with_input(b"secret\x1b[1;5Dunused\n")
            };
            let error = run_terminal(&mut terminal, "Password: ", feedback)
                .err()
                .unwrap();
            assert_eq!(error.error.kind(), io::ErrorKind::Interrupted);
            assert!(!error.may_fallback);
            assert!(terminal.cancel.is_cancelled());
            assert!(
                terminal
                    .operations
                    .ends_with(&["ready", "restore", "newline"])
            );
            assert_eq!(
                terminal.output,
                match feedback {
                    Feedback::Stars => "Password: ******\n",
                    Feedback::Hidden => "Password: \n",
                }
            );
            assert_eq!(
                terminal.input.into_iter().collect::<Vec<_>>(),
                b"[1;5Dunused\n"
            );
        }
    }

    #[test]
    fn controlling_terminal_fallback_is_hidden_and_uses_owned_editing_and_restoration() {
        let secret = fallback_with(
            "Password: ",
            || Ok(FakeIo::with_input("clear\u{15}é汉\u{7f}x\nnext".as_bytes())),
            |mut terminal, prompt| {
                let result = run_terminal(&mut terminal, prompt, Feedback::Hidden)
                    .map_err(|failure| failure.error);
                assert_eq!(terminal.output, "Password: \n");
                assert!(terminal.operations.ends_with(&["restore", "newline"]));
                assert_eq!(terminal.input.into_iter().collect::<Vec<_>>(), b"next");
                result
            },
            |_| panic!("an available controlling terminal must not read plain stdin"),
        )
        .unwrap();
        assert_eq!(secret.expose_secret(), "éx");
    }

    #[test]
    fn controlling_terminal_fallback_cancellation_restores_and_never_retries_plain_input() {
        for external_cancel in [false, true] {
            let error = fallback_with(
                "Password: ",
                || {
                    Ok(FakeIo {
                        cancel_on_ready: external_cancel,
                        ..FakeIo::with_input(if external_cancel {
                            b"secret\x1b[Aunused\n"
                        } else {
                            b"secret\x03unused\n"
                        })
                    })
                },
                |mut terminal, prompt| {
                    let result = run_terminal(&mut terminal, prompt, Feedback::Hidden)
                        .map_err(|failure| failure.error);
                    assert_eq!(terminal.output, "Password: \n");
                    assert!(terminal.operations.ends_with(&["restore", "newline"]));
                    assert_eq!(
                        terminal.input.into_iter().collect::<Vec<_>>(),
                        if external_cancel {
                            b"[Aunused\n".as_slice()
                        } else {
                            b"unused\n"
                        }
                    );
                    result
                },
                |_| panic!("cancellation must not start a visible-input retry"),
            )
            .err()
            .unwrap();
            assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        }
    }

    #[test]
    fn controlling_terminal_failures_attempt_restoration_without_plain_input_retry() {
        for terminal in [
            FakeIo {
                begin_error: true,
                ..FakeIo::with_input(b"secret\n")
            },
            FakeIo {
                read_error: Some(io::ErrorKind::Other),
                ..FakeIo::with_input(b"secret\n")
            },
            FakeIo {
                restore_error: true,
                ..FakeIo::with_input(b"secret\n")
            },
        ] {
            let result = fallback_with(
                "Password: ",
                || Ok(terminal),
                |mut terminal, prompt| {
                    let result = run_terminal(&mut terminal, prompt, Feedback::Hidden)
                        .map_err(|failure| failure.error);
                    assert_eq!(terminal.output, "Password: \n");
                    assert!(terminal.operations.ends_with(&["restore", "newline"]));
                    result
                },
                |_| panic!("terminal failures must not start a visible-input retry"),
            );
            assert!(result.is_err());
        }
    }

    #[test]
    fn unavailable_controlling_terminal_selects_the_warned_plain_line_fallback() {
        let mut input = Cursor::new(b"  secret  \nnext\n");
        let mut output = Vec::new();
        let secret = fallback_with(
            "Password: ",
            || Err::<FakeIo, _>(io::ErrorKind::NotFound.into()),
            |_, _| panic!("an unavailable controlling terminal cannot be used"),
            |prompt| read_plain_line(&mut input, &mut output, prompt),
        )
        .unwrap();
        assert_eq!(secret.expose_secret(), "  secret  ");
        assert_eq!(input.position(), b"  secret  \n".len() as u64);
        assert_eq!(
            output,
            b"Warning: Password input may be echoed.\nPassword: "
        );
    }

    #[test]
    fn setup_read_and_output_failures_attempt_restoration_and_discard_the_entry() {
        let mut cases = [
            FakeIo {
                begin_error: true,
                ..FakeIo::with_input(b"unconsumed\n")
            },
            FakeIo {
                read_error: Some(io::ErrorKind::Other),
                ..FakeIo::with_input(b"unconsumed\n")
            },
            FakeIo {
                fail_star: true,
                ..FakeIo::with_input(b"secret\n")
            },
        ];
        for terminal in &mut cases {
            assert!(failure(terminal).may_fallback);
            assert!(terminal.operations.ends_with(&["restore", "newline"]));
            assert!(!terminal.output.contains("secret"));
        }
        assert_eq!(cases[0].input.len(), b"unconsumed\n".len());
    }

    #[test]
    fn restore_failure_forbids_retrying_with_the_modified_terminal_mode() {
        let mut terminal = FakeIo {
            restore_error: true,
            ..FakeIo::with_input(b"secret\n")
        };
        let error = failure(&mut terminal);
        assert_eq!(error.error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!error.may_fallback);
        assert_eq!(terminal.output, "Password: ******\n");
    }

    #[test]
    fn non_tty_fallback_warns_and_reads_one_untrimmed_plain_line_without_stars() {
        let mut input = Cursor::new(b"  fixture password  \r\nsecond\n");
        let mut output = Vec::new();
        let secret = read_plain_line(&mut input, &mut output, "Password: ").unwrap();
        assert_eq!(secret.expose_secret(), "  fixture password  ");
        assert_eq!(input.position(), b"  fixture password  \r\n".len() as u64);
        assert_eq!(
            output,
            b"Warning: Password input may be echoed.\nPassword: "
        );

        let mut empty = Cursor::new(b"");
        assert_eq!(
            read_plain_line(&mut empty, &mut Vec::new(), "Password: ")
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn fallback_handles_fragmented_utf8_growth_and_preserves_plain_control_bytes() {
        // Cancellable ordinary input preserves read_line's newline and partial
        // EOF contract without consuming the following line.
        for (raw, expected) in [("é🦀\r\nunused\n", "é🦀\r\n"), ("partial", "partial")] {
            let cancel = TerminalCancel::default();
            let mut input = Cursor::new(raw.as_bytes());
            let bytes =
                read_line_bytes(&cancel, |byte| std::io::Read::read(&mut input, byte)).unwrap();
            assert_eq!(line_from_bytes(bytes).unwrap(), expected);
            assert_eq!(input.position(), expected.len() as u64);
        }
        let cancel = TerminalCancel::default();
        let coordinator = cancel.clone();
        let mut input = Cursor::new(b"private-line\nunused");
        let error = read_line_bytes(&cancel, |byte| {
            let count = std::io::Read::read(&mut input, byte)?;
            if input.position() == 3 {
                coordinator.cancel();
            }
            Ok(count)
        })
        .err()
        .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(input.position(), 3);
        assert_eq!(
            read_line_bytes(&cancel, |_| panic!("a cancelled reader must not retry"))
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::Interrupted
        );

        let line = format!(" {} \t\x08\x15", "é🦀".repeat(80));
        let mut input = BufReader::with_capacity(3, Cursor::new(line.as_bytes()));
        let mut output = Vec::new();
        let secret = read_plain_line(&mut input, &mut output, "Password: ").unwrap();
        assert_eq!(secret.expose_secret(), &line);
        assert_eq!(
            output,
            b"Warning: Password input may be echoed.\nPassword: "
        );

        let mut input = Cursor::new(b"\xff\n");
        assert_eq!(
            read_plain_line(&mut input, &mut Vec::new(), "Password: ")
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn fallback_output_failure_does_not_consume_secret_input() {
        struct BrokenOutput;
        impl Write for BrokenOutput {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut input = Cursor::new(b"fixture-secret\n");
        assert_eq!(
            read_plain_line(&mut input, &mut BrokenOutput, "Password: ")
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(input.position(), 0);
    }
}
