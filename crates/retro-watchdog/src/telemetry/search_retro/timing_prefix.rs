//! Pieces of local-search's stderr timing line glued to the start of a hit line. Before
//! ~/automation local-search `stderr_line.rs`, `eprintln!` wrote each formatted piece separately,
//! and in an Antigravity PTY with `| head -c N` those pieces landed before head's lines:
//! `route   /Users/…/d093286f….jsonl:123  [history/turn] …` (search-retro 812685146552855153702,
//! a1b92138…jsonl:2, whose agent then read d093286f at :24). The hit was printed and seen, so its
//! file is a returned file.

/// `line` without leading timing pieces when a locator with its `[corpus/kind]` label follows
/// them, its whitespace kept so an indented hit stays indented; otherwise `line` unchanged.
pub(super) fn strip(line: &str) -> &str {
    if !line.contains("  [") {
        return line;
    }
    let mut rest = line;
    let mut skipped = false;
    loop {
        let trimmed = rest.trim_start();
        let Some(token) = trimmed.split_whitespace().next() else { return line };
        if located(token) {
            return if skipped { rest } else { line };
        }
        if !piece(token) {
            return line;
        }
        skipped = true;
        rest = &trimmed[token.len()..];
    }
}

/// A word, number or punctuation of `route <r>, embed <n> ms, search <n> ms, …`.
fn piece(token: &str) -> bool {
    const WORDS: [&str; 11] = ["route", "vector", "mixed", "keyword", "embed", "search", "rerank", "extras", "print", "ms", "previous"];
    let word = token.trim_matches(|c: char| c == ',' || c == '.');
    word.is_empty() || WORDS.contains(&word) || word.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// A `path:line` locator or a report URL.
fn located(token: &str) -> bool {
    token.starts_with("https://") || (token.contains('/') && token.split_once(':').is_some_and(|(_, at)| at.starts_with(|c: char| c.is_ascii_digit())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_pieces_before_a_locator_are_dropped() {
        // a1b92138…jsonl:2 and 257e71ec…jsonl:54 (Antigravity, `| head -c 8000`).
        assert_eq!(strip("route   /a/-x/d093286f.jsonl:123  [history/turn] t"), "   /a/-x/d093286f.jsonl:123  [history/turn] t");
        assert_eq!(strip(", embed   /a/b/c.jsonl:30  [history/assistant] t"), "   /a/b/c.jsonl:30  [history/assistant] t");
        assert_eq!(strip("route vector  /u/token/antigravity-speed.ts:40-76  [code/block] x"), "  /u/token/antigravity-speed.ts:40-76  [code/block] x");
        assert_eq!(strip("74  https://r.fun/gemini-4-carbon-x-status-20261103/  [report/overview] x"), "  https://r.fun/gemini-4-carbon-x-status-20261103/  [report/overview] x");
    }

    #[test]
    fn a_prefixed_hit_names_its_file() {
        let output = "route   /a/-x/d093286f.jsonl:123  [history/turn] t\n    text\nvector  /a/-y/30d540d1.jsonl:358  [history/assistant] t";
        assert_eq!(super::super::returned::returned_files(output), ["-x/d093286f.jsonl", "-y/30d540d1.jsonl"]);
    }

    #[test]
    fn other_lines_stay_whole() {
        for line in ["  /a/b.jsonl:3  [history/turn] t", "72    ag1 note · 2026-10-13", "web/src/a.ts:3:const x = 1", "fix  /a/b.rs:3  [code/block] x", "route vector, embed 1.0 ms, search 2.0 ms"] {
            assert_eq!(strip(line), line);
        }
    }
}
