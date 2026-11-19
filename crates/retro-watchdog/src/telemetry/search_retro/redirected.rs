//! A search whose stdout went to a file (`local-search search … > hits.txt; wc -l < hits.txt; head
//! -5 hits.txt`) shows the agent only a few lines in its own result; the agent reads the rest with a
//! later call (`cut -c1-300 hits.txt | sed -n 5,135p`). That later result is the search's output too,
//! so the files it names are returned files. Search-retro 816961722485483061920 (8a012622…jsonl:2103):
//! a report search wrote 135 lines to `rw-hits.txt`, its result printed the first two pages, the agent
//! read the file at :2118 and three turns later ran a loop over 19 report slugs from it (:2152), yet
//! the hit counted unadopted because only the two printed pages were its returned files.
use super::adopt::{window, ADOPT_MS, ADOPT_TURNS};
use super::quality::result_of;
use super::steps::{Kind, Step};
use std::borrow::Cow;

/// Shortest file name a later call may name alone (after a `cd`) to read the redirect file.
const MIN_NAME: usize = 6;

/// The call's output: its own result, then the results of the calls in the adoption window that
/// read the file its local-search stdout was redirected to.
pub(super) fn output<'a>(command: &str, answer: &'a str, items: &[Step], after: &[Step], at_ms: f64, own: &str) -> Cow<'a, str> {
    let Some(path) = target(command) else { return Cow::Borrowed(answer) };
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    let reads = |s: &&Step| matches!(s.kind, Kind::Shell | Kind::Tool) && (s.text.contains(path.as_str()) || (name.len() >= MIN_NAME && names(&s.text, &name)));
    let more: Vec<&str> = window(after, at_ms, own, ADOPT_TURNS, ADOPT_MS).iter().filter(reads).filter_map(|s| result_of(items, &s.id)).map(|r| r.text.as_str()).collect();
    if more.is_empty() { Cow::Borrowed(answer) } else { Cow::Owned(std::iter::once(answer).chain(more).collect::<Vec<_>>().join("\n")) }
}

/// The name as a whole word, not part of a longer path or name.
fn names(text: &str, name: &str) -> bool {
    let joined = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/');
    text.match_indices(name).any(|(i, _)| !text[..i].chars().next_back().is_some_and(joined) && !text[i + name.len()..].chars().next().is_some_and(joined))
}

/// The file the stdout of the command's local-search search|query is written to (`> f`, `>> f`,
/// `1> f`, `&> f`, or joined `>f`), not `2> f`, `>&2` or `/dev/null`.
pub(super) fn target(command: &str) -> Option<String> {
    let start = command.match_indices("local-search").map(|(i, _)| i).find(|&i| {
        let rest = command[i + "local-search".len()..].trim_start();
        rest.starts_with("search") || rest.starts_with("query")
    })?;
    let words = words(&command[start..]);
    let mut found = None;
    for (i, w) in words.iter().enumerate() {
        let rest = ["&>>", "&>", "1>>", "1>", ">>", ">"].iter().find_map(|op| w.strip_prefix(op));
        let Some(rest) = rest else { continue };
        let file = if rest.is_empty() { words.get(i + 1).map(String::as_str) } else { Some(rest) };
        match file {
            Some(f) if !f.starts_with('&') && f != "/dev/null" => found = Some(f.to_string()),
            _ => {}
        }
    }
    found
}

