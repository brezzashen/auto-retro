//! Whether the agent used a hit within the adoption window. Since 2026-11-05 (founder decision 1A on
//! search-retro 813111088844585114560) the window counts assistant turns, not tool calls: the tool
//! calls one assistant message issues in parallel are one turn. agent-a55443…jsonl:19 read the
//! hit's `wiki.json` lead at :89, five turns but eleven tool calls later.
use super::module_ref;
use super::steps::{Kind, Step};

/// Turns after a search inside which an adoption counts.
pub(super) const ADOPT_TURNS: usize = 8;
pub(super) const ADOPT_MS: f64 = 15.0 * 60_000.0;

/// The steps of `after` within `window_ms` of `at_ms` whose tool calls belong to the first `turns`
/// distinct assistant turns after the search's own message `own`; text and results between them stay
/// in. The calls that message ran beside the search stay in the window and fill no turn (founder
/// decision 1A on search-retro 813204256598053172535, 2026-11-10: the window is eight turns counted
/// from the next message). 8d13be13…jsonl:389 ran skill-recall beside its query and read the returned
/// `loading/scripts/probe.js` in the eighth later turn, which had counted as the ninth.
pub(super) fn window<'a>(after: &'a [Step], at_ms: f64, own: &str, turns: usize, window_ms: f64) -> &'a [Step] {
    let mut seen: Vec<&str> = Vec::new();
    let mut end = 0;
    for (i, s) in after.iter().enumerate() {
        if s.at_ms - at_ms > window_ms {
            break;
        }
        if matches!(s.kind, Kind::Shell | Kind::Tool) && s.turn != own && !seen.contains(&s.turn.as_str()) {
            if seen.len() == turns {
                break;
            }
            seen.push(&s.turn);
        }
        end = i + 1;
    }
    &after[..end]
}

/// A returned file named by a tool call in the next ADOPT_TURNS turns after the message `own` (path
/// tail) or by assistant text among them (file name), within ADOPT_MS of the search.
#[cfg(test)]
pub(super) fn adopts(after: &[Step], at_ms: f64, own: &str, files: &[String]) -> bool {
    adopts_with(after, at_ms, own, files, &[])
}

/// `adopts`, with the full paths the search printed (`returned::full_paths`) for the glob rule: a
/// two-component `files` key holds one directory, so `glob.rs`'s two-directory branch
/// (`mars-crm-commerce/src/stripe/*.rs` for `stripe/gateway.rs`) never fired on it. Search-retro
/// 88394746224259496385 (b5ce7520…jsonl:280 returned `…/src/stripe/gateway.rs:68-101`; :295 grepped
/// `rust/crates/mars-crm-commerce/src/stripe/*.rs` and read `form.rs` there; counted unadopted).
pub(super) fn adopts_with(after: &[Step], at_ms: f64, own: &str, files: &[String], full: &[String]) -> bool {
    let globbed = |text: &str, f: &str| super::glob::names(text, f) || full.iter().filter(|p| p.ends_with(&format!("/{f}"))).any(|p| super::glob::names(text, p) || names_from_cd_tail(text, p));
    window(after, at_ms, own, ADOPT_TURNS, ADOPT_MS).iter().any(|step| match step.kind {
        Kind::Shell | Kind::Tool => files.iter().any(|f| step.text.contains(f.as_str()) || names_from_cd(&step.text, f) || names_from_var(&step.text, f) || module_ref::names_module(&step.text, f) || super::member::names(&step.text, f) || names_session(&step.text, f) || (step.kind == Kind::Tool && super::skill_load::names(&step.text, f)) || (step.kind == Kind::Shell && (names_stem(&step.text, f) || super::subcommand::names(&step.text, f) || super::bare::names(&step.text, f) || globbed(&step.text, f))) || cites(&step.text, f)),
        Kind::Text => files.iter().any(|f| step.text.contains(f.rsplit('/').next().unwrap_or(f)) || names_session(&step.text, f)),
        Kind::Result => false,
    })
}

