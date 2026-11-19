use super::*;
use super::super::adopt::names_from_cd;

fn sweeps_for(command: &str, names: &[String]) -> bool {
    sweeps_for_in(command, names, "", "")
}

fn sweeps_for_in(command: &str, names: &[String], home: &str, search: &str) -> bool {
    sweeps_for_at(command, names, home, search, (None, None))
}

#[test]
fn returned_files_keep_the_last_two_components_of_printed_paths() {
    let out = "note: --path x is not under y\nMars_CRM/ios/MarsCRM/Features/People/PersonDetailScreen.swift:5:/// panels\n  /Users/b/Mars_agent/web/src/App.tsx:1-49  [code/block]\n== heading\nNONE: no code chunk has every word.";
    assert_eq!(returned_files(out), ["People/PersonDetailScreen.swift", "src/App.tsx"]);
    assert!(returned_files("NONE: no code chunk has every word.").is_empty());
}

#[test]
fn near_lines_are_not_hits_and_escaped_quotes_keep_their_names() {
    // Search-retro 84763104493312366879: 17a40a19…jsonl:1530 printed only near lines for record.example.com.
    let near = "no chunk holds the exact pattern; chunks with all of its words:\nMars_CRM/web/.storybook/manager.ts:1-12  (words spread over the chunk)";
    assert!(returned_files(near).is_empty());
    let mixed = format!("{near}\n  -- docs: rules and notes naming the same words --\n.claude/skills/x/references/appstore-rejection.md:50:| Pro $19.99 |");
    assert_eq!(returned_files(&mixed), ["references/appstore-rejection.md"]);
    // Search-retro 816506381674351735418: every line under a looser retry's label is a near line,
    // up to the next program's output (c5ce4338…jsonl:105 printed two such searches around `echo ---`).
    let relaxed = "no chunk holds the exact pattern; chunks with all of its words:\n/h/.agents/skills/general-video/SKILL.md: | Any media asset\n.claude/skills/x/features/posters.md:26:- a poster with its words\nMars_CRM/ios/docs/responsibility.md: - cards\n---\nno chunk holds the exact pattern; chunks naming AuthGlass:\nios/Auth/AuthStateIcon.swift:10:/// glass";
    assert!(returned_files(relaxed).is_empty());
    assert_eq!(returned_files(&format!("{relaxed}\n---\nweb/src/a.ts:3:const a = 1")), ["src/a.ts"]);
    // An indented locator after the label is a hit: a following `query` (b9b85bcd…jsonl:1867) or a
    // history retry the agent opened (0abfc34d…jsonl:303).
    assert_eq!(returned_files("no chunk holds the exact pattern; chunks with all of its words:\nios/C/A.swift:27-64  (words spread over the chunk)\n  /h/ios/C/ComposerDraftArchive.swift:28-55  [code/block] x"), ["C/ComposerDraftArchive.swift"]);
    // A history retry prints its near lines indented: still near lines, then `---` ends them
    // (search-retro 89953860612474017944, a02094a4…jsonl:64 `singleorb`).
    let history = "no chunk holds the exact pattern; chunks with all of its words:\n  /h/p/219881fe.jsonl:8  [history/user]\n    user: energy 1 and `MarsCRMTests/StrandsOrbTests.swift` pins it\n    - commits touched Strands.tsx\n---\nweb/src/a.ts:3:const a = 1";
    assert_eq!(returned_files(history), ["src/a.ts"]);
    assert!(super::returned::returned_or_used("local-search search x --corpus history", history.split("\n---").next().unwrap(), |_| false).is_empty());
    // A near line the agent then opened is a hit it used (9b1a1318…jsonl:478 read main.rs:51).
    let naming = "literal: 5 indexed chunks hold the words\nno chunk holds the exact pattern; chunks naming LocalSearch:\n/a/crates/local-search/src/main.rs:51:    name: \"LocalSearch\",";
    assert_eq!(super::returned::returned_or_used("local-search search x --lines", naming, |f| f.iter().any(|f| f == "src/main.rs")), ["src/main.rs"]);
    assert!(super::returned::returned_or_used("local-search search x --lines", naming, |_| false).is_empty());
    // Two searches in one command: the first one's exact line stays a hit; the second one's near
    // line joins it only when opened (since search-retro 818364334650240656506).
    let two = "a/b.rs:1:x\nno chunk holds the exact pattern; x:\nc/d.rs:2:y";
    assert_eq!(super::returned::returned_or_used("local-search search a --lines; local-search search x --lines", two, |_| false), ["a/b.rs"]);
    assert_eq!(super::returned::returned_or_used("local-search search a --lines; local-search search x --lines", two, |_| true), ["a/b.rs", "c/d.rs"]);
    // One search: lines above its no-exact label came from another program (b21509cc…jsonl:5277).
    assert_eq!(super::returned::returned_or_used("grep -n x a/b.rs; local-search search x --lines", "a/b.rs:1:x\nno chunk holds the exact pattern; x:\nc/d.rs:2:y", |f| f.iter().any(|f| f == "c/d.rs")), ["c/d.rs"]);
    let grep_first = "crates/mars-crm-comms/Cargo.toml:27:reqwest = { workspace = true }\nliteral: 8 indexed chunks hold the words, but no line of their files has multipart::Form (the index ignores punctuation; rg would read it literally).\nno chunk holds the exact pattern; chunks with all of its words:\ncrates/x/src/figma_glyphs.rs:94:fn multipart_body(symbol: &str) {";
    let cmd = r#"grep -n "reqwest" crates/mars-crm-comms/Cargo.toml; ~/.local/bin/local-search search "multipart::Form" --corpus code --lines 2>&1 | grep -v "^  " | head -5"#;
    assert!(super::returned::returned_or_used(cmd, grep_first, |_| false).is_empty());
    assert_eq!(returned_files("lrwxr-xr-x link\nno chunk holds the exact pattern; chunks with all of its words:\nagent/x/y.ts:19:fn"), Vec::<String>::new());
    // Search-retro 87508011737703381146: `--lines` near lines with no exact line before them are a
    // NONE with suggestions (b0bbe2c0…jsonl:111); after an exact line they stay with the hit.
    let spread = "  -- no line holds every word as written; nearest lines of chunks with the words --\nlib/informationPlacements.ts:28:export function informationGroupTone(key: string) {\nlib/informationPlacements.ts:34:export function refreshInformationPlacements(id) {";
    assert!(returned_files(spread).is_empty());
    assert!(super::returned::returned_or_used("local-search search x --lines", spread, |_| false).is_empty());
    assert_eq!(super::returned::returned_or_used("local-search search x --lines", spread, |f| f.iter().any(|f| f == "lib/informationPlacements.ts")), ["lib/informationPlacements.ts"]);
    assert_eq!(returned_files(&format!("{spread}\n  -- outside --path: 2 more file(s) hold it, first a/b.ts:3 --\n  -- docs: rules --\ndocs/x/notes.md:4:words")), ["x/notes.md"]);
    assert_eq!(returned_files(&format!("web/a.ts:3:export function f\n{spread}")), ["web/a.ts", "lib/informationPlacements.ts"]);
    // be7a95f1…jsonl:136: escaped quotes inside a double-quoted query.
    assert_eq!(invocation(r#"~/.local/bin/local-search search "\"/me/subscribe\"|/subscribe\"" --corpus code"#).unwrap(), ["subscribe", "subscribe"]);
    assert_eq!(invocation("local-search search 'a\\b_name' --lines").unwrap(), ["b_name"]);
}

#[test]
fn indented_history_near_lines_stay_near() {
    // Search-retro 811147635907382047180, rollout-2026-11-08T12-32-55-01a1b96e…jsonl:258: a history
    // `--lines` search for an SSH fingerprint printed no exact line, only indented near lines.
    let out = "-- more match than --limit 20: this list is cut; rerun with --limit 100 for every match --\n  -- no line holds every word as written; nearest lines of chunks with the words --\n  /h/.codex/sessions/2026/10/12/rollout-2026-10-12T19-18-59-01a12fd6.jsonl:112  [history/turn]\n    看一下surge的服务器 aws 和zorocloud · 2026-10-12\n    2. ZoroCloud：执行 `ssh zorocloud`。\n  /h/.claude/projects/p/bb40f05e.jsonl:416  [history/assistant]\n    assistant: hy2 节点 zorocloudSG";
    let cmd = "~/.local/bin/local-search search '7pzyN|rh3DDwG|zorocloud.*host.key' --corpus history --per-source 1 --limit 20 --lines";
    assert!(super::returned::returned_or_used(cmd, out, |_| false).is_empty());
    assert_eq!(super::returned::returned_or_used(cmd, out, |f| f.iter().any(|f| f.ends_with("bb40f05e.jsonl"))), ["p/bb40f05e.jsonl"]);
    // Flush-left code near lines keep their rule, and an exact line before the label stays a hit.
    let code = "  -- no line holds every word as written; nearest lines of chunks with the words --\nlib/a.ts:28:export function f() {";
    assert!(super::returned::returned_or_used("local-search search x --lines", code, |_| false).is_empty());
    assert_eq!(returned_files(&format!("web/b.ts:3:x\n{out}")).first().map(String::as_str), Some("web/b.ts"));
}

#[test]
fn only_a_lone_local_search_command_is_timed() {
    assert!(is_local_search("cd /r && ~/.local/bin/local-search search \"x\" --corpus code | head"));
    assert!(!is_local_search("local-search sync"));
    assert!(single_command("cd /r && ~/.local/bin/local-search search \"x\" --lines 2>&1 | head -5"));
    assert!(single_command("local-search query \"how\" --corpus docs"));
    assert!(!single_command("cd /r && local-search search a; sed -n 1,5p f.rs"));
    assert!(!single_command("local-search search a | head; local-search search b"));
}

#[test]
fn calls_count_hits_adoption_fallback_and_wall_time() {
    let home = crate::telemetry::test_home("search-quality");
    let dir = home.join(".claude/projects/-r/11111111-1111-1111-1111-111111111111/subagents");
    std::fs::create_dir_all(&dir).unwrap();
    let at = |s: u32| format!("2026-11-02T10:00:{s:02}.000Z");
    let call = |s: u32, id: &str, cmd: &str| json!({"timestamp": at(s), "message": {"content": [{"type": "tool_use", "id": id, "input": {"command": cmd}}]}});
    let read = |s: u32, id: &str, file: &str| json!({"timestamp": at(s), "message": {"content": [{"type": "tool_use", "id": id, "input": {"file_path": file}}]}});
    let result = |s: u32, id: &str, text: &str| json!({"timestamp": at(s), "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}});
    let edit = |s: u32, id: &str| json!({"timestamp": at(s), "message": {"content": [{"type": "tool_use", "id": id, "input": {"file_path": "/r/a.rs", "new_string": "run local-search search x"}}]}});
    let say = |s: u32, text: &str| json!({"timestamp": at(s), "type": "assistant", "message": {"content": [{"type": "text", "text": text}]}});
    let rows = [
        call(1, "a", "local-search search \"fetchDossier\" --corpus code --lines"),
        result(2, "a", "web/src/lib/dossier.ts:12:export function fetchDossier"),
        read(3, "b", "/r/web/src/lib/dossier.ts"),
        result(4, "b", "ok"),
        call(5, "c", "cd /r && local-search search \"nothing\" --corpus code"),
        result(6, "c", "NONE: no code chunk has every word."),
        call(7, "d", "LOCAL_SEARCH_MISS=1 rg -n nothing ."),
        result(8, "d", ""),
        call(9, "e", "local-search query \"how releases queue\" --corpus docs; ls"),
        result(10, "e", "  /r/docs/release/queue.md:1-30  [docs/section]"),
        edit(11, "f"),
        call(12, "g", "LOCAL_SEARCH_NO_LOG=1 local-search search releases --corpus code"),
        result(13, "g", "NONE"),
        call(14, "h", "local-search search \"lastConversationAt\" --corpus code --lines"),
        result(15, "h", "ios/Relationships/RelationshipListModels.swift:11:    var lastConversationAt: String?"),
        say(16, "The field lives in `RelationshipListModels.swift:11`."),
        call(17, "i", "local-search search \"useThing\" --corpus code --lines"),
        result(18, "i", "NONE: no code chunk has every word."),
        call(19, "j", "rg -n useThing web"),
        result(20, "j", "web/x.ts:4:useThing()"),
        // An edit script whose heredoc mentions local-search is no call; a call captured by $(…) is unobserved.
        call(21, "k", "cat > /t/e.js <<'EOF'\nrun(`local-search query \"x y\"`)\nEOF\nnode /t/e.js"),
        result(22, "k", "ok"),
        call(23, "l", "for f in $(local-search search \"Foo {\" --lines | cut -d: -f1); do echo $f; done"),
        result(24, "l", ""),
    ];
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    std::fs::write(dir.join("agent-x.jsonl"), &text).unwrap();
    // A resumed copy under another profile repeats the same call ids and must not count twice.
    let copy = home.join(".claude-profiles/default/projects/-r");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join("11111111-1111-1111-1111-111111111111.jsonl"), &text).unwrap();
    std::fs::create_dir_all(home.join(".local/share/local-search")).unwrap();
    std::fs::write(home.join(".local/share/local-search/queries.jsonl"), [
        json!({"ts_ms": 1793613601000.0, "sub": "search", "hits": 3, "search_ms": 20.0}),
        json!({"ts_ms": 1793613602000.0, "sub": "search", "hits": 0, "search_ms": 40.0}),
        json!({"ts_ms": 1793613602500.0, "sub": "search", "hits": 2, "search_ms": 900.0, "query": "slow words"}),
        json!({"ts_ms": 1793613603000.0, "sub": "query", "hits": 5, "embed_ms": 80.0, "search_ms": 20.0, "caller": "cpu-watchdog retro-recall"}),
    ].iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
    let from = crate::cause_tree::attribute::parse_time(&at(0)).unwrap();
    let q = measure(&home, from, from + 60_000.0);
    let c = &q["calls"];
    assert_eq!((c["total"].as_u64(), c["hits"].as_u64(), c["adopted"].as_u64()), (Some(5), Some(3), Some(2)));
    assert_eq!((c["no_hit"].as_u64(), c["fallback_after_no_hit"].as_u64(), c["false_none"].as_u64(), c["answer_rate"].as_f64()), (Some(2), Some(2), Some(1), Some(0.8)));
    assert_eq!((c["adoption_rate"].as_f64(), c["fallback_rate"].as_f64()), (Some(0.666667), Some(1.0)));
    assert_eq!((c["wall_ms"]["n"].as_u64(), c["wall_ms"]["p50"].as_f64()), (Some(4), Some(1000.0)));
    assert_eq!(c["failures"]["unadopted"].as_array().map(Vec::len), Some(1));
    // "e" is searched again by "g" (same name `releases`), so its printed lines do not count as an answer.
    assert_eq!(c["adopted_by_lines"].as_u64(), Some(0));
    assert_eq!(c["unobserved"].as_u64(), Some(1));
    let e = &q["engine"];
    assert_eq!((e["search"]["calls"].as_u64(), e["search"]["hit_rate"].as_f64(), e["search"]["p95_ms"].as_f64()), (Some(3), Some(0.666667), Some(900.0)));
    assert_eq!((e["speed_rate"].as_f64(), e["slow_calls"].as_u64(), e["slow"][0]["query"].as_str()), (Some(0.666667), Some(1), Some("slow words")));
    assert_eq!(c["failures"]["false_none"].as_array().map(Vec::len), Some(1));
    let g = &q["goal"];
    assert_eq!((g["answer_rate"].as_f64(), g["met"].as_bool(), g["failures"]["unadopted"].as_u64(), g["nines"]["answer"].as_f64()), (Some(0.8), Some(false), Some(1), Some(0.7)));
    assert_eq!(e["query"]["calls"].as_u64(), Some(0));
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn only_a_real_invocation_counts_and_its_names_tie_a_fallback_to_it() {
    assert_eq!(invocation("cd /r && ~/.local/bin/local-search search \"fetchDossier|useThing\" --corpus code"), Some(vec!["fetchDossier".to_string(), "useThing".to_string()]));
    assert_eq!(invocation("local-search --db /x.db query 'how releases queue' --corpus docs"), Some(vec!["releases".to_string(), "queue".to_string()]));
    assert_eq!(invocation("local-search sync; ~/.local/bin/local-search search api --corpus code"), Some(vec!["api".to_string()]));
    // An edit or a heredoc that only mentions local-search is not a search.
    assert_eq!(invocation("perl -0pi -e 's/a/run local-search search x/' f.rs"), None);
    assert_eq!(invocation("cat > m.txt <<'EOF'\nthe local-search search command\nEOF"), None);
    assert_eq!(invocation("LOCAL_SEARCH_NO_LOG=1 local-search search x"), None);
    // A kebab-case slug is one name (f36acff7…jsonl:85).
    assert_eq!(invocation("local-search search \"KEYWORD_PAGE_SLUGS|personal-crm\" --corpus code"), Some(vec!["KEYWORD_PAGE_SLUGS".to_string(), "personal-crm".to_string()]));
    assert_eq!(invocation("local-search search \"--lock-status\""), Some(vec!["lock-status".to_string()]));
}

#[test]
fn a_fallback_sweep_is_an_unpiped_search_naming_the_word_itself() {
    let names = vec!["parityWatch".to_string()];
    // The 4 false NONEs of search-retro 83012811238074165395 that were no sweep for the word.
    assert!(!sweeps_for("cat > m.mjs <<'EOF'\nconst add = 'parityWatch.js now'\nEOF\nnode m.mjs && grep -c 'Fixed 2026-11-02' $f", &names));
    assert!(!sweeps_for("local-search search 'rev-parse' --corpus code | grep -i \"parse\"", &["parse".to_string()]));
    assert!(!sweeps_for("sed -n 1,9p t.jsonl | grep -oE 'migrate' | grep -i migrate", &["migrate".to_string()]));
    assert!(!sweeps_for("sed -n 1p a.tsx; grep -rn \"editEventAction\" src/i18n", &["DesktopEventChip".to_string()]));
    // Real sweeps still count, after `;`, `&&` and `||` too.
    assert!(sweeps_for("cd ~/automation; rg -l \"fn score|grade\" .", &["score".to_string()]));
    assert!(sweeps_for("LOCAL_SEARCH_MISS=1 rg -n parityWatch web", &names));
    assert!(sweeps_for("false || grep -rn parityWatch web", &names));
    // Search-retro 812181474823629636591 (f7ae8eda…jsonl:519): listing every env reader after the
    // right NONE for `process.env.AGENT_` is no sweep for it; a grep for the member still is.
    let env = names_of("process.env.AGENT_");
    assert_eq!(env, vec!["AGENT_".to_string()]);
    assert!(!sweeps_for("cd agent && ls; rg -n \"process\\.env\" src --glob '!**/config/**' -l | head", &env));
    assert!(sweeps_for("rg -n \"process\\.env\\.AGENT_\" src", &env));
    // 7746c089…jsonl:52: the rg over unindexed trees that the code NONE asked for is no re-search.
    let tail = vec!["tailscale".to_string()];
    assert!(!sweeps_for_in("rg -il tailscale ~/automation/config ~/automation/host ~/.local/bin 2>&1 | head -30; scutil --nc list", &tail, "/Users/b", ""));
    assert!(sweeps_for_in("rg -il tailscale ~/automation/config ~/.codex/skills", &tail, "/Users/b", ""));
    // Search-retro 8988472317094567194: a code NONE (0e0dcd6a…jsonl:209), then a grep of another session's transcript (:229).
    let target = vec!["mars-i18n-server-target".to_string()];
    let grep = "grep -o 'mars-i18n-server-target' /Users/b/.claude-profiles/team4/projects/x/cf0987b5.jsonl | head -5";
    assert!(!sweeps_for_in(grep, &target, "/Users/b", "local-search search mars-i18n-server-target --corpus code --lines"));
    assert!(sweeps_for_in(grep, &target, "/Users/b", "local-search search mars-i18n-server-target --corpus history"));
    assert_eq!(commands("a | b || c").iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![false, true, false]);
}

#[test]
fn declaration_keywords_do_not_tie_a_sweep_to_a_search() {
    assert_eq!(invocation("local-search search \"function DesktopEventChip\" --corpus code"), Some(vec!["DesktopEventChip".to_string()]));
    assert_eq!(invocation("local-search search \"struct ReminderDraft\""), Some(vec!["ReminderDraft".to_string()]));
}

#[test]
fn upper_case_sql_keywords_are_no_names() {
    // Search-retro 86517430927628776430, 60390666…jsonl:330: a later search for another statement
    // (`FROM crm.user_account …`, :337) shared only `FROM`.
    assert_eq!(
        invocation("local-search search \"SELECT enabled, day_reminder FROM crm.birthday_settings\" --corpus code"),
        // `crm.birthday_settings` is a lowercase access path (`chain.rs`): its member comes first.
        Some(vec!["birthday_settings".to_string(), "enabled".to_string(), "day_reminder".to_string()])
    );
    // A lower-case word stays a name: `update` may be a real function.
    assert_eq!(invocation("local-search search \"update\""), Some(vec!["update".to_string()]));
}

#[test]
fn a_file_read_by_name_after_cd_into_its_directory_is_named() {
    // Search-retro 815462549835379528073: 6b9ba9de…jsonl:1762, 9c4c41ab…jsonl:353.
    assert!(names_from_cd("cd /r/mars-ops/src/canary; sed -n 104,130p preflight.rs", "canary/preflight.rs"));
    assert!(names_from_cd("cd \"/r/seo_competitor/\" && sed -n 50,90p watch_publish.rs | head", "seo_competitor/watch_publish.rs"));
    assert!(!names_from_cd("cd /r/other; sed -n 1,5p preflight.rs", "canary/preflight.rs"));
    assert!(!names_from_cd("sed -n 1,5p preflight.rs; cd /r/canary", "canary/preflight.rs"));
    assert!(!names_from_cd("cd /r/canary; sed -n 1,5p preflight.rs.bak", "canary/preflight.rs"));
    assert!(!names_from_cd("cd /r/canary; ls", "canary/preflight.rs"));
    // A glob over a distinctive directory (search-retro 89714103305116763065, fb3fa899…jsonl:1900).
    assert!(names_from_cd("cd /r/mars-ops-crm/src/fix_notice; ls; grep -rn \"language\" *.rs | grep -v \"^cli.rs\"", "fix_notice/cli.rs"));
    assert!(!names_from_cd("cd /r/canary; grep -n x *.rs", "canary/preflight.rs"));
    assert!(!names_from_cd("cd /r/fix_notice; grep -n x *.ts", "fix_notice/cli.rs"));
    assert!(!names_from_cd("cd /r/fix_notice; ls", "fix_notice/cli.rs"));
}

#[test]
fn a_relative_path_from_the_cd_directory_names_the_file() {
    // Search-retro 812798351439866126096: 0fb18ebc…jsonl:1385 returned lib/composerAttachments.ts,
    // :1408 read it as `../composerAttachments.ts` from lib/api.
    let hit = "lib/composerAttachments.ts";
    assert!(names_from_cd("cd /r/web/src/lib/api && sed -n 60,135p directUpload.ts; grep -n \"x\" -A25 ../composerAttachments.ts | head", hit));
    assert!(names_from_cd("cd /r/web/src/lib && sed -n 1,9p ./composerAttachments.ts", hit));
    assert!(names_from_cd("cd /r/web/src/lib/api/deep && cat ../../composerAttachments.ts", hit));
    // Controls: another directory, a bare name outside the parent, a path that resolves elsewhere.
    assert!(!names_from_cd("cd /r/web/src/other/api && cat ../composerAttachments.ts", hit));
    assert!(!names_from_cd("cd /r/web/src/lib/api && cat composerAttachments.ts", hit));
    assert!(!names_from_cd("cd /r/web/src/lib/api && cat ../../composerAttachments.ts", hit));
    assert!(!names_from_cd("cat ../composerAttachments.ts", hit));
}

#[test]
fn a_two_character_han_query_is_a_name_only_on_its_own() {
    // Search-retro 87276522378180065818 (founder decision "确认不存在计入"), d02e115b…jsonl:517.
    assert_eq!(names_of("何棠"), vec!["何棠".to_string()]);
    assert!(!names_of("何棠 demo seed").contains(&"何棠".to_string()));
    assert_eq!(names_of("子任务"), vec!["子任务".to_string()]);
}

#[test]
fn search_retro_814125901426638775162_adoption_miscounts() {
    use super::super::answered::accepted;
    use super::super::steps::{Kind, Step};
    let shell = |text: &str| Step { at_ms: 1.0, id: "1".into(), kind: Kind::Shell, text: text.into(), line: 1, turn: "1".into() };
    // 31b7923a…jsonl:152: a one-word acronym query is that name, so its answered hit can count.
    assert_eq!(names_of("ICP"), vec!["ICP".to_string()]);
    assert_eq!(names_of("ICP footer"), vec!["footer".to_string()]);
    // Founder decision 1A on search-retro 85874229037822047064: any one three-character word is the name.
    assert_eq!(names_of("Foo"), vec!["Foo".to_string()]);
    assert_eq!(names_of("278"), vec!["278".to_string()]);
    assert_eq!(names_of("/man/"), vec!["man".to_string()]);
    assert!(names_of("ab").is_empty());
    // Founder decision 第1项选A on search-retro 812685146552855153702: in a query made only of one-word
    // alternatives each three-character alternative is a name (40c0a03c…jsonl:29 `ag1|agy`).
    assert_eq!(names_of("ag1|agy"), vec!["ag1".to_string(), "agy".to_string()]);
    assert_eq!(names_of("cg4|gemini-4-carbon|carbon"), vec!["cg4".to_string(), "gemini-4-carbon".to_string(), "carbon".to_string()]);
    // An alternative with a space is a phrase, so short words in it stay out.
    assert_eq!(names_of("fn foo|bar"), Vec::<String>::new());
    // e265c2a6…jsonl:3883: a path alternative holds `api`, which is no alternative of its own.
    assert_eq!(names_of("rememberRecentList|storedRecentList|/api/threads\\?limit"), vec!["rememberRecentList".to_string(), "storedRecentList".to_string(), "threads".to_string(), "limit".to_string()]);
    let ag = vec!["ag1".to_string(), "agy".to_string()];
    // :37 ran `ag1 --model …` and searched nothing else for the names: the printed lines answered.
    assert!(accepted(&[], &[shell("~/.local/bin/ag1 --model gemini-4-carbon-high -p hi")], 0.0, &ag, false, 8, 9e5));
    assert!(!accepted(&[], &[shell("rg -n 'ag1' crates/script-guard")], 0.0, &ag, false, 8, 9e5));
    let man = vec!["man".to_string()];
    // b7876efb…jsonl:254 / 044c53fa…jsonl:803: no later search for the word, so the printed lines answered.
    assert!(accepted(&[], &[shell("mars k8s sql --help")], 0.0, &man, false, 8, 9e5));
    // Only a whole-word re-search counts: `command` does not hold `man`.
    assert!(accepted(&[], &[shell("rg -n command src")], 0.0, &man, false, 8, 9e5));
    assert!(!accepted(&[], &[shell("rg -n '/man/' src")], 0.0, &man, false, 8, 9e5));
    let icp = vec!["ICP".to_string()];
    assert!(accepted(&[], &[shell("rg -n 'ICPC' web/src")], 0.0, &icp, false, 8, 9e5));
    assert!(!accepted(&[], &[shell("rg -n 'ICP备' web/src")], 0.0, &icp, false, 8, 9e5));
    // 04a9629b…jsonl:1142: session-status's own name is not a search for `status`; its arguments are.
    let deliver = vec!["deliver".to_string(), "status".to_string()];
    assert!(accepted(&[], &[shell("~/.local/bin/session-status fc3856e7 2>&1 | sed -n 1,6p")], 0.0, &deliver, false, 8, 9e5));
    assert!(!accepted(&[], &[shell("~/.local/bin/session-status deliver status")], 0.0, &deliver, false, 8, 9e5));
    // 04a9629b…jsonl:964: a grep inside Mars_studio, which no corpus indexes, is not a re-search.
    let offload = vec!["offload".to_string(), "wait".to_string()];
    assert!(accepted(&[], &[shell("cd /Volumes/MarsWork/Workspaces/Mars_studio/.claude/skills; LOCAL_SEARCH_MISS=other grep -rn \"offload hosts\\|offload wait\" . 2>/dev/null | head")], 0.0, &offload, false, 8, 9e5));
    assert!(accepted(&[], &[shell("rg -n 'offload wait' /Volumes/MarsWork/Workspaces/Mars_studio/AGENTS.md")], 0.0, &offload, false, 8, 9e5));
    assert!(!accepted(&[], &[shell("cd /Volumes/MarsWork/Workspaces/Mars_agent; grep -rn 'offload wait' .claude")], 0.0, &offload, false, 8, 9e5));
    assert!(!accepted(&[], &[shell("cd /Volumes/MarsWork/Workspaces/Mars_studio; grep -rn 'offload wait' /Volumes/MarsWork/Workspaces/Mars_agent/.claude")], 0.0, &offload, false, 8, 9e5));
}

#[test]
fn search_retro_815399477109309239632_host_names() {
    use super::super::answered::accepted;
    use super::super::steps::{Kind, Step};
    let shell = |text: &str| Step { at_ms: 1.0, id: "1".into(), kind: Kind::Shell, text: text.into(), line: 1, turn: "1".into() };
    // be857861…jsonl:51: the hosts are names, their labels `apps`/`apple` are not.
    let names = names_of("apps.apple.com|itunes.apple.com|appStoreUrl|APP_STORE");
    assert_eq!(names, ["apps.apple.com", "itunes.apple.com", "appStoreUrl", "APP_STORE"].map(String::from).to_vec());
    // be857861…jsonl:77: a search for the app id is another question.
    assert!(accepted(&[], &[shell("~/.local/bin/local-search search \"ascAppId|appStoreId|ASC_APP_ID|apple_app_id\" --corpus code")], 0.0, &names, false, 8, 9e5));
    // A search for the same host, escaped or not, is still a re-search.
    assert!(!accepted(&[], &[shell("~/.local/bin/local-search search \"apps.apple.com\" --corpus docs")], 0.0, &names, false, 8, 9e5));
    assert!(sweeps_for_in("rg -n 'apps\\.apple\\.com' Mars_CRM/web", &names, "/Users/x", ""));
}

#[test]
fn look_alikes_under_the_whole_word_label_are_suggestions() {
    use super::super::returned::returned_or_used;
    // Search-retro 85383374938275326400, 8a06d9aa…jsonl:223: no whole-word line in scope, only `OpenSubmission` look-alikes.
    let cmd = "local-search search \"Submission {\" --corpus code --lines --path tests";
    let out = "  -- outside --path: 6 more file(s) hold it, as a whole word first /Users/b/automation/crates/listing-watch/src/ledger.rs:21 --\ntests/ios_appstore_resubmit.rs:25:fn open(id: &str) -> OpenSubmission {\ntests/ios_appstore_resubmit.rs:26:    OpenSubmission { id: id.into() }";
    assert!(returned_or_used(cmd, out, |_| false).is_empty());
    assert_eq!(returned_or_used(cmd, out, |f| f.iter().any(|k| k == "tests/ios_appstore_resubmit.rs")), ["tests/ios_appstore_resubmit.rs"]);
    // Under a whole-word line the look-alikes stay returned files (7b999c5e…jsonl:130 edited one).
    let mixed = "a/adopt.rs:9:pub fn adopt() {\n  -- outside --path: 3 more file(s) hold it, as a whole word first /x/app.rs:115 --\na/adoption.rs:4:fn adoption()";
    assert_eq!(returned_or_used(cmd, mixed, |_| false), ["a/adopt.rs", "a/adoption.rs"]);
    // The plain count label (no look-alikes in scope) changes nothing.
    assert_eq!(returned_files("a/adopt.rs:9:pub fn adopt() {\n  -- outside --path: 3 more file(s) hold it, first /x/app.rs:115 --"), ["a/adopt.rs"]);
}

#[test]
fn search_retro_814194729051765393311_refused_lines() {
    // 884cd1bf…jsonl:130 was refused by script-guard; :142 retried the same query 2.8 s later and
    // the engine logged that retry; :161 was a refused sweep naming `cloneThread`.
    let home = crate::telemetry::test_home("search-quality-refused");
    let dir = home.join(".claude/projects/-r");
    std::fs::create_dir_all(&dir).unwrap();
    let at = |s: u32| format!("2026-11-02T10:00:{s:02}.000Z");
    let call = |s: u32, id: &str, cmd: &str| json!({"timestamp": at(s), "message": {"id": format!("m{id}"), "content": [{"type": "tool_use", "id": id, "input": {"command": cmd}}]}});
    let result = |s: u32, id: &str, text: &str| json!({"timestamp": at(s), "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}});
    let refused = "PreToolUse:Bash hook error: [command hook 1]: Blocked by script-guard: this grep sweeps an indexed code tree.\nNo local-search call of the last 10 minutes searched for: messageFilter";
    let rows = [
        call(1, "a", "local-search search \"branchThread|cloneThread\" --corpus code --lines | head; grep -n cloneThread -r agent/node_modules"),
        result(1, "a", refused),
        call(4, "b", "local-search search \"branchThread|cloneThread\" --corpus code --lines | head"),
        result(4, "b", "agent/src/domain/messageActions.ts:88:export async function branchThread("),
        call(11, "c", "rg -n \"messageFilter|cloneThread\" dist/index.d.ts | head"),
        result(11, "c", refused),
    ];
    std::fs::write(dir.join("11111111-1111-1111-1111-111111111112.jsonl"), rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
    std::fs::create_dir_all(home.join(".local/share/local-search")).unwrap();
    let row = json!({"ts_ms": 1793613604200.0, "sub": "search", "hits": 4, "search_ms": 20.0, "query": "branchThread|cloneThread"});
    std::fs::write(home.join(".local/share/local-search/queries.jsonl"), format!("{row}\n")).unwrap();
    let from = crate::cause_tree::attribute::parse_time(&at(0)).unwrap();
    let c = &measure(&home, from, from + 60_000.0)["calls"];
    assert_eq!((c["not_run"].as_u64(), c["total"].as_u64(), c["false_none"].as_u64()), (Some(1), Some(1), Some(0)), "{c}");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn search_retro_811204797093281505698_follow_up_after_the_window_end() {
    // 20ab51b1…jsonl:1275 ran a `query` 5 s before the window closed; the agent read the hit with a
    // `sed` at :1299, after the close. That `sed` adopts the hit; a search after the close is no call.
    let home = crate::telemetry::test_home("search-quality-edge");
    let dir = home.join(".claude/projects/-r");
    std::fs::create_dir_all(&dir).unwrap();
    let at = |s: u32| format!("2026-11-02T10:00:{s:02}.000Z");
    let call = |s: u32, id: &str, cmd: &str| json!({"timestamp": at(s), "message": {"id": format!("m{id}"), "content": [{"type": "tool_use", "id": id, "input": {"command": cmd}}]}});
    let result = |s: u32, id: &str, text: &str| json!({"timestamp": at(s), "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}});
    let rows = [
        call(5, "a", "local-search query \"web route that opens a thread\" --corpus code | head -12"),
        result(5, "a", "  /r/web/src/screens/settings/CreditUsageLedger.tsx:169-172  [code/block] export function conversationTurnPath"),
        call(12, "b", "sed -n 165,175p web/src/screens/settings/CreditUsageLedger.tsx"),
        result(12, "b", "export function conversationTurnPath(threadId: string) {"),
        call(14, "c", "local-search search \"conversationTurnPath\" --corpus code --lines"),
        result(14, "c", "web/src/screens/settings/CreditUsageLedger.tsx:169:export function conversationTurnPath("),
    ];
    std::fs::write(dir.join("11111111-1111-1111-1111-111111111113.jsonl"), rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
    std::fs::create_dir_all(home.join(".local/share/local-search")).unwrap();
    let row = json!({"ts_ms": 1793613605200.0, "sub": "query", "hits": 5, "embed_ms": 80.0, "search_ms": 20.0, "query": "web route that opens a thread"});
    std::fs::write(home.join(".local/share/local-search/queries.jsonl"), format!("{row}\n")).unwrap();
    let from = crate::cause_tree::attribute::parse_time(&at(0)).unwrap();
    let c = &measure(&home, from, from + 10_000.0)["calls"];
    assert_eq!((c["total"].as_u64(), c["hits"].as_u64(), c["adopted"].as_u64()), (Some(1), Some(1), Some(1)), "{c}");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn search_retro_816805219412517074086_member_chain_is_its_type() {
    use super::super::answered::accepted;
    use super::super::steps::{Kind, Step};
    let shell = |text: &str| Step { at_ms: 1.0, id: "1".into(), kind: Kind::Shell, text: text.into(), line: 1, turn: "2".into() };
    // f1cbfdbf…jsonl:3217 printed the call site; :3253 grepped another type's singleton.
    let names = names_of("AppStorePurchases.shared.configure");
    assert_eq!(names, vec!["AppStorePurchases".to_string()]);
    let other = shell(r#"cd /x/ios && grep -n "static.*var current\|static let shared\|func refresh" MarsCRM/Core/Network/APIClient.swift MarsCRM/Features/Settings/AccountStore.swift"#);
    assert!(accepted(&[], &[other], 0.0, &names, false, 8, 9e5));
    // A later search for the type or the whole chain is still a re-search.
    assert!(!accepted(&[], &[shell("rg -n 'AppStorePurchases' ios")], 0.0, &names, false, 8, 9e5));
    assert!(!accepted(&[], &[shell("grep -rn 'AppStorePurchases.shared.configure' ios")], 0.0, &names, false, 8, 9e5));
}
