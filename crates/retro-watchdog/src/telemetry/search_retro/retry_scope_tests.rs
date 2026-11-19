//! Files outside `--path` printed under a looser retry's label are suggestions, like those under
//! the scope's NONE (`returned.rs` `split_keys`). Search-retro 812899216858410875382
//! (d5e72866…jsonl:193, 2026-11-10T08:39Z): `search "hook-audit|hook_audit|skill-hub check|skill_hub"
//! --path crates/cpu-watchdog` printed the retry label, then `-- outside --path: 11 file(s) hold it
//! on one line --` and three `crates/hook-audit/…` lines. cpu-watchdog named hook-audit first in
//! 00583bf (08:53Z), which this same session wrote at 08:50Z, so the scope's NONE was right and the
//! agent moved on; the indented group label switched the retry off and its files counted as an
//! unadopted hit.
use super::returned::returned_or_used;

const COMMAND: &str = "cd ~/automation; grep -n \"KIND_ORDER\\|ADVISORY_KINDS\" -A4 crates/skill-hub/src/commands.rs | head -30; echo '---watchdog hook refs'; ~/.local/bin/local-search search \"hook-audit|hook_audit|skill-hub check|skill_hub\" --corpus code --path crates/cpu-watchdog --per-source 1 --limit 40 --lines 2>&1 | head -30; echo '---reconcile in hook-audit'; ~/.local/bin/local-search search \"reconcile\" --corpus code --path crates/hook-audit --per-source 1 --limit 20 --lines 2>&1 | head";