/// A history hit is a session transcript `<uuid>.jsonl`; agents name it by the 8-hex session id
/// prefix that session-status prints and takes (`session-status 007bc272`). Search-retro
/// 84502635138469566927: d7f1192d…jsonl:36 returned 007bc272 and 94c7b0b5, the agent opened
/// 007bc272 at :76 and cited both by id at :89, yet the hit counted unadopted.
pub(super) fn names_session(text: &str, file: &str) -> bool {
    let stem = file.rsplit('/').next().unwrap_or(file).strip_suffix(".jsonl").unwrap_or("");
    let uuid = stem.len() == 36 && stem.chars().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() });
    uuid && text.contains(&stem[..8])
}

/// True when a non-search tool call cites the file as `name.ext:<line>` without a directory, as a
/// report row or commit note does. Search-retro 813242367899389877458 (2bdf4cd8…jsonl:1052): the hit
/// printed `Shell/SidebarGlide.swift:179`, two turns later a `sed` wrote `SidebarGlide.swift:179`
/// into the report table (:1073), and the hit counted unadopted. A name joined to `/` is another path.
pub(super) fn cites(text: &str, file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    if !name.contains('.') {
        return false;
    }
    let joined = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.');
    let cited = |segment: &str| segment.match_indices(name).any(|(i, _)| {
        let rest = &segment[i + name.len()..];
        !segment[..i].chars().next_back().is_some_and(joined) && rest.strip_prefix(':').is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
    });
    // Each segment on its own: the report edit at :1073 ends with `; grep -n … report.md`.
    super::quality::commands(text).iter().any(|(_, segment)| !super::values::searches(segment) && cited(segment))
}

/// Shortest file stem (name before its first `.`) a command may name alone.
const MIN_STEM: usize = 6;

/// True when a non-search shell command names the file by its stem as a whole word, the stem a
/// kebab or snake name of MIN_STEM+ characters: `bun test seo-locales` runs `tests/seo-locales.test.ts`,
/// `cargo test release_watch` runs `release_watch.rs`. Search-retro 85383374938275326400
/// (64fde3e6…jsonl:341): the hit printed `web/tests/seo-locales.test.ts:3`, two turns later the agent
/// ran `bun test seo-locales …`, and the hit counted unadopted. A word joined to `/` or `.` is
/// another path, and a search for the stem is a re-search, not a use.
pub(super) fn names_stem(command: &str, file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    let stem = name.split('.').next().unwrap_or("");
    let shaped = stem.chars().count() >= MIN_STEM && stem.contains(['-', '_']) && stem.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if !shaped || name == stem || super::values::searches(command) {
        return false;
    }
    let joined = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.');
    command.match_indices(stem).any(|(i, _)| {
        let before = command[..i].chars().next_back();
        let after = command[i + stem.len()..].chars().next();
        !before.is_some_and(joined) && !after.is_some_and(joined)
    })
}

/// True when a shell line enters the file's directory and then names the file alone:
/// `cd …/canary; sed -n 1,9p preflight.rs` reads `canary/preflight.rs`. Search-retro
/// 815462549835379528073: 2 of 19 unadopted hits were read this way.
/// A glob covering the file inside a distinctive directory (`_` or `-` in its name) counts too:
/// search-retro 89714103305116763065 (fb3fa899…jsonl:1900) returned `fix_notice/cli.rs:37`, and the
/// next turn ran `cd …/fix_notice; grep -rn language *.rs` to read the hit's siblings. A bare `ls`
/// or a generic directory (`canary`) still does not count.
pub(super) fn names_from_cd(command: &str, file: &str) -> bool {
    let Some((parent, name)) = file.rsplit_once('/') else { return false };
    let (mut inside, mut dir) = (false, None::<String>);
    for (_, segment) in super::quality::commands(command) {
        let words: Vec<&str> = segment.split_whitespace().map(|w| w.trim_matches(|c| c == '"' || c == '\'')).collect();
        if words.first() == Some(&"cd") {
            inside = words.get(1).is_some_and(|d| d.trim_end_matches('/').rsplit('/').next() == Some(parent));
            dir = words.get(1).map(|d| d.trim_end_matches('/').to_string());
        } else if inside && (words.contains(&name) || (distinctive_dir(parent) && words.iter().any(|w| covers(w, name)))) {
            return true;
        } else if dir.as_deref().is_some_and(|d| words.iter().any(|w| relative_names(d, w, file))) {
            return true;
        }
    }
    false
}

