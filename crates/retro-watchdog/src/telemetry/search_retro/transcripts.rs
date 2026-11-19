//! Whether an rg/grep sweep after a NONE read only agent transcripts that the NONE's corpus never
//! covered. Transcripts are the `history` corpus (local-search `history.rs` roots: `~/.claude`,
//! `~/.claude-profiles/*`, `~/.codex`, `~/.codex-profiles/*`, Orca's Codex home); a `--corpus code`
//! or `docs` NONE says nothing about them, so a match there cannot prove that NONE wrong.
//!
//! Search-retro 8988472317094567194: `local-search search mars-i18n-server-target --corpus code`
//! printed NONE at 13:07 SGT (0e0dcd6a…jsonl:209), correctly: the name first entered code with
//! ~/automation 438a672 (`data_target.rs`) at 13:30. The next steps ran `local-search tools` and
//! `grep -o 'mars-i18n-server-target…' ~/.claude-profiles/team4/projects/…/cf0987b5….jsonl`
//! (0e0dcd6a…jsonl:229), which printed the cargo commands of another session, and the NONE was
//! counted false. A NONE without `--corpus`, or one that lists `history` or `all`, keeps counting.
use super::pattern;

const ROOTS: &[&str] = &["/.claude/", "/.claude-profiles/", "/.codex/", "/.codex-profiles/", "/orca/codex-runtime-home/"];

/// The corpora a local-search command named with `--corpus`, or None when it named none.
pub(super) fn corpora(command: &str) -> Option<Vec<String>> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let at = words.iter().position(|w| *w == "--corpus" || w.starts_with("--corpus="))?;
    let value = words[at].strip_prefix("--corpus=").or_else(|| words.get(at + 1).copied())?;
    Some(value.trim_matches(|c| c == '"' || c == '\'').split(',').map(str::to_string).collect())
}

/// True when the NONE named its corpora and none of them reads transcripts.
pub(super) fn none_skips_history(search: &str) -> bool {
    corpora(search).is_some_and(|c| !c.iter().any(|x| x == "history" || x == "all"))
}

/// True when `segment` (one rg/grep simple command) names file operands and every one is a
/// `.jsonl` transcript under a history root.
pub(super) fn only_transcripts(segment: &str, home: &str) -> bool {
    let files = pattern::files(segment);
    !files.is_empty() && files.iter().all(|f| is_transcript(f, home))
}

fn is_transcript(path: &str, home: &str) -> bool {
    let path = path.trim_matches(|c| c == '"' || c == '\'');
    let path = match path.strip_prefix("~/").or_else(|| path.strip_prefix("$HOME/")) {
        Some(rest) => format!("{home}/{rest}"),
        None => path.to_string(),
    };
    path.ends_with(".jsonl") && ROOTS.iter().any(|r| path.contains(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/b";

    #[test]
    fn a_code_none_does_not_cover_transcripts() {
        // 0e0dcd6a…jsonl:209 and :229.
        assert!(none_skips_history("local-search search mars-i18n-server-target --corpus code --per-source 1 --limit 20 --lines"));
        assert!(only_transcripts("grep -o 'mars-i18n-server-target[^\"]\\{0,120\\}' /Users/b/.claude-profiles/team4/projects/-Volumes-x/cf0987b5.jsonl", HOME));
        assert!(only_transcripts("rg -n foo ~/.codex/sessions/2026/11/07/rollout.jsonl", HOME));
    }

    #[test]
    fn history_searches_and_code_sweeps_still_count() {
        assert!(!none_skips_history("local-search search foo --corpus history"));
        assert!(!none_skips_history("local-search search foo --corpus code,history"));
        assert!(!none_skips_history("local-search search foo"));
        assert!(!only_transcripts("grep -n foo /Users/b/.claude-profiles/team4/projects/x/a.jsonl src/main.rs", HOME));
        assert!(!only_transcripts("grep -n foo /Users/b/.claude/skills/report/SKILL.md", HOME));
        assert!(!only_transcripts("grep -rn foo .", HOME));
    }
}
