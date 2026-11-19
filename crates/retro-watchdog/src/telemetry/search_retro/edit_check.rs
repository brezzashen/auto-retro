//! A grep that reads back only files its own command just edited in place checks that edit; it is
//! not a sweep for the name a NONE missed. Search-retro 83485594338778809860: the code NONE for
//! `aria-expanded:opacity-0 mars-head` under `Mars_CRM/web/src` was right (`rg -F` exit 1), and
//! c93dccaa…jsonl:1650 `sed -i '' 's/…/' …/conversation-share.md && grep -c 'mars-head-capsule \[aria'
//! …/conversation-share.md` (a recipe outside that path) printed `2` and counted as a false NONE.
use super::{none, pattern, quality};

/// True when `segment`, one simple command of `command`, is an rg/grep whose every file an earlier
/// `sed -i` / `perl -i` segment of `command` named.
pub(super) fn verifies(command: &str, segment: &str) -> bool {
    let files = pattern::files(segment);
    if files.is_empty() {
        return false;
    }
    let mut edited: Vec<String> = Vec::new();
    for (_, seg) in quality::commands(&none::strip_heredocs(command)) {
        if seg == segment {
            break;
        }
        edited.extend(in_place(&seg));
    }
    files.iter().all(|f| edited.contains(f))
}

/// The non-flag words of a `sed`/`perl` command that edits in place (`-i`, `-i ''`, `-i.bak`,
/// `-pi`, `--in-place`); empty for any other command. The script word is included, harmlessly:
/// no grep reads a file named like `s/a/b/`.
fn in_place(segment: &str) -> Vec<String> {
    let words = pattern::words(segment);
    let Some(at) = words.iter().position(|w| !w.contains('=') || w.starts_with('-')) else { return Vec::new() };
    let program = words[at].rsplit('/').next().unwrap_or_default();
    let args = &words[at + 1..];
    let edits = matches!(program, "sed" | "gsed" | "perl")
        && args.iter().any(|w| w.starts_with("--in-place") || (w.starts_with('-') && !w.starts_with("--") && w[1..].contains('i')));
    if !edits {
        return Vec::new();
    }
    args.iter().filter(|w| !w.is_empty() && !w.starts_with('-')).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::verifies;

    #[test]
    fn a_grep_of_the_file_just_edited_checks_the_edit() {
        // c93dccaa…jsonl:1650.
        let cmd = r#"sed -i '' 's/\.mars-head-capsule\[aria-label=\\"更多操作\\"\]/.mars-head-capsule [aria-label=\\"更多操作\\"]/g' .claude/f/share.md && grep -c 'mars-head-capsule \[aria' .claude/f/share.md && bun verify.js share"#;
        assert!(verifies(cmd, " grep -c 'mars-head-capsule \\[aria' .claude/f/share.md "));
        assert!(verifies("perl -pi -e 's/a/b/' x.ts; grep -n b x.ts", " grep -n b x.ts"));
    }

    #[test]
    fn a_sweep_of_other_files_still_counts() {
        // A file the edit did not touch, a tree, a read-only sed, or no edit at all.
        assert!(!verifies("sed -i '' 's/a/b/' x.ts && grep -n b x.ts y.ts", " grep -n b x.ts y.ts"));
        assert!(!verifies("sed -i '' 's/a/b/' x.ts && rg -n b src", " rg -n b src"));
        assert!(!verifies("sed -n 1,5p x.ts; grep -n b x.ts", " grep -n b x.ts"));
        assert!(!verifies("grep -n b x.ts; sed -i '' 's/a/b/' x.ts", "grep -n b x.ts"));
        assert!(!verifies("rg -n b", "rg -n b"));
    }
}
