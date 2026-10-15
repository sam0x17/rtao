// Derived from RaoFoundation/subtensor sdk/python/bittensor/masked_input.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Star-echo secret editor with injected input and output boundaries.

#[cfg(any(unix, test))]
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::io::{self, BufRead, Write};
use zeroize::{Zeroize, Zeroizing};

#[cfg(any(unix, test))]
const ERASE: &str = "\u{8} \u{8}";

#[cfg(any(unix, test))]
trait MaskedIo {
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
    let count = io.read_byte(byte)?;
    // ISIG is disabled, so interruption must apply to every consumed byte,
    // including a byte swallowed while recognizing an escape sequence.
    if count != 0 && byte[0] == b'\x03' {
        return Err(io::ErrorKind::Interrupted.into());
    }
    Ok(count)
}

#[cfg(any(unix, test))]
fn discard_escape(io: &mut impl MaskedIo) -> io::Result<()> {
    if !io.escape_byte_ready()? {
        return Ok(());
    }
    let mut byte = Zeroizing::new([0; 1]);
    if read_key_byte(io, &mut byte)? == 0 || !matches!(byte[0], b'[' | b'O') {
        return Ok(());
    }
    while io.escape_byte_ready()? {
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
    io.echo(prompt).map_err(|error| MaskedFailure {
        error,
        may_fallback: true,
    })?;
    let result = io.begin().and_then(|()| edit(io, feedback));
    let restored = io.restore();
    let newline = io.echo("\n");
    if let Err(error) = restored {
        return Err(MaskedFailure {
            error,
            may_fallback: false,
        });
    }
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
    match String::from_utf8(std::mem::take(&mut *bytes)) {
        Ok(line) => Ok(SecretString::new(line)),
        Err(error) => {
            let _discarded = Zeroizing::new(error.into_bytes());
            Err(io::ErrorKind::InvalidData.into())
        }
    }
}

#[cfg(unix)]
mod posix {
    use super::*;
    use nix::sys::{
        select::{FdSet, select},
        termios::{LocalFlags, SetArg, SpecialCharacterIndices, Termios, tcgetattr, tcsetattr},
        time::{TimeVal, TimeValLike},
    };
    use std::{fs::OpenOptions, io::IsTerminal, os::fd::AsFd};

    struct PosixIo<'a, Fd: AsFd> {
        input: &'a Fd,
        output: &'a mut dyn Write,
        original: Option<Termios>,
    }
    impl<Fd: AsFd> PosixIo<'_, Fd> {
        fn restore_original(&mut self) -> io::Result<()> {
            if let Some(original) = &self.original {
                tcsetattr(self.input, SetArg::TCSADRAIN, original)?;
                self.original = None;
            }
            Ok(())
        }
    }
    impl<Fd: AsFd> MaskedIo for PosixIo<'_, Fd> {
        fn begin(&mut self) -> io::Result<()> {
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
            tcsetattr(self.input, SetArg::TCSADRAIN, &mode)?;
            Ok(())
        }
        fn restore(&mut self) -> io::Result<()> {
            self.restore_original()
        }
        fn read_byte(&mut self, byte: &mut [u8; 1]) -> io::Result<usize> {
            // Bypass StdinLock's read-ahead buffer: select must see every byte
            // remaining after ESC, including an immediately following Enter.
            nix::unistd::read(self.input, byte).map_err(Into::into)
        }
        fn escape_byte_ready(&mut self) -> io::Result<bool> {
            let mut readable = FdSet::new();
            readable.insert(self.input.as_fd());
            let mut timeout = TimeVal::milliseconds(50);
            select(None, &mut readable, None, None, &mut timeout)
                .map(|ready| ready != 0)
                .map_err(Into::into)
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

    pub(super) fn read(prompt: &str) -> io::Result<SecretString> {
        if io::stdin().is_terminal() {
            let stdin = io::stdin().lock();
            let mut stderr = io::stderr().lock();
            let mut terminal = PosixIo {
                input: &stdin,
                output: &mut stderr,
                original: None,
            };
            match run_masked(&mut terminal, prompt) {
                Ok(secret) => return Ok(secret),
                Err(error) if !error.may_fallback => return Err(error.error),
                Err(_) => {}
            }
        }
        fallback(prompt)
    }

    fn fallback(prompt: &str) -> io::Result<SecretString> {
        // Python's getpass first tries the controlling terminal, even when
        // standard input is a pipe. Only an unavailable terminal permits the
        // explicit warned plain-line fallback.
        fallback_with(
            prompt,
            || {
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
                };
                run_terminal(&mut io, prompt, Feedback::Hidden).map_err(|failure| failure.error)
            },
            |prompt| read_plain_line(&mut io::stdin().lock(), &mut io::stderr().lock(), prompt),
        )
    }
}

#[cfg(unix)]
pub(super) fn read(prompt: &str) -> io::Result<SecretString> {
    posix::read(prompt)
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
            assert_eq!(error.error.kind(), io::ErrorKind::Interrupted);
            assert!(!error.may_fallback);
            assert!(terminal.operations.ends_with(&["restore", "newline"]));
            assert!(matches!(
                terminal.output.as_str(),
                "Password: \n" | "Password: *\n"
            ));
        }
    }

    #[test]
    fn cancellation_during_escape_parsing_restores_in_masked_and_hidden_modes() {
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
        let error = fallback_with(
            "Password: ",
            || Ok(FakeIo::with_input(b"secret\x03unused\n")),
            |mut terminal, prompt| {
                let result = run_terminal(&mut terminal, prompt, Feedback::Hidden)
                    .map_err(|failure| failure.error);
                assert_eq!(terminal.output, "Password: \n");
                assert!(terminal.operations.ends_with(&["restore", "newline"]));
                assert_eq!(terminal.input.into_iter().collect::<Vec<_>>(), b"unused\n");
                result
            },
            |_| panic!("cancellation must not start a visible-input retry"),
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
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
