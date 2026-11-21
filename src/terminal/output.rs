use unicode_width::UnicodeWidthChar;

pub(super) fn bounded_frame(frame: &str, width: usize, height: usize) -> String {
    let budget = width.saturating_sub(1);
    frame
        .split('\n')
        .take(height)
        .map(|line| bounded_line(line, budget))
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub(super) fn bounded_line(line: &str, budget: usize) -> String {
    let mut result = String::new();
    let mut characters = line.chars().peekable();
    let mut width = 0usize;
    let mut clipped = false;
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            match characters.next() {
                Some('[') => {
                    let mut sequence = String::from("\x1b[");
                    for next in characters.by_ref() {
                        sequence.push(next);
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                    if sequence.ends_with('m')
                        && sequence.len() <= 64
                        && sequence[2..sequence.len() - 1]
                            .chars()
                            .all(|ch| ch.is_ascii_digit() || ch == ';' || ch == ':')
                    {
                        result.push_str(&sequence);
                    }
                }
                Some(']') => {
                    let mut escaped = false;
                    for next in characters.by_ref() {
                        if next == '\x07' || escaped && next == '\\' {
                            break;
                        }
                        escaped = next == '\x1b';
                    }
                }
                _ => {}
            }
            continue;
        }
        if character.is_control() {
            continue;
        }
        let cells = character.width().unwrap_or(0);
        if clipped || width.saturating_add(cells) > budget {
            clipped = true;
            continue;
        }
        width += cells;
        result.push(character);
    }
    result
}

pub(super) fn visible_width(line: &str) -> usize {
    let mut width = 0;
    let mut escape = false;
    for character in line.chars() {
        if character == '\x1b' {
            escape = true;
        } else if escape {
            escape = !character.is_ascii_alphabetic();
        } else {
            width += character.width().unwrap_or(0);
        }
    }
    width
}
