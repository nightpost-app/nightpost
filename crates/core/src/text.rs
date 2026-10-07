//! Received text, made safe to show. Applied once, when a message arrives,
//! so the terminal interface, the graphical one and `nightpost listen` piped
//! into a terminal all get the same clean text.
//!
//! Gone: control characters (ESC is how a terminal is told to do anything,
//! clipboard writes included), the bidirectional overrides that make
//! `innocent<RLO>gnp.exe` display as `innocentexe.png`, and the invisible
//! characters that make two identical-looking texts differ. Line breaks stay,
//! up to 50 lines, so one message of empty lines cannot scroll a
//! conversation away. (As in murmure.)

const MAX_LINES: usize = 50;

fn hidden(c: char) -> bool {
    const BIDI: [char; 12] = [
        '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}',
        '\u{2067}', '\u{2068}', '\u{2069}',
    ];
    const INVISIBLE: [char; 13] = [
        '\u{00ad}', '\u{180e}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{2028}', '\u{2029}', '\u{2060}', '\u{2061}',
        '\u{2062}', '\u{2063}', '\u{2064}', '\u{feff}',
    ];
    c.is_control() || BIDI.contains(&c) || INVISIBLE.contains(&c) || ('\u{e0000}'..='\u{e007f}').contains(&c)
}

/// One line: no line breaks either.
pub fn line(raw: &str) -> String {
    raw.chars().filter(|&c| !hidden(c)).collect()
}

/// A message body: [`line`] for each line, never two blank lines in a row,
/// none at either end, and past 50 lines the rest joined with spaces.
pub fn message(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for l in raw.split('\n').map(line) {
        let blank = l.trim().is_empty();
        if blank && lines.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        lines.push(if blank { String::new() } else { l });
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    if lines.len() > MAX_LINES {
        let rest = lines.split_off(MAX_LINES - 1).join(" ");
        lines.push(rest);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_could_steer_or_spoof_the_screen_is_dropped() {
        // An OSC 52 clipboard write, a bidi override, a zero-width space.
        assert_eq!(message("a\u{1b}]52;c;ZXZpbA==\u{7}b"), "a]52;c;ZXZpbA==b");
        assert_eq!(line("innocent\u{202e}gnp.exe"), "innocentgnp.exe");
        assert_eq!(line("pay\u{200b}pal"), "paypal");
        assert_eq!(message("\n\nun\r\n\n\n\ndeux\n\n"), "un\n\ndeux");
        assert_eq!(message(&"x\n".repeat(80)).lines().count(), 50);
        assert_eq!(message("été 日本 🙂"), "été 日本 🙂");
    }
}
