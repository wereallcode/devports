mod input;
mod output;

pub use input::Key;
use input::{PasteBuffer, parse_sequence};
use output::{bounded_frame, visible_width};

use std::{
    env,
    io::{self, IsTerminal, Write},
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

// Drawn inline, not on the alternate screen, so the list stays in the scrollback.
const ENTER_SCREEN: &[u8] = b"\x1b[?25l\x1b[?2004h";
const LEAVE_SCREEN: &[u8] = b"\x1b[0m\x1b[?2004l\x1b[?25h\r\n";
const SEQUENCE_WAIT: Duration = Duration::from_millis(40);
const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SIGNALLED: AtomicBool = AtomicBool::new(false);

pub struct Terminal {
    original: libc::termios,
    previous_handlers: Vec<(libc::c_int, libc::sigaction)>,
    raw: bool,
    screen: bool,
    paste: Option<PasteBuffer>,
    drawn: Vec<usize>,
}

extern "C" fn quit_signal(_signal: libc::c_int) {
    // Signal handlers may only touch lock-free atomics: no I/O or allocation.
    SIGNALLED.store(true, Ordering::Relaxed);
}

impl Terminal {
    pub fn enter() -> Result<Self, String> {
        if !io::stdin().is_terminal()
            || !io::stdout().is_terminal()
            || env::var_os("TERM").is_some_and(|term| term == "dumb")
        {
            return Err("devports needs an interactive terminal with keyboard input.".into());
        }
        if ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("The terminal is already in use.".into());
        }
        let mut original = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initializes this termios when it returns zero.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) } != 0 {
            ACTIVE.store(false, Ordering::Release);
            return Err(format!(
                "Cannot read terminal settings: {}",
                io::Error::last_os_error()
            ));
        }
        // SAFETY: successful tcgetattr initialized the value above.
        let original = unsafe { original.assume_init() };
        let mut terminal = Self {
            original,
            previous_handlers: Vec::with_capacity(SIGNALS.len()),
            raw: false,
            screen: false,
            paste: None,
            drawn: Vec::new(),
        };
        let _blocked = BlockedSignals::new()?;
        SIGNALLED.store(false, Ordering::Relaxed);
        for signal in SIGNALS {
            // SAFETY: a zeroed sigaction is initialized below before use.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = quit_signal as *const () as libc::sighandler_t;
            action.sa_flags = 0; // Interrupt poll/read so the quit flag is observed.
            // SAFETY: sa_mask points to valid writable sigset_t storage.
            unsafe { libc::sigemptyset(&mut action.sa_mask) };
            let mut previous = MaybeUninit::<libc::sigaction>::uninit();
            // SAFETY: both action pointers remain valid for this synchronous call.
            if unsafe { libc::sigaction(signal, &action, previous.as_mut_ptr()) } != 0 {
                return Err(format!(
                    "Cannot install terminal signal handler: {}",
                    io::Error::last_os_error()
                ));
            }
            // SAFETY: successful sigaction initialized the previous action.
            terminal
                .previous_handlers
                .push((signal, unsafe { previous.assume_init() }));
        }
        let mut raw = terminal.original;
        // SAFETY: cfmakeraw only updates the valid termios value.
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: raw contains a valid termios derived from tcgetattr.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(format!(
                "Cannot enter terminal raw mode: {}",
                io::Error::last_os_error()
            ));
        }
        terminal.raw = true;
        // Set before writing so a partial write still restores the screen.
        terminal.screen = true;
        write_stdout(ENTER_SCREEN)?;
        Ok(terminal)
    }

    pub fn size(&self) -> (usize, usize) {
        // SAFETY: zero is a valid initial winsize value.
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: ioctl receives valid writable winsize storage.
        if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
            && size.ws_col > 0
            && size.ws_row > 0
        {
            (usize::from(size.ws_col), usize::from(size.ws_row))
        } else {
            (80, 24)
        }
    }

    /// Lines the terminal wrapped after a resize count as several rows.
    pub fn draw(&mut self, frame: &str) -> Result<(), String> {
        let (width, height) = self.size();
        let content = bounded_frame(frame, width, height.saturating_sub(1).max(1));
        let rows = self
            .drawn
            .iter()
            .map(|line| line.div_ceil(width).max(1))
            .sum::<usize>();
        let mut bytes = String::from("\r");
        if rows > 1 {
            bytes.push_str(&format!("\x1b[{}A", rows - 1));
        }
        bytes.push_str("\x1b[J");
        bytes.push_str(&content);
        bytes.push_str("\x1b[0m");
        self.drawn = content.split("\r\n").map(visible_width).collect();
        let mut output = io::stdout().lock();
        output
            .write_all(bytes.as_bytes())
            .and_then(|()| output.flush())
            .map_err(|error| format!("Cannot draw terminal: {error}"))
    }

    pub fn read_key(&mut self, timeout: Duration) -> Result<Option<Key>, String> {
        let started = Instant::now();
        let mut first = true;
        loop {
            if SIGNALLED.load(Ordering::Relaxed) {
                return Ok(Some(Key::Quit));
            }
            if !first && started.elapsed() >= timeout {
                return Ok(None);
            }
            first = false;
            let byte = match read_byte(timeout.saturating_sub(started.elapsed()))? {
                Input::Byte(byte) => byte,
                Input::Timeout => return Ok(None),
                Input::Quit => return Ok(Some(Key::Quit)),
            };
            if let Some(paste) = &mut self.paste {
                if paste.feed(byte) {
                    let paste = self.paste.take().expect("paste is active");
                    return Ok(Some(Key::Paste(paste.into_text())));
                }
                continue;
            }
            let key = match byte {
                b'\r' | b'\n' => Some(Key::Enter),
                0x03 | 0x04 => Some(Key::Quit),
                0x08 | 0x7f => Some(Key::Backspace),
                0x1b => self.read_escape()?,
                0x20..=0x7e => Some(Key::Char(char::from(byte))),
                0xc2..=0xf4 => read_utf8(byte)?,
                _ => None,
            };
            if key.is_some() {
                return Ok(key);
            }
        }
    }

    fn read_escape(&mut self) -> Result<Option<Key>, String> {
        let started = Instant::now();
        let next = match read_byte(SEQUENCE_WAIT)? {
            Input::Byte(byte) => byte,
            Input::Timeout => return Ok(Some(Key::Escape)),
            Input::Quit => return Ok(Some(Key::Quit)),
        };
        if next != b'[' && next != b'O' {
            // Alt+character is one unsupported key, not a shortcut for its trailing character.
            return Ok(None);
        }
        let mut sequence = Vec::with_capacity(16);
        while sequence.len() < 64 {
            let byte = match read_byte(SEQUENCE_WAIT.saturating_sub(started.elapsed()))? {
                Input::Byte(byte) => byte,
                Input::Timeout => return Ok(None),
                Input::Quit => return Ok(Some(Key::Quit)),
            };
            sequence.push(byte);
            // Linux virtual-console F1-F5 sequences are ESC [ [ A through E.
            if sequence.len() == 1 && byte == b'[' {
                continue;
            }
            if (0x40..=0x7e).contains(&byte) {
                if next == b'[' && sequence == b"200~" {
                    self.paste = Some(PasteBuffer::default());
                    return Ok(None);
                }
                return Ok(parse_sequence(&sequence));
            }
            if !(0x20..=0x3f).contains(&byte) {
                return Ok(None);
            }
        }
        Ok(None)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _blocked = BlockedSignals::new().ok();
        if self.raw {
            // SAFETY: original came from tcgetattr on stdin; TCSANOW skips waiting on a stuck reader.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original);
                // Discard pending shortcuts separately, without draining output.
                libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
            }
        }
        if self.screen {
            restore_screen();
        }
        for (signal, previous) in self.previous_handlers.iter().rev() {
            // SAFETY: previous is the saved sigaction for this signal.
            unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
        }
        ACTIVE.store(false, Ordering::Release);
    }
}

