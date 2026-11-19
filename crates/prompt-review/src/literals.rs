//! Template literals of a TypeScript source, enough to read prompt text out of `*.instructions.ts`
//! and `*.references.ts` files: comments and quoted strings are skipped, `${…}` interpolations are
//! replaced by `…`, and each literal keeps the 1-based source line it starts on.

/// Shorter literals are identifiers, paths or format strings, not prompt text.
pub const MIN_LITERAL: usize = 120;

#[derive(PartialEq)]
enum State { Code, Line, Block, Quote(char) }

/// `(start line, text)` of every template literal of MIN_LITERAL characters or more.
pub fn templates(source: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = source.chars().collect();
    let (mut out, mut state, mut line, mut i) = (Vec::new(), State::Code, 1usize, 0usize);
    while i < chars.len() {
        let (c, next) = (chars[i], chars.get(i + 1).copied());
        if c == '\n' {
            line += 1;
        }
        match state {
            State::Line if c == '\n' => state = State::Code,
            State::Block if c == '*' && next == Some('/') => { state = State::Code; i += 1; }
            State::Quote(_) if c == '\\' => i += 1,
            State::Quote(q) if c == q => state = State::Code,
            State::Code if c == '/' && next == Some('/') => state = State::Line,
            State::Code if c == '/' && next == Some('*') => state = State::Block,
            State::Code if c == '\'' || c == '"' => state = State::Quote(c),
            State::Code if c == '`' => {
                let (start, text, end, lines) = template(&chars, i + 1, line);
                if text.chars().count() >= MIN_LITERAL {
                    out.push((start, text));
                }
                line = lines;
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Reads one template literal body from `at` (just after the opening backtick); returns its start
/// line, text, the index of the closing backtick and the line count there.
fn template(chars: &[char], at: usize, mut line: usize) -> (usize, String, usize, usize) {
    let (start, mut text, mut i) = (line, String::new(), at);
    while i < chars.len() {
        let c = chars[i];
        match c {
            '`' => return (start, text, i, line),
            '\\' => {
                if let Some(n) = chars.get(i + 1) {
                    if *n == 'n' { text.push(' '); } else if *n != '\n' { text.push(*n); } else { line += 1; }
                }
                i += 1;
            }
            '$' if chars.get(i + 1) == Some(&'{') => {
                let mut depth = 0;
                while i < chars.len() {
                    match chars[i] {
                        '{' => depth += 1,
                        '}' => { depth -= 1; if depth == 0 { break; } }
                        '\n' => line += 1,
                        _ => {}
                    }
                    i += 1;
                }
                text.push('…');
            }
            '\n' => { line += 1; text.push('\n'); }
            _ => text.push(c),
        }
        i += 1;
    }
    (start, text, i, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_literals_with_lines_and_skips_comments() {
        let body = "Rule one is long enough to be prompt text for the scanner to keep it. ".repeat(3);
        let src = format!("// a `comment` with a backtick\nconst a = 'it''s'\nexport const X = `\n{body}\nsecond ${{ y ? `inner` : 'z' }} line`\n");
        let found = templates(&src);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 3);
        assert!(found[0].1.contains("second … line"));
    }
}
