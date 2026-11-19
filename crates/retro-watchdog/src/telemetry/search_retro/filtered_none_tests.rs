//! Search-retro 82856125487639539959: two false NONEs that were the agent's own filter or the sweep
//! step's own local-search output, each with a control that stays a false NONE.
use serde_json::json;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| n.to_string()).collect()
}

#[test]
fn a_local_search_none_inside_the_sweep_step_is_no_match() {
    // 895a65e4…jsonl:310 and its output :312 (the grep printed only `AssistantTurnView` lines).
    let sweep = r#"~/.local/bin/local-search search "StepCardView" --corpus code --per-source 1 --limit 100 --lines --path ios/MarsCRM/Features/Onboarding 2>&1 | head -6; grep -n "StepCardView\|AssistantTurnView\|StrandsOrb" ios/MarsCRM/Features/Onboarding/OnboardingChatPhone.swift | head -4"#;
    let block = "NONE: no code chunk has every word. The code corpus holds Mars_agent and ~/automation (not its config/ or host/); build output is unindexed, so rg there.\n  -- outside --path: 7 file(s) hold it on one line, first 3 --\nios/MarsCRM/Features/Conversation/StepCardView.swift:12:struct StepCardView: View {\nios/MarsCRM/Features/Conversation/StepCardView.swift:155:        StepCardView(turn: LiveTurn(local: \"preview\"))\nios/MarsCRM/Features/Conversation/StepCardView.swift:156:        StepCardView(turn: LiveTurn(stored: x))\nios/MarsCRM/Features/Conversation/StepCardView.swift:166:    StepCardView(turn: y)";
    let grep = "6:/// into `OnboardingPhoneFrame`'s screen: the conversation's own `UserTurnView` and `AssistantTurnView`\n228:    /// An answer as `AssistantTurnView` draws it\n239:        return AssistantTurnView(turn: live, answerable: last,";
    let wanted = names(&["StepCardView", "StrandsOrb"]);
    assert!(!super::none::confirms_names(&format!("{block}\n{grep}"), sweep, &wanted));
    // Control: the grep itself printed a query name.
    assert!(super::none::confirms_names(&format!("{block}\n40:  StrandsOrb(size: 32)"), sweep, &wanted));
    // Control: past the `head -6` limit, a path line belongs to the next program.
    let rg = sweep.replace("grep -n", "rg -n").replace("ios/MarsCRM/Features/Onboarding/OnboardingChatPhone.swift", "ios");
    assert!(super::none::confirms_names(&format!("{block}\nios/MarsCRM/Features/Onboarding/A.swift:3:StepCardView()"), &rg, &wanted));
    // Control: without a head limit, a fourth file is beyond the label's `first 3`.
    let unbounded = rg.replace(" | head -6", "");
    let three = "NONE: no code chunk has every word.\n  -- outside --path: 7 file(s) hold it on one line, first 3 --\na/One.swift:1:StepCardView\na/Two.swift:1:StepCardView\na/Three.swift:1:StepCardView";
    assert_eq!(super::none_block::strip(three, &unbounded), "");
    assert!(super::none::confirms_names(&format!("{three}\nios/Onboarding/Four.swift:9:StepCardView()"), &unbounded, &wanted));
    // A sweep step without local-search keeps every line.
    assert_eq!(super::none_block::strip(three, "grep -rn StepCardView ios"), three);
}

