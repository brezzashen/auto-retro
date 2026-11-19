//! Whether an rg/grep/find sweep read only trees no local-search corpus indexes. Such a sweep is
//! the rg the NONE line itself asks for ("~/automation (not its config/ or host/) … so rg there",
//! local-search `output/none.rs`), not a re-search that can prove the NONE wrong. Search-retro
//! 8986486579033029752: a code and docs NONE for `tailscale` (7746c089…jsonl:35) was counted false
//! because the next step's `rg -il tailscale ~/automation/config ~/automation/host ~/.local/bin`
//! (7746c089…jsonl:52) printed nothing while the step's history search, `defaults read` and
//! `scutil --nc list` printed lines naming Tailscale; no indexed file holds the word.
//!
//! The roots are those no corpus (code, docs, history, report, memory, watchdog, perf) reads:
//! ~/automation's skipped `config/` and `host/` (local-search `code.rs` repo_skips), installed
//! binaries and system program trees, and the separate Mars_studio repository (local-search prints
//! "unindexed: the index holds no file under /Volumes/MarsWork/Workspaces/Mars_studio"). Search-retro
//! 814125901426638775162: after a docs hit for `offload wait` in Mars_agent and ~/automation
//! (04a9629b…jsonl:961), `grep -rn "offload wait" .` in Mars_studio/.claude/skills (:964) read the
//! tree the index lacks, and was counted a re-search that refused the hit. Logs (`~/Library`,
//! `/Library`, `/private`, `/var`) and `~/.local/state` are watchdog sources and stay out of this
//! list. A relative operand is read against an unknown working directory and so never counts as
//! unindexed, unless a leading `cd` names an unindexed root (`within`) or the path passes through a
//! `node_modules` directory, which no corpus holds at any depth (git-ignored; the index has no
//! source under one). Search-retro 816805219412517074086: a right code NONE for guessed Mastra
//! retry names (0ddb7660…jsonl:848) was counted a fallback because the next step read the package's
//! bundle, `F=node_modules/.bun/@mastra+core@…/dist/…js; rg -n "isRetryable|…" $F` (:876).
use super::pattern;

/// Since search-retro 84886809318527941381 also third-party dependency sources (cargo registry and
/// git checkouts, rustup toolchains): `local-search search "KERN_PROC_PID|p_starttime|kp_proc"` in
/// ~/automation was a right NONE (05b55291…jsonl:2099), and `d=$(ls -d ~/.cargo/registry/src/*/libc-0.2.*
/// …); grep -rn "…KERN_PROC_PID" $d/src/unix/bsd/apple/` (:2115) found the name in the libc crate.
const ROOTS: &[&str] = &["~/automation/config", "~/automation/host", "~/.local/bin", "/Applications", "/usr", "/opt", "/etc", "/System", "/Volumes/MarsWork/Workspaces/Mars_studio", "~/.cargo/registry", "~/.cargo/git", "~/.rustup"];

/// The user's home, for callers that do not carry it.
pub(super) fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// True when `command` starts with `cd <dir>` into an unindexed root and `segment` (one of its
/// simple commands) reads only relative paths or none: its sweep stays inside that root.
pub(super) fn within(command: &str, segment: &str, home: &str) -> bool {
    let Some(dir) = command.trim_start().strip_prefix("cd ").and_then(|rest| rest.split(|c| c == ';' || c == '&').next()).map(str::trim) else { return false };
    under_root(dir, home) && operands(segment).iter().all(|p| !p.starts_with('/') && !p.starts_with('~') && !p.starts_with('$'))
}

/// True when `segment` (one rg, grep or find simple command) names at least one path and every
/// path lies under an unindexed root.
#[cfg(test)]
fn only_unindexed(segment: &str, home: &str) -> bool {
    only_unindexed_in("", segment, home)
}

/// `only_unindexed`, with a `$NAME`/`${NAME}` operand read as the first absolute or home path of
/// the `NAME=` assignment in the same `command` (`d=$(ls -d ~/.cargo/registry/…); grep … $d/src`).
/// An operand whose variable the command does not assign stays as written, so it never counts.
pub(super) fn only_unindexed_in(command: &str, segment: &str, home: &str) -> bool {
    let paths: Vec<String> = operands(segment).iter().map(|p| expand_var(command, p)).collect();
    !paths.is_empty() && paths.iter().all(|p| under_root(p, home))
}

/// `operand` with a leading `$NAME`/`${NAME}` replaced by the path `command` assigns to it, else as written.
pub(super) fn expand_var(command: &str, operand: &str) -> String {
    let bare = operand.trim_matches(|c| c == '"' || c == '\'');
    let Some(rest) = bare.strip_prefix('$') else { return operand.to_string() };
    let (name, tail) = match rest.strip_prefix('{') {
        Some(inner) => match inner.split_once('}') { Some(pair) => pair, None => return operand.to_string() },
        None => rest.split_at(rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len())),
    };
    assigned(command, name).map_or_else(|| operand.to_string(), |value| format!("{value}{tail}"))
}

/// The first `/…`, `~/…` or `$HOME/…` word of the value assigned to `name` at a word start.
fn assigned(command: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let start = command.match_indices(&key).map(|(i, _)| i).find(|&i| i == 0 || command[..i].ends_with(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '(' | '|')))?;
    let value = command[start + key.len()..].split(|c| c == ';' || c == '\n').next()?;
    value
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | '`'))
        .find(|w| w.starts_with('/') || w.starts_with("~/") || w.starts_with("$HOME/") || in_node_modules(w))
        .map(|w| w.trim_end_matches('/').to_string())
}