/// True when `word`, a `./` or `../` path read from directory `dir`, resolves to a path ending in
/// `file`. Search-retro 812798351439866126096 (0fb18ebc…jsonl:1385) returned
/// `web/src/lib/composerAttachments.ts`; three turns later (:1408) the agent ran
/// `cd …/web/src/lib/api && … grep -n … ../composerAttachments.ts`, and the hit counted unadopted.
fn relative_names(dir: &str, word: &str, file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    if !(word.starts_with("../") || word.starts_with("./")) || !word.ends_with(name) {
        return false;
    }
    let mut parts: Vec<&str> = dir.split('/').collect();
    for part in word.split('/') {
        match part {
            "." | "" => {}
            ".." => { if parts.len() > 1 { parts.pop(); } else { return false; } }
            p => parts.push(p),
        }
    }
    parts.join("/").ends_with(&format!("/{file}"))
}

/// `names_from_cd` for a full printed path whose parent is generic but whose grandparent is
/// distinctive: entering `…/<grand>/<parent>` and globbing it (`*`, `*.md`) reads the hit's
/// siblings, as the two-directory glob rule of `glob.rs` already counts. Search-retro
/// 811835240271232588103 (62e0e2e2…jsonl:624 returned `verify-mars-crm/features/README.md:39`
/// "Feature file contract"; :632 ran `cd …/verify-mars-crm/features && ls; grep -l … *` and took
/// the owning feature file from it; counted unadopted because `features` alone is generic).
pub(super) fn names_from_cd_tail(command: &str, full: &str) -> bool {
    let mut parts = full.rsplit('/');
    let (Some(name), Some(parent), Some(grand)) = (parts.next(), parts.next(), parts.next()) else { return false };
    if !distinctive_dir(grand) {
        return false;
    }
    let tail = format!("{grand}/{parent}");
    let mut inside = false;
    for (_, segment) in super::quality::commands(command) {
        let words: Vec<&str> = segment.split_whitespace().map(|w| w.trim_matches(|c| c == '"' || c == '\'')).collect();
        if words.first() == Some(&"cd") {
            inside = words.get(1).is_some_and(|d| { let d = d.trim_end_matches('/'); d == tail || d.ends_with(&format!("/{tail}")) });
        } else if inside && words.iter().any(|w| covers(w, name)) {
            return true;
        }
    }
    false
}

/// A directory name an agent would not enter by chance: snake or kebab, six characters or more.
fn distinctive_dir(dir: &str) -> bool {
    dir.len() >= 6 && dir.contains(['_', '-'])
}

/// A one-level glob `*.ext` (or `*`) that matches the file name.
fn covers(word: &str, name: &str) -> bool {
    word.strip_prefix('*').is_some_and(|tail| !tail.contains('/') && !tail.contains('*') && name.ends_with(tail))
}

