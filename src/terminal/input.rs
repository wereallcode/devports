const MAX_PASTE_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Backspace,
    Delete,
    Char(char),
    Paste(String),
    Quit,
}

pub(super) fn parse_sequence(sequence: &[u8]) -> Option<Key> {
    let (&final_byte, parameters) = sequence.split_last()?;
    if !parameters
        .iter()
        .all(|byte| byte.is_ascii_digit() || *byte == b';')
    {
        return None;
    }
    match final_byte {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        b'H' => Some(Key::Home),
        b'F' => Some(Key::End),
        b'~' => match parameters.split(|byte| *byte == b';').next()? {
            b"1" | b"7" => Some(Key::Home),
            b"4" | b"8" => Some(Key::End),
            b"3" => Some(Key::Delete),
            b"5" => Some(Key::PageUp),
            b"6" => Some(Key::PageDown),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct PasteBuffer {
    pub(super) matched: usize,
    pub(super) bytes: Vec<u8>,
}

impl PasteBuffer {
    pub(super) fn feed(&mut self, byte: u8) -> bool {
        const END: &[u8] = b"\x1b[201~";
        if byte == END[self.matched] {
            self.matched += 1;
        } else {
            for pending in &END[..self.matched] {
                self.append(*pending);
            }
            self.matched = 0;
            if byte == END[0] {
                self.matched = 1;
            } else {
                self.append(byte);
            }
        }
        self.matched == END.len()
    }

    fn append(&mut self, byte: u8) {
        if self.bytes.len() < MAX_PASTE_BYTES {
            self.bytes.push(byte);
        }
    }

    pub(super) fn into_text(self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        if text.len() > MAX_PASTE_BYTES {
            let mut boundary = MAX_PASTE_BYTES;
            while !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            text.truncate(boundary);
        }
        text
    }
}
