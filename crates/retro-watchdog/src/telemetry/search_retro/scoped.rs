//! Whether an rg/grep after a NONE read only paths outside the search's `--path` scopes. A NONE
//! under `--path` says the name is absent from that scope only, so a match elsewhere does not prove
//! it wrong. Search-retro 812899216858410875382 (d5e72866…jsonl:193, 2026-11-10T08:39Z): after `cd
//! ~/automation`, `search "reconcile" --path crates/hook-audit` printed NONE, right then (the word
//! entered hook-audit in 00583bf at 08:53Z); at :220 `cd …/Mars_agent/.claude/skills; grep -l
//! "skill-hub-stop-reconcile" */SKILL.md | wc -l` counted skill files and read as a false NONE.
//!
//! Conservative: only rg/grep (a `find` names its start points differently); every search segment
//! must carry `--path` (each one counts; it repeats); each scope and each swept path must resolve to
//! an absolute path through a `cd`, `~/`, `/` or the shell directory the transcript recorded for
//! that command (Claude Code's `cwd`, since search-retro 816759169077413470494: 79a9b918…jsonl:1127
//! searched `crm_app` with `--path Mars_CRM/supabase` from Mars_agent and no `cd`, the only
//! exact matches sat under the `-- outside --path` label, and at :1145 the agent grepped that
//! suggested file, `Mars_CRM/rust/crates/mars-dev-tools/src/db/db_provision_roles.rs`; the meter
//! could not place either relative path and read the read as a false NONE), with no `..`, `$`,
//! backtick or bare `~`; each scope
//! must exist (local-search falls back to a parent directory otherwise). A sweep path inside a scope,
//! or an ancestor of one (`rg x .` from the repository root), still re-searches it; anything
//! unresolved keeps the sweep counted.
use super::{pattern, split};
use std::path::Path;

/// `outside_from` with no recorded working directory.
#[cfg(test)]
pub(super) fn outside(search: &str, sweep: &str, segment: &str, home: &str) -> bool {
    outside_from(search, sweep, segment, home, None, None)
}

/// True when `segment` of `sweep` reads only paths outside every `--path` scope of `search`;
/// `search_cwd` and `sweep_cwd` are the directories the transcript recorded for the two commands.
pub(super) fn outside_from(search: &str, sweep: &str, segment: &str, home: &str, search_cwd: Option<&str>, sweep_cwd: Option<&str>) -> bool {
    if !greps(segment) { return false; }
    let Some(scopes) = scopes(search, home, search_cwd) else { return false };
    // Absolute and assigned paths need no directory; a relative one or a bare sweep does.
    let dir = dir_before(sweep, segment, home, sweep_cwd);
    let files = pattern::files(segment);
    // A `$f` the same command assigns reads that path (`unindexed::expand_var`); search-retro
    // 81473143086819926510 (90b7190b…jsonl:119): `f=~/.claude-profiles/…/<id>.jsonl; grep -c '…' $f`
    // checked another session's transcript after a hit scoped to one skill directory.
    let paths: Option<Vec<String>> = if files.is_empty() { dir.map(|d| vec![d]) } else { files.iter().map(|f| resolve(dir.as_deref(), &super::unindexed::expand_var(sweep, f), home)).collect() };
    let Some(paths) = paths else { return false };
    paths.iter().all(|p| scopes.iter().all(|s| !related(&stem(p), s)))
}

/// True when `segment` of the later search `later` looks only outside every `--path` scope of the
/// hit `search` did: an rg/grep reading only such paths (`outside_from`), or a local-search whose
/// every `--path` lies outside them. The hit answered for its scope; the same name sought in another
/// tree is the next hop of the data flow, not distrust of the answer. Search-retro
/// 82104642038523697224 (24596541…jsonl:233 then :249): `search "case done(" --path Mars_CRM/ios`
/// printed `ChatEvent.swift:71 case "done": return .done(messageId: frame.messageId …)`, the agent
/// said it would now check that the server always sends `messageId`, and `search "type: 'done'"
/// --path Mars_CRM/agent/src` counted as a re-search. An unscoped or wider later search still counts.
pub(super) fn elsewhere_from(search: &str, later: &str, segment: &str, home: &str, search_cwd: Option<&str>, later_cwd: Option<&str>) -> bool {
    if greps(segment) {
        return outside_from(search, later, segment, home, search_cwd, later_cwd);
    }
    let words: Vec<&str> = segment.split_whitespace().collect();
    if !words.iter().any(|w| w.rsplit('/').next() == Some("local-search")) { return false; }
    let Some(scopes) = scopes(search, home, search_cwd) else { return false };
    let Some(dir) = dir_before(later, segment, home, later_cwd) else { return false };
    let Some(theirs) = segment_scopes(&words, Some(&dir), home) else { return false };
    theirs.iter().all(|t| scopes.iter().all(|s| !related(t, s)))
}