/// The transcript output, shortened to one line per group.
const OUTPUT: &str = "10:pub const ADVISORY_KINDS: &[&str] = &[\"dangling-citation\", \"stale-verified\"];
--
66:        print!(\"{}\", render_report(&findings, KIND_ORDER));
---watchdog hook refs
no chunk holds the exact pattern; chunks with all of its words:
  -- outside --path: 11 file(s) hold it on one line, first 3 --
crates/hook-audit/Cargo.toml:2:name = \"hook-audit\"
crates/hook-audit/src/main.rs:1://! `hook-audit`: proves the hooks of Claude Code, Codex and Antigravity are registered once, run,
crates/hook-audit/src/smoke.rs:50:        return json!({\"conversationId\": \"hook-audit-smoke\"});
  -- no line holds every word as written; nearest lines of chunks with the words --
crates/cpu-watchdog/src/telemetry/memory_dream/audit.rs:69:fn check_lessons(home: &Path, dir: &Path) -> Result<()> {
crates/cpu-watchdog/src/telemetry/memory_dream/transcript.rs:83-121  (words spread over the chunk)
---reconcile in hook-audit
NONE: no code chunk has every word. The code corpus holds Mars_agent and ~/automation (not its config/ or host/); build output is unindexed, so rg there.";

#[test]
fn files_outside_the_path_under_a_retry_label_are_suggestions() {
    // The agent took the scope's NONE: no hit of this call.
    assert!(returned_or_used(COMMAND, OUTPUT, |_| false).is_empty(), "{:?}", returned_or_used(COMMAND, OUTPUT, |_| false));
    // Opening a suggested file still adopts it.
    let opened = returned_or_used(COMMAND, OUTPUT, |f| f.iter().any(|k| k == "src/main.rs"));
    assert_eq!(opened, ["src/main.rs"]);
}

#[test]
fn an_outside_group_under_an_exact_line_keeps_its_files() {
    // An in-scope hit before the group: the outside files stay returned files, as before.
    let found = "crates/cpu-watchdog/src/telemetry/hook_watch.rs:37:const BIN: &str = \"/home/user/.local/bin/hook-audit\";
  -- outside --path: 11 more file(s) hold it, first crates/hook-audit/src/defects.rs:1 --";
    assert_eq!(returned_or_used("local-search search hook-audit --path crates/cpu-watchdog --lines", found, |_| false), ["telemetry/hook_watch.rs"]);
}

#[test]
fn an_oversize_count_line_changes_no_hit_or_retry_state() {
    // Search-retro 87971671342710251539: local-search 6cf9ab1 prints a count line above an oversized
    // `--lines` output. Under a retry label it must not end the retry, so the files stay suggestions.
    let count = "-- 377 lines in 86 files (76 KB, over the ~30 KB a caller may show whole: the end may be cut from view). Lines per alternative: cta_clicked 21 (first web/src/screens/Intro/landing/IntroLanding.tsx:50), showcase 203 (first .claude/skills/changelog/scripts/showcase.js:2); search a frequent one apart or narrow with --path --";
    let retry = format!("no chunk holds the exact pattern; chunks with all of its words:\n{count}\ncrates/hook-audit/src/main.rs:1://! `hook-audit`: proves the hooks\n");
    let cmd = "local-search search \"hook-audit\" --path crates/cpu-watchdog --lines";
    assert!(returned_or_used(cmd, &retry, |_| false).is_empty(), "{:?}", returned_or_used(cmd, &retry, |_| false));
    // Above exact lines it adds no file of its own: the first-line places are not returned files.
    let exact = format!("{count}\nweb/src/lib/analytics.ts:40:  track('cta_clicked')\n");
    assert_eq!(returned_or_used(cmd, &exact, |_| false), ["lib/analytics.ts"]);
}

/// local-search's translations marker (`output/same_line.rs`, search-retro 89259434869436960053)
/// under a looser retry's label stays inside the retry: its files are suggestions, and a view
/// listed after it is too.
#[test]
fn a_translations_marker_under_a_retry_label_keeps_the_retry_scope() {
    let out = "no chunk holds the exact pattern; chunks with all of its words:\nios/App/th.lproj/Localizable.strings:861:\"加载失败\" = \"โหลดไม่สำเร็จ\";\nios/App/th.lproj/Localizable.strings: … same keys in 1 more translation: ios/App/ko.lproj/Localizable.strings:861\nios/Features/MemoryEventsTab.swift:26:Button(\"加载失败\")";
    let cmd = "local-search search \"加载失败 重试\" --corpus code --lines";
    assert!(returned_or_used(cmd, out, |_| false).is_empty(), "{:?}", returned_or_used(cmd, out, |_| false));
    assert_eq!(returned_or_used(cmd, out, |f| f.iter().any(|k| k == "ko.lproj/Localizable.strings")), ["ko.lproj/Localizable.strings"]);
}

/// local-search's upstream note (`upstream.rs`, search-retro 86788129137822849179) names files of
/// commits the checkout lacks as `<upstream>:<path>` without a line number. They are no hit of the
/// call, under a NONE or above exact lines, and the note does not end a looser retry that follows.
#[test]
fn an_upstream_note_adds_no_hit_and_keeps_the_retry() {
    let note = "-- origin/mastra holds it in 6 file(s) this checkout lacks (71 commit(s) behind; the index reads this checkout, so it is not stale); read one with git show origin/mastra:<path> --
  origin/mastra:Mars_CRM/web/src/screens/invite/InviteFriends.tsx
  origin/mastra:Mars_CRM/web/src/screens/invite/InviteHistory.tsx
  … 1 more";
    let cmd = "local-search search \"invite-friends|inviteFriends|invite-history\" --corpus code --path Mars_CRM/web/src --lines";
    let none = format!("NONE: no code chunk has every word.\n{note}\n");
    assert!(returned_or_used(cmd, &none, |_| false).is_empty(), "{:?}", returned_or_used(cmd, &none, |_| false));
    let retry = format!("{note}\nno chunk holds the exact pattern; chunks with all of its words:\nMars_CRM/web/src/screens/Me.tsx:12:  invite\n");
    assert!(returned_or_used(cmd, &retry, |_| false).is_empty(), "{:?}", returned_or_used(cmd, &retry, |_| false));
    let absent = format!("-- no chunk in --path holds: InviteRewards, invite-rewards; the lines below match the other alternatives --\n{note}\nMars_CRM/web/src/screens/Intro/landing/howItWorks/howItWorksAutofill.ts:17:/** the referral screenshot */\n");
    assert_eq!(returned_or_used(cmd, &absent, |_| false), ["howItWorks/howItWorksAutofill.ts"]);
    // A note file the agent then reads is a suggestion it took (search-retro 813254723049142099088,
    // 51d32cbc…jsonl:56 → :118 `git show origin/mastra:…/invite/Invite.css`), above local lines or under a NONE.
    let read = |f: &[String]| f.iter().any(|k| k == "invite/InviteHistory.tsx");
    assert_eq!(returned_or_used(cmd, &absent, read), ["howItWorks/howItWorksAutofill.ts", "invite/InviteHistory.tsx"]);
    assert_eq!(returned_or_used(cmd, &none, read), ["invite/InviteHistory.tsx"]);
    // The `… 1 more` line keeps the note open; a later flush-left line ends it.
    let after = format!("{note}\nlocal/other.rs:3:  origin/mastra:x/y.rs\n");
    assert_eq!(returned_or_used(cmd, &after, |_| true), ["local/other.rs", "invite/InviteFriends.tsx", "invite/InviteHistory.tsx"]);
    // Piped through `cut -d: -f1` the note's lines are `origin/mastra`, still no file.
    let cut = "local-search search InviteRewards --corpus code --lines | cut -d: -f1 | sort -u";
    assert!(returned_or_used(cut, "  origin/mastra\n-- origin/mastra holds it in 6 file(s) this checkout lacks\n", |_| false).is_empty());
}

/// Search-retro 815124133498867247874 (7b135b8c…jsonl:567, 2026-11-16T07:58Z): after script-guard
/// refused `grep -rn "%% series" …/quota_economics/`, the agent ran the suggested search. It printed
/// NONE, the literal-absent note, then `legend_rules.rs` lines under the `-- outside --path` label.
/// The NONE was right (the report emitted no legend; the agent added one in mastra b399294d3e), yet
/// `own_output` cut the output at the note, below the NONE line, so the outside group read as exact
/// hits and the call counted as an unadopted hit.
#[test]
fn a_none_above_the_literal_note_keeps_outside_files_suggestions() {
    let cmd = "cd /path/to/workspace/Mars_CRM; ~/.local/bin/local-search search \"%% series\" --corpus code --per-source 1 --limit 20 --lines --path rust/crates/mars-ops/src/quota_economics/ 2>&1 | head -8";
    let out = "NONE: no code chunk has every word. The code corpus holds Mars_agent and ~/automation (not its config/ or host/); build output is unindexed, so rg there.
literal: 6 indexed chunks hold the words, but no line of their files has %% series (the index ignores punctuation; rg would read it literally).
  -- outside --path: 4 file(s) hold it on one line, first 3 --
rust/crates/mars-skill-tools/src/report/legend_rules.rs:3://! `%% series: <name>, <name>` with one non-empty name per series in source order; the page renders
rust/crates/mars-skill-tools/src/report/legend_rules.rs:44:        format!(\"{series} series with no legend; add `%% series: <name>, …` naming each line/bar in source order\")";
    assert!(returned_or_used(cmd, out, |_| false).is_empty(), "{:?}", returned_or_used(cmd, out, |_| false));
    assert_eq!(returned_or_used(cmd, out, |f| f.iter().any(|k| k == "report/legend_rules.rs")), ["report/legend_rules.rs"]);
    // Without a NONE above it the note still cuts another program's lines away (b21509cc…jsonl:5277).
    let other = "crates/a/Cargo.toml:9:reqwest = 1\nliteral: 2 indexed chunks hold the words, but no line of their files has multipart::Form (x).";
    assert!(returned_or_used("grep -n reqwest crates/*/Cargo.toml; local-search search \"multipart::Form\" --lines", other, |_| false).is_empty());
}