/// Unquoted words of the first simple command of `text`, quotes removed; a quoted word never reads
/// as a redirection (`"a>b"`). Ends at an unquoted `;`, `|`, newline or a `&` that is no part of a
/// redirection (`2>&1`, `&>`).
fn words(text: &str) -> Vec<String> {
    let (mut out, mut word, mut quoted, mut quote) = (Vec::new(), String::new(), false, None::<char>);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let flush = |out: &mut Vec<String>, word: &mut String, quoted: &mut bool| {
        if !word.is_empty() && !*quoted { out.push(std::mem::take(word)); }
        word.clear();
        *quoted = false;
    };
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some('\'') if c == '\'' => quote = None,
            Some('"') if c == '\\' && i + 1 < chars.len() => { word.push(chars[i + 1]); i += 1; }
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => { quote = Some(c); quoted = true; }
            None if c == '\\' && i + 1 < chars.len() => { word.push(chars[i + 1]); i += 1; }
            None if c.is_whitespace() && c != '\n' => flush(&mut out, &mut word, &mut quoted),
            None if c == ';' || c == '|' || c == '\n' => break,
            None if c == '&' && !(word.ends_with('>') || chars.get(i + 1) == Some(&'>')) => break,
            None => word.push(c),
        }
        i += 1;
    }
    flush(&mut out, &mut word, &mut quoted);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(kind: Kind, id: &str, turn: &str, text: &str) -> Step {
        Step { at_ms: 1.0, id: id.into(), kind, text: text.into(), line: 1, turn: turn.into() }
    }

    const CALL: &str = "cd /r && ~/.local/bin/local-search search \"a|b\" --corpus report --lines 2>&1 > /tmp/k/rw-hits.txt; wc -l < /tmp/k/rw-hits.txt; head -5 /tmp/k/rw-hits.txt";

    #[test]
    fn the_redirect_file_of_the_search() {
        assert_eq!(target(CALL).as_deref(), Some("/tmp/k/rw-hits.txt"));
        assert_eq!(target("local-search query x >>out.txt").as_deref(), Some("out.txt"));
        assert_eq!(target("local-search search x &> o.log").as_deref(), Some("o.log"));
        // stderr, fd duplication, /dev/null, a quoted `>` and a later command's redirect are not it.
        assert_eq!(target("local-search search x 2> err.txt | head"), None);
        assert_eq!(target("local-search search x >&2"), None);
        assert_eq!(target("local-search search x > /dev/null"), None);
        assert_eq!(target("local-search search \"a > b\" --lines"), None);
        assert_eq!(target("local-search search x | head -5; echo y > note.txt"), None);
        assert_eq!(target("echo x > a.txt; cat a.txt"), None);
    }

    /// 8a012622…jsonl:2103 → :2118 read the file → :2152 used slugs only the file held.
    #[test]
    fn a_later_read_of_the_file_is_the_search_output() {
        let after = vec![
            step(Kind::Result, "c0", "", "135\n  https://reports.example.com/singapore-perspective-20261022/  [report/overview]"),
            step(Kind::Shell, "c1", "t1", "cut -c1-300 /tmp/k/rw-hits.txt | sed -n 5,135p"),
            step(Kind::Result, "c1", "", "  https://reports.example.com/cn-seo-index-retro-20261109/  [report/section]"),
            step(Kind::Shell, "c2", "t2", "ls"),
            step(Kind::Result, "c2", "", "https://reports.example.com/not-read-20261101/"),
        ];
        let out = output(CALL, "135", &after, &after, 0.0, "t0");
        assert!(out.contains("cn-seo-index-retro-20261109"));
        assert!(!out.contains("not-read-20261101"));
        // Named alone after a cd, by its file name.
        let mut cd = after.clone_steps();
        cd[1].text = "cd /tmp/k && sed -n 5,135p rw-hits.txt".into();
        assert!(output(CALL, "135", &cd, &cd, 0.0, "t0").contains("cn-seo-index-retro-20261109"));
        // A longer name, or no redirect at all, adds nothing.
        cd[1].text = "cat old-rw-hits.txt".into();
        assert_eq!(output(CALL, "135", &cd, &cd, 0.0, "t0"), "135");
        assert_eq!(output("local-search search x | head -5", "135", &after, &after, 0.0, "t0"), "135");
    }

    trait CloneSteps { fn clone_steps(&self) -> Vec<Step>; }
    impl CloneSteps for Vec<Step> {
        fn clone_steps(&self) -> Vec<Step> { self.iter().map(|s| Step { at_ms: s.at_ms, id: s.id.clone(), kind: s.kind, text: s.text.clone(), line: s.line, turn: s.turn.clone() }).collect() }
    }
}