/// True when a shell line sets a variable to the file's directory and then names the file through
/// it: `F=…/seo_competitor; sed -n 1,80p $F/monid.rs` reads `seo_competitor/monid.rs`. Search-retro
/// 812828117540949169408: 2c00c97b…jsonl:378 returned `seo_competitor/monid.rs:21`, the agent read
/// it this way at :392 three seconds later, and the hit counted unadopted.
pub(super) fn names_from_var(command: &str, file: &str) -> bool {
    let Some((parent, name)) = file.rsplit_once('/') else { return false };
    let mut vars: Vec<String> = Vec::new();
    for (_, segment) in super::quality::commands(command) {
        for word in segment.split_whitespace() {
            let word = word.trim_matches(|c| c == '"' || c == '\'');
            if let Some((var, value)) = word.split_once('=') {
                let value = value.trim_matches(|c| c == '"' || c == '\'').trim_end_matches('/');
                let shaped = !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if shaped && value.rsplit('/').next() == Some(parent) { vars.push(var.to_string()); }
            } else if vars.iter().any(|v| word.contains(&format!("${v}/{name}")) || word.contains(&format!("${{{v}}}/{name}"))) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(at_ms: f64, turn: &str, text: &str) -> Step {
        Step { at_ms, id: format!("{turn}{at_ms}"), kind: Kind::Shell, text: text.into(), line: 1, turn: turn.into() }
    }

    #[test]
    fn a_transcript_hit_is_named_by_its_session_id_prefix() {
        // d7f1192d…jsonl:36 -> :76 `session-status 007bc272 --answer`.
        let hit = "-Volumes-MarsWork-Workspaces-Mars-agent/007bc272-1209-4dbb-914c-08f804d2054a.jsonl".to_string();
        assert!(adopts(&[tool(1.0, "a", "~/.local/bin/session-status 007bc272 --answer")], 0.0, "", &[hit.clone()]));
        assert!(!adopts(&[tool(1.0, "a", "~/.local/bin/session-status 94c7b0b5")], 0.0, "", &[hit]));
        // A non-transcript file keeps the path rule: its first 8 characters are no id.
        assert!(!names_session("cat abcdefgh", "src/abcdefgh-1234.jsonl"));
    }

    #[test]
    fn a_test_runner_names_the_file_by_its_stem() {
        // 64fde3e6…jsonl:341 → `bun test seo-locales seo-metadata` two turns later.
        let hit = "tests/seo-locales.test.ts".to_string();
        let after = [tool(1.0, "a", "git show -s 906a1adfe7"), tool(2.0, "b", "cd Mars_CRM/web && bun test seo-locales seo-metadata 2>&1 | tail -8")];
        assert!(adopts(&after, 0.0, "", &[hit.clone()]));
        // Part of a longer name, another path, a search, or a plain one-word stem is no use.
        assert!(!names_stem("bun test seo-locales-v2", &hit));
        assert!(!names_stem("cat other/seo-locales.ts", &hit));
        assert!(!names_stem("rg -n seo-locales web/tests", &hit));
        assert!(!names_stem("bun test locales", "tests/locales.test.ts"));
        assert!(names_stem("cargo test -p cpu-watchdog release_watch", "src/release_watch.rs"));
    }

    #[test]
    fn a_name_and_line_citation_uses_the_file() {
        // 2bdf4cd8…jsonl:1052 → :1073: the hit's line written into a report table.
        let hit = "Shell/SidebarGlide.swift".to_string();
        let cmd = "cd /r/tab-c; sed -i '' 's#有 `sidebar-*`#有（`SidebarGlide.swift:179`）#' report.md; grep -n \"SidebarGlide\" report.md";
        assert!(adopts(&[tool(1.0, "a", "ls"), tool(2.0, "b", cmd)], 0.0, "", &[hit.clone()]));
        // No line number, another directory's file, a longer name or a search for it is no use.
        assert!(!cites("echo SidebarGlide.swift", &hit));
        assert!(!cites("cat Other/SidebarGlide.swift:1", &hit));
        assert!(!cites("echo MySidebarGlide.swift:4", &hit));
        assert!(!cites("rg -n 'SidebarGlide.swift:179' docs", &hit));
    }

    #[test]
    fn a_cd_into_a_distinctive_two_directory_tail_and_a_glob_names_the_file() {
        // 62e0e2e2…jsonl:624 -> :632.
        let full = "/path/to/workspace/.claude/skills/verify-mars-crm/features/README.md";
        let cmd = "cd /Volumes/MarsWork/Workspaces/Mars_agent/.claude/skills/verify-mars-crm/features && ls | head -80; grep -l \"turnSubjects\\|conversation_person\" * 2>/dev/null";
        assert!(names_from_cd_tail(cmd, full));
        let after = [tool(1.0, "a", cmd)];
        assert!(adopts_with(&after, 0.0, "", &["features/README.md".to_string()], &[full.to_string()]));
        // The generic parent alone, a bare ls, a generic grandparent or another tree is no use.
        assert!(!names_from_cd(cmd, "features/README.md"));
        assert!(!names_from_cd_tail("cd /r/verify-mars-crm/features && ls", full));
        assert!(!names_from_cd_tail("cd /r/skills/features; grep -l x *", "/r/skills/features/README.md"));
        assert!(!names_from_cd_tail("cd /r/other-skill/features; grep -l x *", full));
    }

    #[test]
    fn a_variable_holding_the_directory_names_the_file() {
        // 2c00c97b…jsonl:378 -> :392.
        let hit = "seo_competitor/monid.rs".to_string();
        let cmd = "cd /r; F=Mars_CRM/rust/crates/mars-skill-tools/src/skills/seo_competitor; sed -n 1,80p $F/monid.rs; grep -rn x $F/*.rs";
        assert!(adopts(&[tool(1.0, "a", cmd)], 0.0, "", &[hit.clone()]));
        assert!(names_from_var("D=\"a/seo_competitor/\"; cat ${D}/monid.rs", &hit));
        // Another directory, or a variable never set to it, is no use.
        assert!(!names_from_var("F=src/other; cat $F/monid.rs", &hit));
        assert!(!names_from_var("cat $F/monid.rs", &hit));
    }

    #[test]
    fn calls_beside_the_search_fill_no_turn() {
        // 8d13be13…jsonl:389: skill-recall ran in the query's own message; the read came eight turns later.
        let mut after = vec![tool(1.0, "own", "~/.local/bin/skill-recall query \"web frontend change delivery\"")];
        after.extend((0..7).map(|i| tool(2.0 + i as f64, &format!("t{i}"), "ls")));
        after.push(tool(20.0, "t7", "cat .claude/skills/loading/scripts/probe.js"));
        let hit = ["scripts/probe.js".to_string()];
        assert!(adopts(&after, 0.0, "own", &hit));
        // Counted as a turn (the rule before the decision), the read is the ninth and falls outside.
        assert!(!adopts(&after, 0.0, "", &hit));
        // A ninth later turn stays outside.
        after.insert(8, tool(19.0, "t6b", "ls"));
        assert!(!adopts(&after, 0.0, "own", &hit));
    }

    #[test]
    fn a_skill_tool_call_loads_a_returned_skill_md() {
        // 1d859b8b…jsonl:1207 -> :1209 `Skill {"skill":"swipe-arbitration"}`.
        let load = || Step { at_ms: 1.0, id: "s".into(), kind: Kind::Tool, text: r#"{"skill":"swipe-arbitration"}"#.into(), line: 1, turn: "a".into() };
        assert!(adopts(&[load()], 0.0, "", &["swipe-arbitration/SKILL.md".into()]));
        assert!(!adopts(&[load()], 0.0, "", &["components/SKILL.md".into()]));
    }

    #[test]
    fn parallel_calls_of_one_turn_count_once() {
        // agent-a55443…jsonl:19 → :89: ten reads in four turns, then the lead in the fifth.
        let mut after: Vec<Step> = (0..10).map(|i| tool(1.0 + i as f64, &format!("t{}", i / 3), "cat storyline.md")).collect();
        after.push(tool(20.0, "t9", "jq -r . web/src/data/wiki.json"));
        assert!(adopts(&after, 0.0, "", &["data/wiki.json".into()]));
        let serial: Vec<Step> = (0..8).map(|i| tool(1.0 + i as f64, &format!("s{i}"), "ls")).chain([tool(20.0, "s9", "cat data/wiki.json")]).collect();
        assert!(!adopts(&serial, 0.0, "", &["data/wiki.json".into()]));
        assert!(!adopts(&[tool(1e6, "a", "cat data/wiki.json")], 0.0, "", &["data/wiki.json".into()]));
    }

    #[test]
    fn a_glob_over_the_returned_files_two_directories_reads_it() {
        // b5ce7520…jsonl:280 returned this file; :295 grepped its directory through a glob.
        let out = "  /path/to/workspace/Mars_CRM/rust/crates/mars-crm-commerce/src/stripe/gateway.rs:68-101  [code/block] pub struct CheckoutSessionInput";
        let full = super::super::returned::full_paths(out);
        let key = vec!["stripe/gateway.rs".to_string()];
        let cmd = r#"cd /V/Mars_CRM && ls rust/crates/mars-crm-commerce/src/stripe/; grep -n "locale\\|currency" rust/crates/mars-crm-commerce/src/stripe/*.rs | head -40"#;
        assert!(adopts_with(&[tool(1.0, "a", cmd)], 0.0, "", &key, &full));
        // The two-component key alone holds one directory, so the glob needed the full path.
        assert!(!adopts(&[tool(1.0, "a", cmd)], 0.0, "", &key));
        // Another package's `src/stripe` or a bare `stripe/*.rs` is no read of it.
        assert!(!adopts_with(&[tool(1.0, "a", "grep -n x rust/crates/other/lib/stripe/*.rs")], 0.0, "", &key, &full));
        assert!(!adopts_with(&[tool(1.0, "a", "grep -n x stripe/*.rs")], 0.0, "", &key, &full));
    }
}