#[test]
fn an_excluding_filter_that_emptied_the_hits_is_unobserved() {
    // 25d92dc6…jsonl:1050: the engine logged 10 hits; `grep -v` dropped every one, then `rg` (:1056)
    // printed the definition line the filter had named.
    let run = |hits: u64| {
        let home = crate::telemetry::test_home(&format!("search-quality-excluded-{hits}"));
        let dir = home.join(".claude/projects/-r");
        std::fs::create_dir_all(&dir).unwrap();
        let at = |s: u32| format!("2026-11-02T10:00:{s:02}.000Z");
        let call = |s: u32, id: &str, cmd: &str| json!({"timestamp": at(s), "message": {"id": format!("m{id}"), "content": [{"type": "tool_use", "id": id, "input": {"command": cmd}}]}});
        let result = |s: u32, id: &str, text: &str| json!({"timestamp": at(s), "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}});
        let search = r#"cd /r; ~/.local/bin/local-search search "event_extraction::|extract_event_details\(" --corpus code --path Mars_CRM/rust/crates --per-source 3 --limit 40 --lines 2>&1 | grep -v "tests/\|mars-contracts/src/event_extraction.rs" | cut -c1-170 | head"#;
        let rows = [
            call(1, "a", search),
            result(2, "a", ""),
            call(5, "b", r#"cd /r/Mars_CRM/rust/crates; LOCAL_SEARCH_MISS=regex rg -n "extract_event_details|event_extraction::" --glob '*.rs' . | grep -v "/tests/" | head"#),
            result(6, "b", "./mars-contracts/src/event_extraction.rs:57:pub fn extract_event_details(input: &ExtractEventInput) -> Result<EventDetails, EventTimeError> {"),
        ];
        std::fs::write(dir.join("11111111-1111-1111-1111-111111111121.jsonl"), rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
        std::fs::create_dir_all(home.join(".local/share/local-search")).unwrap();
        let row = json!({"ts_ms": 1793613601500.0, "sub": "search", "hits": hits, "search_ms": 15.7, "query": "event_extraction::|extract_event_details\\("});
        std::fs::write(home.join(".local/share/local-search/queries.jsonl"), format!("{row}\n")).unwrap();
        let from = crate::cause_tree::attribute::parse_time(&at(0)).unwrap();
        let c = super::quality::measure(&home, from, from + 60_000.0)["calls"].clone();
        std::fs::remove_dir_all(&home).unwrap();
        (c["unobserved"].as_u64(), c["total"].as_u64(), c["false_none"].as_u64())
    };
    assert_eq!(run(10), (Some(1), Some(0), Some(0)));
    // Control: an engine row with no hits keeps the call and its false NONE.
    assert_eq!(run(0), (Some(0), Some(1), Some(1)));
}

#[test]
fn names_printed_above_the_echo_marker_are_no_sweep_match() {
    // Search-retro 812798351439866126096, 9f3ea62e…jsonl:81: the step's own history search printed
    // the name above `echo ----`; the rg below printed only a line with the other alternative.
    let sweep = r#"cd /r; ~/.local/bin/local-search search "zqxwvk" --corpus history --per-source 1 --limit 40 --lines 2>&1 | head -50; echo ----; rg -n "zqxwvk|creators" docs/traffic-sources.md | head -40"#;
    let above = "  /h/projects/-r/74b48193.jsonl:347  [history/assistant]\n    1. is this the zqxwvk account?\n----";
    let wanted = names(&["zqxwvk"]);
    assert!(!super::none::confirms_names(&format!("{above}\n108:influencers (creators) trying the product"), sweep, &wanted));
    // Control: the rg itself printed the name below the marker.
    assert!(super::none::confirms_names(&format!("{above}\n12:the zqxwvk account"), sweep, &wanted));
    // Control: without an echo marker every printed line still counts.
    let unmarked = sweep.replace("echo ----; ", "");
    assert!(super::none::confirms_names("  1. is this the zqxwvk account?\n108:influencers (creators)", &unmarked, &wanted));
}

#[test]
fn a_replayed_none_after_an_empty_sweep_is_no_match() {
    // Search-retro 817272677504635157723, 8ee59d2a…jsonl:908: the `rg -l` printed nothing; the step's
    // trailing `LOCAL_SEARCH_NO_LOG=1` history search printed a NONE and its tool-call hint lines.
    let sweep = r#"rg -l --hidden "\.zqxwvk/model" ~/automation/config ~/.config 2>/dev/null | head; echo "rg exit $?"; LOCAL_SEARCH_NO_LOG=1 ~/.local/bin/local-search search "zqxwvk/model" --corpus history --per-source 1 --limit 5 --lines 2>&1 | head -8"#;
    let out = "rg exit 0\nNONE: no history chunk has every word. History holds prompts, replies, summaries.\n  -- tool calls hold it: 9 in 3 transcript(s) of the last 14d, newest first (all of them: `local-search tools zqxwvk/model`) --\n  /u/.claude-profiles/b/projects/-r/b7876efb.jsonl:679  [Bash] 2026-11-09 A=~/.zqxwvk/model/m.bin; lsof \"$A\"\n  /u/.claude-profiles/b/projects/-r/0e0dcd6a.jsonl:585  [Bash] 2026-11-07 S=/u/.zqxwvk/model/m.bin; cp -c";
    let wanted = names(&["zqxwvk/model"]);
    assert_eq!(super::none_block::strip(out, sweep), "rg exit 0");
    assert!(!super::none::confirms_names(out, sweep, &wanted));
    // Control: a file the rg listed above the replay still confirms the name.
    assert!(super::none::confirms_names(&format!("/u/.config/tool/settings.toml\n{out}"), sweep, &wanted));
    // Report hits under a label and their indented text lines are the search's own.
    let report = "NONE: no code chunk has every word.\n  -- report: the code corpus holds none of it, report does; rerun with --corpus report --\n  https://report.example/disk-low-1/  [report/section]\n    13. zqxwvk model duplicated";
    assert_eq!(super::none_block::strip(report, sweep), "");
}