fn write_stdout(bytes: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("Cannot write terminal: {error}"))
}

fn restore_screen() {
    let mut bytes = LEAVE_SCREEN;
    while !bytes.is_empty() {
        // SAFETY: bytes points to readable static storage for the supplied length.
        let written =
            unsafe { libc::write(libc::STDOUT_FILENO, bytes.as_ptr().cast(), bytes.len()) };
        if written > 0 {
            bytes = &bytes[written as usize..];
        } else if written < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        } else {
            break;
        }
    }
}

struct BlockedSignals(libc::sigset_t);

impl BlockedSignals {
    fn new() -> Result<Self, String> {
        // SAFETY: the set is initialized with sigemptyset before it is read.
        let mut blocked: libc::sigset_t = unsafe { std::mem::zeroed() };
        // SAFETY: each function receives valid writable sigset_t storage.
        unsafe {
            libc::sigemptyset(&mut blocked);
            for signal in SIGNALS {
                libc::sigaddset(&mut blocked, signal);
            }
        }
        let mut previous = MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: both pointers are valid and sigmask initializes previous on success.
        let error =
            unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, previous.as_mut_ptr()) };
        if error != 0 {
            return Err(format!(
                "Cannot protect terminal setup: {}",
                io::Error::from_raw_os_error(error)
            ));
        }
        // SAFETY: successful pthread_sigmask initialized previous.
        Ok(Self(unsafe { previous.assume_init() }))
    }
}

impl Drop for BlockedSignals {
    fn drop(&mut self) {
        // SAFETY: the mask was initialized by pthread_sigmask on this thread.
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut()) };
    }
}

enum Input {
    Byte(u8),
    Timeout,
    Quit,
}

