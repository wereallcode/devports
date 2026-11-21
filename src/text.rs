use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn escape_controls(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                character.escape_default().to_string()
            } else {
                character.to_string()
            }
        })
        .collect()
}

pub(crate) fn prefix(text: &str, width: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|character| {
            let next = character.width().unwrap_or(0);
            if used + next > width {
                false
            } else {
                used += next;
                true
            }
        })
        .collect()
}

pub(crate) fn clipped(text: &str, width: usize) -> String {
    if width == 0 {
        String::new()
    } else if text.width() <= width {
        text.to_owned()
    } else {
        format!("{}…", prefix(text, width - 1))
    }
}