/// The paths a sweep reads: rg/grep file operands (`pattern::files`), or a find's start points.
fn operands(segment: &str) -> Vec<String> {
    let files = pattern::files(segment);
    if !files.is_empty() {
        return files;
    }
    let words: Vec<&str> = segment.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
    if words.first().is_some_and(|p| p.rsplit('/').next() == Some("find")) {
        return words[1..].iter().take_while(|w| !w.starts_with('-') && !matches!(**w, "(" | "!" | "\\(")).map(|w| w.to_string()).collect();
    }
    Vec::new()
}

fn under_root(path: &str, home: &str) -> bool {
    if in_node_modules(path) {
        return true;
    }
    let path = expand(path.trim_matches(|c| c == '"' || c == '\''), home);
    let path = path.trim_end_matches('/');
    ROOTS.iter().map(|r| expand(r, home)).any(|r| path == r || path.starts_with(&format!("{r}/")))
}

/// True when a component of `path` is `node_modules` (relative or absolute).
fn in_node_modules(path: &str) -> bool {
    path.trim_matches(|c| c == '"' || c == '\'').split('/').any(|c| c == "node_modules")
}

fn expand(path: &str, home: &str) -> String {
    for prefix in ["~/", "$HOME/", "${HOME}/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/b";

    #[test]
    fn a_sweep_of_unindexed_roots_only_is_unindexed() {
        // 7746c089…jsonl:52: the rg the code NONE pointed at, over config/, host/ and installed binaries.
        assert!(only_unindexed("rg -il tailscale ~/automation/config ~/automation/host ~/.local/bin", HOME));
        assert!(only_unindexed("grep -rn tailscale /Users/b/automation/config/frp \"$HOME/.local/bin\"", HOME));
        assert!(only_unindexed("find /Applications -maxdepth 1 -iname '*tailscale*'", HOME));
    }

    #[test]
    fn a_variable_operand_reads_its_assignment_in_the_command() {
        // 05b55291…jsonl:2115: the libc crate source, found through `$d`.
        let command = "d=$(ls -d ~/.cargo/registry/src/*/libc-0.2.* 2>/dev/null | sort -V | tail -1); echo $d; grep -rn \"pub struct kinfo_proc\\|p_starttime\\|pub const KERN_PROC_PID\" $d/src/unix/bsd/apple/ 2>/dev/null | head";
        let segment = "grep -rn \"pub struct kinfo_proc\\|p_starttime\\|pub const KERN_PROC_PID\" $d/src/unix/bsd/apple/ 2>/dev/null ";
        assert!(!only_unindexed(segment, HOME));
        assert!(only_unindexed_in(command, segment, HOME));
        assert!(only_unindexed_in("D=\"$HOME/.rustup/toolchains\"; rg -n x ${D}/lib", "rg -n x ${D}/lib", HOME));
        // Unassigned, or assigned an indexed tree: not unindexed.
        assert!(!only_unindexed_in("grep -rn x $d/src", "grep -rn x $d/src", HOME));
        assert!(!only_unindexed_in("d=~/automation/crates; grep -rn x $d", "grep -rn x $d", HOME));
        // `old_d=` does not assign `d`.
        assert!(!only_unindexed_in("old_d=~/.cargo/registry; grep -rn x $d", "grep -rn x $d", HOME));
    }

    #[test]
    fn a_sweep_touching_an_indexed_tree_or_a_relative_path_is_not() {
        assert!(!only_unindexed("rg -il tailscale ~/automation/config ~/automation/crates", HOME));
        assert!(!only_unindexed("rg -n tailscale ~/.codex/skills", HOME));
        assert!(!only_unindexed("rg -n tailscale src", HOME));
        assert!(!only_unindexed("rg -n tailscale", HOME));
        // A sibling sharing the root's text is not under it; logs stay watchdog evidence.
        assert!(!only_unindexed("rg -n x ~/automation/configs", HOME));
        assert!(!only_unindexed("grep -rn x ~/Library/Logs", HOME));
        assert!(!only_unindexed("find . -name '*.rs'", HOME));
        assert!(!only_unindexed("rg -n x web/node_modules_old", HOME));
    }

    #[test]
    fn a_node_modules_path_is_unindexed_at_any_depth() {
        // 0ddb7660…jsonl:876: the bundle of a dependency, named through `$F`.
        let command = "F=node_modules/.bun/@mastra+core@1.82.0+c78cfd2e1644d398/node_modules/@mastra/core/dist/create-durable-agent-f3IpD9mo.js; rg -n \"Error processing LLM stream\" $F | head; rg -n \"isRetryable|maxRetries\" $F | cut -c1-220";
        assert!(only_unindexed_in(command, "rg -n \"isRetryable|maxRetries\" $F ", HOME));
        assert!(only_unindexed("rg -n x agent/node_modules/@mastra/core/dist", HOME));
        assert!(only_unindexed("grep -rn x /Volumes/W/Mars_CRM/node_modules/.bun", HOME));
        // One indexed operand beside it keeps the sweep a search of the index.
        assert!(!only_unindexed("rg -n x node_modules/@mastra src", HOME));
    }
}