fn read_byte(timeout: Duration) -> Result<Input, String> {
    let started = Instant::now();
    loop {
        if SIGNALLED.load(Ordering::Relaxed) {
            return Ok(Input::Quit);
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        let milliseconds = remaining.as_nanos().div_ceil(1_000_000);
        let milliseconds = milliseconds.min(i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one valid pollfd and a bounded timeout.
        let ready = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
        if ready == 0 {
            if started.elapsed() >= timeout {
                return Ok(Input::Timeout);
            }
            continue;
        }
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("Cannot wait for keyboard input: {error}"));
        }
        if descriptor.revents & libc::POLLIN != 0 {
            let mut byte = 0u8;
            // SAFETY: byte is writable for one byte and stdin is ready for reading.
            let count = unsafe { libc::read(libc::STDIN_FILENO, (&mut byte as *mut u8).cast(), 1) };
            if count == 1 {
                return Ok(Input::Byte(byte));
            }
            if count == 0 {
                return Ok(Input::Quit);
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("Cannot read keyboard input: {error}"));
        }
        if descriptor.revents & libc::POLLHUP != 0 {
            return Ok(Input::Quit);
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err("The terminal input connection was closed.".into());
        }
        if started.elapsed() >= timeout {
            return Ok(Input::Timeout);
        }
    }
}

fn read_utf8(first: u8) -> Result<Option<Key>, String> {
    let length = match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Ok(None),
    };
    let mut bytes = [0u8; 4];
    bytes[0] = first;
    let started = Instant::now();
    for byte in bytes.iter_mut().take(length).skip(1) {
        match read_byte(SEQUENCE_WAIT.saturating_sub(started.elapsed()))? {
            Input::Byte(next) if next & 0xc0 == 0x80 => *byte = next,
            Input::Quit => return Ok(Some(Key::Quit)),
            _ => return Ok(None),
        }
    }
    Ok(std::str::from_utf8(&bytes[..length])
        .ok()
        .and_then(|text| text.chars().next())
        .filter(|character| !character.is_control())
        .map(Key::Char))
}

#[cfg(test)]
mod tests {
    use super::*;
    use output::bounded_line;

    #[test]
    fn decodes_navigation_and_modified_arrows() {
        for (bytes, expected) in [
            (b"A".as_slice(), Key::Up),
            (b"1;5B", Key::Down),
            (b"C", Key::Right),
            (b"D", Key::Left),
            (b"H", Key::Home),
            (b"F", Key::End),
            (b"3~", Key::Delete),
            (b"5~", Key::PageUp),
            (b"6~", Key::PageDown),
            (b"7~", Key::Home),
            (b"8~", Key::End),
        ] {
            assert_eq!(parse_sequence(bytes), Some(expected));
        }
    }

    #[test]
    fn ignores_unknown_escape_and_paste_markers_as_keys() {
        for bytes in [
            b"".as_slice(),
            b"200~",
            b"201~",
            b"11~",
            b"[A",
            b"<0;4;9M",
            b"?1A",
        ] {
            assert_eq!(parse_sequence(bytes), None);
        }
    }

    #[test]
    fn paste_collects_shortcuts_as_data_and_handles_partial_markers() {
        let mut paste = PasteBuffer::default();
        for byte in b"q\r\x03r\x1b[A\x1b[20q\x1b" {
            assert!(!paste.feed(*byte));
        }
        for byte in b"\x1b[201" {
            assert!(!paste.feed(*byte));
        }
        assert!(paste.feed(b'~'));
        assert_eq!(paste.into_text(), "q\r\x03r\x1b[A\x1b[20q\x1b");
    }

    #[test]
    fn paste_is_bounded_but_still_consumes_its_closing_marker() {
        let mut paste = PasteBuffer::default();
        for _ in 0..10000 {
            assert!(!paste.feed(b'a'));
        }
        for byte in b"\x1b[201" {
            assert!(!paste.feed(*byte));
        }
        assert!(paste.feed(b'~'));
        assert_eq!(paste.into_text().len(), 8192);
        assert!(
            PasteBuffer {
                matched: 0,
                bytes: vec![0xff; 8192]
            }
            .into_text()
            .len()
                <= 8192
        );
    }

    #[test]
    fn frames_are_bounded_by_unicode_cells_and_terminal_rows() {
        assert_eq!(
            bounded_frame("hello world\n界界界\nthird", 6, 2),
            "hello\r\n界界"
        );
        assert_eq!(bounded_frame("a\r\nb", 10, 2), "a\r\nb");
        assert_eq!(bounded_frame("abc", 1, 2), "");
        assert_eq!(bounded_line("abc界x", 4), "abc");
    }

    #[test]
    fn allows_sgr_but_strips_cursor_and_osc_sequences() {
        assert_eq!(
            bounded_line("\x1b[1mhello\x1b[0m\x1b[2J!", 10),
            "\x1b[1mhello\x1b[0m!"
        );
        assert_eq!(
            bounded_line("a\x1b]0;title\x07b\x1b]8;;evil\x1b\\c", 10),
            "abc"
        );
    }
}