/// True when the segment's program is rg or grep.
fn greps(segment: &str) -> bool {
    let program = segment.split_whitespace().find(|w| !w.contains('=')).unwrap_or_default();
    matches!(program.rsplit('/').next().unwrap_or(program), "rg" | "grep" | "egrep" | "ugrep")
}

/// Every absolute `--path` of every local-search segment of `search`, or None when a segment has
/// none or any cannot be resolved to an existing file or directory.
fn scopes(search: &str, home: &str, cwd: Option<&str>) -> Option<Vec<String>> {
    let mut dir = start(cwd);
    let mut out = Vec::new();
    for (_, segment) in split::commands(search) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        if words.first() == Some(&"cd") {
            dir = words.get(1).and_then(|d| resolve(dir.as_deref(), d, home));
        } else if words.iter().any(|w| w.rsplit('/').next() == Some("local-search")) {
            out.extend(segment_scopes(&words, dir.as_deref(), home)?);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The absolute `--path` scopes of one local-search segment read from `dir`, or None when it has
/// none or one cannot be resolved to an existing file or directory.
fn segment_scopes(words: &[&str], dir: Option<&str>, home: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let raw = if *w == "--path" { *words.get(i + 1)? } else if let Some(v) = w.strip_prefix("--path=") { v } else { continue };
        let scope = resolve(dir, raw, home)?;
        if !Path::new(&scope).exists() { return None; }
        out.push(scope);
    }
    (!out.is_empty()).then_some(out)
}

/// The working directory `segment` runs in, from the `cd`s of `sweep` before it.
fn dir_before(sweep: &str, segment: &str, home: &str, cwd: Option<&str>) -> Option<String> {
    let mut dir = start(cwd);
    for (_, s) in split::commands(sweep) {
        if s.trim() == segment.trim() { return dir; }
        let words: Vec<&str> = s.split_whitespace().collect();
        if words.first() == Some(&"cd") {
            dir = words.get(1).and_then(|d| resolve(dir.as_deref(), d, home));
        }
    }
    None
}

/// The recorded working directory when it is absolute.
fn start(cwd: Option<&str>) -> Option<String> {
    cwd.filter(|d| d.starts_with('/')).map(|d| d.trim_end_matches('/').to_string())
}

/// An absolute path for `word` read from `base`, or None when it cannot be known from the text.
fn resolve(base: Option<&str>, word: &str, home: &str) -> Option<String> {
    let word = word.trim_matches(|c| c == '"' || c == '\'');
    let opaque = word.contains("..") || word.contains('$') || word.contains('`') || (word.starts_with('~') && !word.starts_with("~/"));
    if word.is_empty() || opaque { return None; }
    let abs = match word.strip_prefix("~/") {
        Some(rest) => format!("{home}/{rest}"),
        None if word.starts_with('/') => word.to_string(),
        None => format!("{}/{}", base?.trim_end_matches('/'), word.trim_start_matches("./")),
    };
    Some(abs.trim_end_matches(['/', '.']).to_string())
}

/// The literal directory part of a path with a glob (`/a/b/*/SKILL.md` → `/a/b`).
fn stem(path: &str) -> String {
    match path.find(['*', '?', '[', '{']) {
        Some(at) => path[..at].rsplit_once('/').map_or(String::new(), |(dir, _)| dir.to_string()),
        None => path.to_string(),
    }
}

/// True when one path lies inside the other.
fn related(path: &str, scope: &str) -> bool {
    let within = |a: &str, b: &str| a == b || a.starts_with(&format!("{b}/"));
    within(path, scope) || within(scope, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (String, String) {
        let root = std::env::temp_dir().join(format!("scoped-{}", std::process::id()));
        std::fs::create_dir_all(root.join("auto/crates/hook-audit")).unwrap();
        std::fs::create_dir_all(root.join("auto/crates/cpu-watchdog")).unwrap();
        std::fs::create_dir_all(root.join("agent/.claude/skills")).unwrap();
        let root = root.to_string_lossy().to_string();
        (root.clone(), format!("cd {root}/auto; local-search search \"reconcile\" --corpus code --path crates/hook-audit --lines | head"))
    }

    #[test]
    fn a_sweep_of_another_tree_does_not_refute_a_scoped_none() {
        // d5e72866…jsonl:220: skill files in Mars_agent, not hook-audit.
        let (root, search) = tree();
        let sweep = format!("cd {root}/agent/.claude/skills; grep -l \"skill-hub-stop-reconcile\" */SKILL.md | wc -l");
        assert!(outside(&search, &sweep, " grep -l \"skill-hub-stop-reconcile\" */SKILL.md ", "/h"));
    }

    #[test]
    fn a_sweep_inside_or_above_the_scope_still_counts() {
        let (root, search) = tree();
        assert!(!outside(&search, &format!("cd {root}/auto; rg -n reconcile crates/hook-audit/src"), "rg -n reconcile crates/hook-audit/src", "/h"));
        assert!(!outside(&search, &format!("cd {root}/auto; rg -n reconcile ."), "rg -n reconcile .", "/h"));
        assert!(!outside(&search, &format!("cd {root}/auto/crates; grep -rn reconcile"), "grep -rn reconcile", "/h"));
    }

    #[test]
    fn an_unscoped_or_unresolved_call_keeps_the_sweep() {
        let (root, _) = tree();
        let sweep = format!("cd {root}/agent; rg -n reconcile .");
        // No --path: a corpus-wide NONE.
        assert!(!outside(&format!("cd {root}/auto; local-search search reconcile --lines"), &sweep, "rg -n reconcile .", "/h"));
        // A relative --path with no leading cd, or one that does not exist.
        assert!(!outside("local-search search reconcile --path crates/hook-audit", &sweep, "rg -n reconcile .", "/h"));
        assert!(!outside(&format!("cd {root}/auto; local-search search reconcile --path crates/gone"), &sweep, "rg -n reconcile .", "/h"));
        // A sweep with relative paths and no cd.
        assert!(!outside(&format!("cd {root}/auto; local-search search reconcile --path crates/hook-audit"), "rg -n reconcile src", "rg -n reconcile src", "/h"));
    }

    #[test]
    fn find_dotdot_variables_and_bare_tilde_keep_the_sweep() {
        let (root, search) = tree();
        // A find names its start point, not a grep file operand.
        let find = format!("find {root}/auto/crates/hook-audit -name reconcile");
        assert!(!outside(&search, &format!("cd {root}/agent; {find}"), &find, "/h"));
        // `..` climbs back into the scope.
        assert!(!outside(&search, &format!("cd {root}/auto/crates/cpu-watchdog; grep -rn reconcile ../hook-audit"), "grep -rn reconcile ../hook-audit", "/h"));
        assert!(!outside(&search, &format!("cd {root}/auto/crates/hook-audit/src 2>/dev/null; cd ..; rg reconcile"), "rg reconcile", "/h"));
        // A variable or a bare `~` is not known from the text.
        assert!(!outside(&search, &format!("cd {root}/agent; rg reconcile $D"), "rg reconcile $D", "/h"));
        assert!(!outside(&search, "cd ~; rg -n reconcile automation", "rg -n reconcile automation", "/h"));
    }

    #[test]
    fn a_variable_the_command_assigns_places_the_sweep() {
        // 90b7190b…jsonl:57 then :119: a hit scoped to one skill directory, then a count in another
        // session's transcript named through `$f`.
        let (root, search) = tree();
        let grep = "grep -c 'zqxwvk' $f";
        let sweep = format!("f={root}/agent/x.jsonl; {grep}; grep -o 'k' $f");
        assert!(outside(&search, &sweep, grep, "/h"));
        let home = format!("f=~/t/x.jsonl; {grep}");
        assert!(outside(&search, &home, grep, &root));
        // Assigned inside the scope, it still re-searches it.
        let inside = format!("f={root}/auto/crates/hook-audit/src/a.rs; {grep}");
        assert!(!outside(&search, &inside, grep, "/h"));
    }

    #[test]
    fn the_recorded_working_directory_places_relative_paths() {
        // 79a9b918…jsonl:1127 then :1145, both from Mars_agent with no `cd`.
        let (root, _) = tree();
        std::fs::create_dir_all(format!("{root}/agent/Mars_CRM/supabase")).unwrap();
        let (cwd, search) = (format!("{root}/agent"), "~/.local/bin/local-search search \"crm_app\" --corpus code --path Mars_CRM/supabase --per-source 1 --limit 30 --lines 2>&1 | tail -15");
        let grep = "grep -n \"crm_app\\|plan\" Mars_CRM/rust/crates/mars-dev-tools/src/db/db_provision_roles.rs";
        let sweep = format!("{grep} | head -20");
        assert!(outside_from(search, &sweep, grep, "/h", Some(&cwd), Some(&cwd)));
        // Unknown directories keep the old reading: the sweep counts.
        assert!(!outside_from(search, &sweep, grep, "/h", None, None));
        // Inside the scope it still re-searches, and a `cd` in the command overrides the record.
        assert!(!outside_from(search, "grep -rn crm_app Mars_CRM/supabase", "grep -rn crm_app Mars_CRM/supabase", "/h", Some(&cwd), Some(&cwd)));
        let cd = format!("cd {root}/agent/Mars_CRM; grep -rn crm_app supabase/migrations");
        assert!(!outside_from(search, &cd, "grep -rn crm_app supabase/migrations", "/h", Some(&cwd), Some("/elsewhere")));
        // A scope that exists nowhere keeps the sweep (62c5bd72…jsonl:2491, `--path rust/crates/mars-core-wasm`).
        let gone = "local-search search plan_price --path rust/crates/mars-core-wasm";
        assert!(!outside_from(gone, "grep -n plan_price src/x.rs", "grep -n plan_price src/x.rs", "/h", Some(&cwd), Some(&cwd)));
    }

    #[test]
    fn a_scoped_search_of_another_tree_is_no_re_search_of_a_hit() {
        // Search-retro 82104642038523697224: 24596541…jsonl:233 (iOS) then :249 (agent server).
        let (root, _) = tree();
        for d in ["agent/Mars_CRM/ios/MarsCRM", "agent/Mars_CRM/agent/src"] { std::fs::create_dir_all(format!("{root}/{d}")).unwrap(); }
        let search = format!("cd {root}/agent; ~/.local/bin/local-search search \"case done(\" --corpus code --path Mars_CRM/ios 2>&1 | head; ~/.local/bin/local-search search \"\\\"done\\\"\" --corpus code --path Mars_CRM/ios/MarsCRM 2>&1 | head");
        let seg = "~/.local/bin/local-search search \"type: 'done'\" --corpus code --per-source 1 --limit 40 --lines --path Mars_CRM/agent/src 2>&1";
        let later = format!("cd {root}/agent; {seg} | head -20");
        // Segments as `split::commands` cuts them (`2>&1` splits at `&`), as `answered::researching` passes them.
        let piece = |cmd: &str| super::split::commands(cmd).into_iter().map(|(_, s)| s).find(|s| s.contains("local-search")).unwrap();
        assert!(elsewhere_from(&search, &later, &piece(&later), "/h", None, None));
        // The recorded working directory places a later search with no cd.
        let bare = format!("{seg} | head -20");
        assert!(elsewhere_from(&search, &bare, &piece(&bare), "/h", None, Some(&format!("{root}/agent"))));
        // Inside, above or without a scope it still re-searches (75f54766…jsonl:1040 then :1055 dropped --path).
        let inside = "local-search search done --path Mars_CRM/ios/MarsCRM/Features";
        std::fs::create_dir_all(format!("{root}/agent/Mars_CRM/ios/MarsCRM/Features")).unwrap();
        assert!(!elsewhere_from(&search, &format!("cd {root}/agent; {inside}"), inside, "/h", None, None));
        let above = "local-search search done --path Mars_CRM";
        assert!(!elsewhere_from(&search, &format!("cd {root}/agent; {above}"), above, "/h", None, None));
        let wide = "local-search search done --corpus code";
        assert!(!elsewhere_from(&search, &format!("cd {root}/agent; {wide}"), wide, "/h", None, None));
        // An unresolved directory or a missing scope keeps the re-search.
        assert!(!elsewhere_from(&search, &bare, &piece(&bare), "/h", None, None));
        let gone = "local-search search done --path Mars_CRM/gone";
        assert!(!elsewhere_from(&search, &format!("cd {root}/agent; {gone}"), gone, "/h", None, None));
        // An rg outside the scopes reads elsewhere too; a non-search segment is not judged here.
        let rg = "rg -n done Mars_CRM/agent/src";
        assert!(elsewhere_from(&search, &format!("cd {root}/agent; {rg}"), rg, "/h", None, None));
        assert!(!elsewhere_from(&search, &format!("cd {root}/agent; cat x"), "cat x", "/h", None, None));
    }

    #[test]
    fn every_path_of_a_search_is_a_scope() {
        let (root, _) = tree();
        let search = format!("cd {root}/auto; local-search search reconcile --path crates/cpu-watchdog --path=crates/hook-audit --lines");
        // Inside the second scope: still a re-search.
        assert!(!outside(&search, &format!("cd {root}/auto; rg reconcile crates/hook-audit"), "rg reconcile crates/hook-audit", "/h"));
        // Outside both.
        assert!(outside(&search, &format!("cd {root}/agent; rg reconcile .claude"), "rg reconcile .claude", "/h"));
    }
}
