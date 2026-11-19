//! A value a hit printed that the next tool call uses verbatim adopts the hit (founder decision 3A
//! on search-retro 813111088844585114560). d4b75593…jsonl:3816: a NONE whose `outside --path` near
//! line read ``/// `/me/account`, the account-and-security document`` was followed by
//! `mars-ui-tools ios integrate … --route /me/account`, the answer used without naming its file.
//! Only the next tool call counts, it must be a shell command that searches nothing, and the value
//! must be backticked or quoted in the output and shaped like an identifier (a digit, `_`, `-`,
//! `.` or `/` inside), so prose words never match.
use super::steps::{Kind, Step};

const MIN_VALUE: usize = 4;

/// True when the first tool call after the search is a non-search shell command holding, as a
/// whole word, a backticked or quoted identifier-shaped value printed in `output`.
pub(super) fn used(after: &[Step], output: &str) -> bool {
    let Some(next) = after.iter().find(|s| matches!(s.kind, Kind::Shell | Kind::Tool)) else { return false };
    if next.kind != Kind::Shell || searches(&next.text) {
        return false;
    }
    let words: Vec<&str> = next.text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | '`' | ',' | ';')).collect();
    printed(output).iter().any(|v| words.iter().any(|w| names(w, v)))
}

/// `word` is printed value `v` as written, the file of a printed `path:line` locator, or a longer
/// path ending in it. Search-retro 88319497877162154254: 8d559cdf…jsonl:795 printed
/// `` `.claude/skills/report/scripts/captureWeb.js:52-58,220-224` `` and the next call ran
/// `sed -n 45,62p .claude/skills/report/scripts/captureWeb.js`; 8ee8b5e0…jsonl:739 printed
/// `join("runtime-retro-cursors.json")` and the next call ran `ls -la ~/.local/state/cpu-watchdog/
/// runtime-retro-cursors.json`. Only a value with `-`, `_` or `/` may end a longer path, so a bare
/// `main.rs` printed for one crate does not adopt a read of another crate's `main.rs`.
fn names(word: &str, v: &str) -> bool {
    let located = v.split_once(':').filter(|(_, at)| at.starts_with(|c: char| c.is_ascii_digit()) && at.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | ',')));
    let file = located.map_or(v, |(file, _)| file);
    [v, file].into_iter().any(|v| {
        let tail = !v.starts_with('/') && v.contains(['-', '_', '/']) && word.ends_with(&format!("/{v}"));
        word == v || tail || selects(word, v)
    })
}

/// `word` is the class or id selector of printed class name `v` (`.composer-send`, `#send-button`).
/// Search-retro 83879279163991646107: 301dd1e7…jsonl:1180 printed
/// `createComposerToolbar.tsx:57: className="composer-send transition-opacity …"` and the next call
/// ran `capture.js web … --click ".composer-send"`, the hit used without naming its file.
fn selects(word: &str, v: &str) -> bool {
    class_name(v) && (word.starts_with('.') || word.starts_with('#')) && &word[1..] == v
}

