//! Simple commands of a shell line: split at `;`, `|`, `&`, newlines and parentheses outside quotes,
//! the way the shell reads them. A backslash escapes the next character outside quotes and inside
//! double quotes (as `steps::quoted` reads a quoted word); single quotes take it literally.
//! Search-retro 816476999821716281017 (3a77a3fb…jsonl:1155): `local-search search
//! "['\"\`]/(pricing|changelog|…)['\"\`/?]"` closed its quote at the escaped `\"`, split at the `(`,
//! and left the query `['"` with no names, so its answered hit could never count as adopted.

/// Each simple command with whether a single `|` fed it (`||` does not).
pub(super) fn commands(command: &str) -> Vec<(bool, String)> {
    let chars: Vec<char> = command.chars().collect();
    let (mut out, mut current, mut quote, mut piped) = (Vec::new(), String::new(), None::<char>, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && quote != Some('\'') {
            // A backslash-newline continues the line: the shell drops both, as whitespace here
            // (fc1d7a81…jsonl:577 ran `… && \` then `~/.local/bin/local-search search …`).
            match chars.get(i + 1) {
                Some('\n') => current.push(' '),
                _ => current.extend(chars.get(i..i + 2).unwrap_or(&chars[i..])),
            }
            i += 2;
            continue;
        }
        match (quote, c) {
            (Some(q), _) if c == q => { quote = None; current.push(c); }
            (Some(_), _) => current.push(c),
            (None, '"' | '\'') => { quote = Some(c); current.push(c); }
            (None, ';' | '|' | '&' | '\n' | '(' | ')') => {
                if !current.trim().is_empty() { out.push((piped, std::mem::take(&mut current))); }
                current.clear();
                piped = c == '|' && chars.get(i + 1) != Some(&'|') && (i == 0 || chars[i - 1] != '|');
            }
            (None, _) => current.push(c),
        }
        i += 1;
    }
    if !current.trim().is_empty() { out.push((piped, current)); }
    out
}

#[cfg(test)]
mod tests {
    use super::commands;

    fn texts(command: &str) -> Vec<String> {
        commands(command).into_iter().map(|(_, s)| s.trim().to_string()).collect()
    }

    #[test]
    fn an_escaped_double_quote_keeps_the_word_open() {
        // 3a77a3fb…jsonl:1155, as the transcript holds the command.
        let line = r#"~/.local/bin/local-search search "['\"\`]/(pricing|changelog|wiki)['\"\`/?]" --corpus code --lines 2>&1 | head -40"#;
        assert_eq!(texts(line), [r#"~/.local/bin/local-search search "['\"\`]/(pricing|changelog|wiki)['\"\`/?]" --corpus code --lines 2>"#, "1", "head -40"]);
    }

    #[test]
    fn single_quotes_take_a_backslash_literally() {
        assert_eq!(texts(r"grep -n 'a\' x.rs; ls"), [r"grep -n 'a\' x.rs", "ls"]);
    }

    #[test]
    fn an_escaped_separator_outside_quotes_does_not_split() {
        assert_eq!(texts(r"find . -name x -exec rm {} \; | wc -l"), [r"find . -name x -exec rm {} \;", "wc -l"]);
        assert_eq!(commands("a | b || c").iter().map(|(p, _)| *p).collect::<Vec<_>>(), [false, true, false]);
    }

    #[test]
    fn a_backslash_newline_continues_the_line() {
        // fc1d7a81…jsonl:577: the search after `&& \` is its own command word.
        let line = "perl -pi -e 's/a/b/' x.rs && \\\n~/.local/bin/local-search search \"hant\" --corpus code";
        assert_eq!(texts(line), ["perl -pi -e 's/a/b/' x.rs", "~/.local/bin/local-search search \"hant\" --corpus code"]);
        assert_eq!(texts("ls a \\\n  b"), ["ls a    b"]);
    }
}
