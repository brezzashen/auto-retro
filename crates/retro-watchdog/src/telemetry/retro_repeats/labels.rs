//! Hindsight step labels. With the whole session in view, a tool call is `duplicate` when the same
//! call already succeeded in this session and nothing was written since, `retry` when the same call
//! ran before and failed, else fresh; `failed` comes from its own result. A write (an edit tool, a
//! patch, or a shell command that changes files, Git or installs) clears what was seen, because a
//! check after a change is new work.
use regex::Regex;
use std::{collections::HashMap, sync::LazyLock};

static SHELL_WRITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"\bsed\s+-i|\bperl\s+-\w*i|\btee\b|\bmv\s|\bcp\s|\brm\s|\bgit\s+(?:commit|add|checkout|restore|reset|merge|rebase|pull|apply|worktree)\b|\binstall\b|\bcat\s*>|[^2&>]>\s*[^&\s/]").unwrap());

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Label { Fresh, Duplicate, Retry }

impl Label {
    pub(super) fn name(self) -> &'static str {
        match self { Label::Fresh => "fresh", Label::Duplicate => "duplicate", Label::Retry => "retry" }
    }
}

/// One session's calls so far: normalized call text → whether its result failed (None: no result yet).
#[derive(Default)]
pub(super) struct Session { seen: HashMap<String, Option<bool>> }

impl Session {
    /// Labels a call and returns the key its result is recorded under.
    pub(super) fn call(&mut self, text: &str) -> (Label, Option<String>) {
        if writes(text) { self.seen.clear(); return (Label::Fresh, None); }
        let key = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let label = match self.seen.get(&key) {
            Some(Some(false)) => Label::Duplicate,
            Some(Some(true)) => Label::Retry,
            _ => Label::Fresh,
        };
        self.seen.insert(key.clone(), None);
        (label, Some(key))
    }

    pub(super) fn result(&mut self, key: &str, failed: bool) {
        if let Some(slot) = self.seen.get_mut(key) { *slot = Some(failed); }
    }
}

fn writes(text: &str) -> bool {
    text.contains("\"old_string\"") || text.contains("*** Begin Patch") || text.contains("\"edits\"")
        || (text.contains("\"file_path\"") && text.contains("\"content\"")) || SHELL_WRITE.is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_need_a_prior_success_and_no_write_between() {
        let mut s = Session::default();
        let (first, key) = s.call("git status --short");
        assert_eq!(first, Label::Fresh);
        s.result(key.as_deref().unwrap(), false);
        assert_eq!(s.call("git  status   --short").0, Label::Duplicate);
        let (_, key) = s.call("cargo test");
        s.result(key.as_deref().unwrap(), true);
        assert_eq!(s.call("cargo test").0, Label::Retry);
        assert_eq!(s.call(r#"{"file_path":"/a.rs","old_string":"x","new_string":"y"}"#).0, Label::Fresh);
        assert_eq!(s.call("git status --short").0, Label::Fresh);
        s.call("sed -i '' s/a/b/ f.rs");
        assert_eq!(s.call("cargo test").0, Label::Fresh);
        assert!(!writes("cargo test 2>&1 | tail -5") && !writes("ls >/dev/null") && writes("echo x > out.txt"));
    }
}