/// A CSS class name worth matching: ASCII letters, digits, `-` and `_`, with a `-` or `_` inside.
fn class_name(c: &str) -> bool {
    c.len() >= MIN_VALUE && c.starts_with(|ch: char| ch.is_ascii_alphabetic()) && c.contains(['-', '_']) && c.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

/// A path word of a backticked command (`bun .claude/skills/x/run-turn.ts "<question>"`): it holds
/// a `/` and a file extension and no placeholder or glob character. Search-retro
/// 814821998552520240686: agent-acde90c9…jsonl:64 printed the demo-account-takeover.md row
/// `` `bun .claude/skills/performance/subskills/round-optimization/scripts/run-turn.ts "<question>" --label <name>` ``
/// and the next call ran that script, the hit used without naming its file.
fn command_path(w: &str) -> bool {
    let name = w.rsplit('/').next().unwrap_or("");
    w.len() >= MIN_VALUE && w.contains('/') && name.contains('.') && !name.ends_with('.') && !w.contains(['<', '>', '[', ']', '*', '…', '$', '{', '}'])
}

/// Backticked and double-quoted spans of every output line, identifier-shaped, MIN_VALUE+ chars,
/// the path words of a backticked command (`command_path`),
/// and the class names of a double-quoted class list (`className="composer-send transition-opacity"`).
fn printed(output: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        for quote in ['`', '"'] {
            for (i, span) in line.split(quote).enumerate() {
                let shaped = span.chars().count() >= MIN_VALUE && !span.contains(char::is_whitespace) && span.chars().any(|c| c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/'));
                if i % 2 == 1 && shaped && !out.iter().any(|v| v == span) {
                    out.push(span.to_string());
                }
                if i % 2 == 1 && quote == '`' && span.contains(char::is_whitespace) {
                    for path in span.split_whitespace().filter(|w| command_path(w)) {
                        if !out.iter().any(|v| v == path) {
                            out.push(path.to_string());
                        }
                    }
                }
                if i % 2 == 1 && quote == '"' && span.contains(char::is_whitespace) {
                    for class in span.split_whitespace().filter(|c| class_name(c)) {
                        if !out.iter().any(|v| v == class) {
                            out.push(class.to_string());
                        }
                    }
                }
            }
        }
    }
    out
}

/// A simple command of `command` that searches: rg, grep, find, local-search search|query,
/// agent-memory recall or session-status. An rg/grep a pipe feeds filters another program's
/// lines and searches nothing: 9f6f276e…jsonl:153 ended its download loop with
/// `sips -g pixelWidth *.jpg | grep -v '^$'` (search-retro 85383374938275326400).
pub(super) fn searches(command: &str) -> bool {
    super::quality::commands(command).iter().any(|(piped, segment)| {
        let words: Vec<&str> = segment.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
        match words.first().map(|p| p.rsplit('/').next().unwrap_or(p)) {
            Some("rg" | "grep" | "egrep" | "ugrep") => !piped,
            Some("find" | "session-status") => true,
            Some("local-search") => matches!(words.get(1), Some(&("search" | "query"))),
            Some("agent-memory") => words.get(1) == Some(&"recall"),
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(text: &str) -> Step {
        Step { at_ms: 1.0, id: "2".into(), kind: Kind::Shell, text: text.into(), line: 1, turn: "2".into() }
    }

    #[test]
    fn a_printed_route_the_next_command_passes_adopts_the_hit() {
        let out = "  -- outside --path: 16 file(s) hold it on one line, first 3 --\nios/MarsCRM/Features/Settings/AccountSecurityScreen.swift:3:/// `/me/account`, the account-and-security document";
        assert!(used(&[shell("SP=/tmp/x; mars-ui-tools ios integrate --screenshot $SP/a.png -- --route /me/account > $SP/log 2>&1")], out));
        // A search for it is a re-search, not a use; a prose word or a later call does not count.
        assert!(!used(&[shell("local-search search \"/me/account\" --corpus code")], out));
        assert!(!used(&[shell("rg -n '/me/account' ios")], out));
        assert!(!used(&[shell("echo account")], "a.rs:1:// `account` page"));
        let late = [Step { kind: Kind::Tool, ..shell("{\"file_path\":\"x.swift\"}") }, shell("open /me/account")];
        assert!(!used(&late, out));
        // Part of a longer word is no use.
        assert!(!used(&[shell("curl https://x/me/accounts")], out));
    }

    #[test]
    fn a_printed_locator_or_file_name_inside_a_longer_path_adopts_the_hit() {
        // 8d559cdf…jsonl:795 → :804: the locator's file, read at the printed lines.
        let out = "Web preview | `capture.js web` writes it (`.claude/skills/report/scripts/captureWeb.js:52-58,220-224`)";
        assert!(used(&[shell("sed -n 45,62p .claude/skills/report/scripts/captureWeb.js; sed -n 215,230p .claude/skills/report/scripts/captureWeb.js")], out));
        // 8ee8b5e0…jsonl:739 → :755: a printed file name at the end of the path the next call lists.
        let out = "   1 join(\"runtime-retro-cursors.json\")\n   1 join(\"task-stall-cursors.json\")";
        assert!(used(&[shell("ls -la ~/.local/state/cpu-watchdog/runtime-retro-cursors.json")], out));
        // Not a longer name, not a bare generic file name, not a different file.
        assert!(!used(&[shell("ls ~/x/old-runtime-retro-cursors.json")], out));
        assert!(!used(&[shell("sed -n 1,9p crates/other/src/main.rs")], "see `main.rs`"));
        assert!(!used(&[shell("sed -n 1,9p .claude/skills/report/scripts/captureIos.js")], "(`.claude/skills/report/scripts/captureWeb.js:52-58`)"));
    }

    #[test]
    fn a_printed_class_the_next_command_selects_adopts_the_hit() {
        // 301dd1e7…jsonl:1180 → capture.js clicked `.composer-send` from the printed class list.
        let out = "Mars_CRM/web/src/components/library/chatInput/createComposerToolbar.tsx:57:          className=\"composer-send transition-opacity duration-200 disabled:opacity-30\"";
        assert!(used(&[shell("bun capture.js web --run $R --fill '{\"selector\":\".mars-composer textarea\"}' --click \".composer-send\" --record 45")], out));
        // The bare class name passed as an argument is the value itself.
        assert!(used(&[shell("open-class composer-send")], out));
        // A different class, a longer selector word, or a prose word inside quotes does not count.
        assert!(!used(&[shell("bun capture.js web --click \".composer-sender\"")], out));
        assert!(!used(&[shell("bun capture.js web --click \".mars-composer\"")], out));
        assert!(!used(&[shell("echo .done")], "a.rs:1: \"all done here\""));
        // Utility classes without a letter start or with `:` are not class names to match.
        assert!(!used(&[shell("bun capture.js web --click \".disabled:opacity-30\"")], out));
    }

    #[test]
    fn a_script_path_inside_a_printed_command_the_next_command_runs_adopts_the_hit() {
        // agent-acde90c9…jsonl:64 → :70: the demo-account-takeover.md row printed the run-turn.ts command.
        let out = "| Send one chat turn as demo on beta | `bun .claude/skills/performance/subskills/round-optimization/scripts/run-turn.ts \"<question>\" --label <name> [--sse /abs/out.sse]` prints the thread id |";
        assert!(used(&[shell("cd /x/Mars_agent && bun .claude/skills/performance/subskills/round-optimization/scripts/run-turn.ts \"演示数据\" --label mv-1 --sse \"$TMPDIR/a.sse\" 2>&1 | tail -c 4000")], out));
        // A placeholder path, a directory without a file name, or another script is no use.
        assert!(!used(&[shell("ls /abs/out.sse")], "`bun x.ts --sse /abs/<out>.sse`"));
        assert!(!used(&[shell("ls .claude/skills/x/")], "`cd .claude/skills/x/ && bun a.ts`"));
        assert!(!used(&[shell("bun .claude/skills/performance/subskills/round-optimization/scripts/analyze-rounds.ts t1")], out));
        // A search for the printed script is a re-search.
        assert!(!used(&[shell("rg -n label .claude/skills/performance/subskills/round-optimization/scripts/run-turn.ts")], out));
    }

    #[test]
    fn a_grep_fed_by_a_pipe_is_a_filter_not_a_search() {
        // 9f6f276e…jsonl:141 → :153 downloads the printed `"wiki-deep-research"` and filters sips output.
        let out = "web/src/data/productMedia.json:592:    \"wiki-deep-research\": {\"width\":1568,\"height\":944},";
        assert!(used(&[shell("cd $W/dl; for f in wiki-deep-research wiki-relationship-insights; do curl -o $f.webp https://x/wiki/$f.webp; done; sips -g pixelWidth *.jpg | grep -v '^$'")], out));
        // A grep with no pipe into it still searches.
        assert!(!used(&[shell("grep -rn wiki-deep-research web/src")], out));
        assert!(!used(&[shell("curl -o a.webp https://x/wiki/wiki-deep-research.webp; rg wiki-deep-research web")], out));
    }
}
