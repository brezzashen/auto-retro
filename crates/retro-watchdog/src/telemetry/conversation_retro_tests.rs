use super::*;

const UUID: &str = "e1099ae8-d89d-4f75-a59a-362499ff70cf";

fn files(alias: &'static str, n: usize, from_ms: f64) -> Vec<Found> {
    (0..n).map(|i| (alias, format!("/t/{alias}/{i}.jsonl"), from_ms + i as f64)).collect()
}

fn census(at: f64, files: Vec<Found>, tabs: &[&str]) -> Census {
    let tabs = tabs.iter().map(|t| json!({"tab": t, "title": "Claude Code", "agent": "claude", "worktree": "/r"})).collect();
    Census { at, files, tabs: Ok(tabs) }
}

fn watch(at: f64) -> ConversationRetro {
    let mut watch = ConversationRetro::default();
    for scope in SCOPES {
        watch.baselines.insert(scope.into(), at);
    }
    watch
}

#[test]
fn counts_only_main_transcripts() {
    assert!(is_conversation(Layout::Claude, &format!("{UUID}.jsonl")));
    assert!(!is_conversation(Layout::Claude, "ag-boot-d-4ad8-b9ff-ca5edd198835.jsonl"));
    assert!(!is_conversation(Layout::Claude, &format!("{UUID}.json")));
    assert!(is_conversation(Layout::Codex, "rollout-2026-10-22T10-00-00-abc.jsonl"));
    assert!(!is_conversation(Layout::Codex, "history.jsonl"));

    let home = crate::telemetry::test_home("conversation-walk");
    let write = |path: &str| {
        let path = home.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "{}\n").unwrap();
    };
    write(&format!(".claude-profiles/team/projects/-home-user-workspace/{UUID}.jsonl"));
    write(&format!(".claude-profiles/team/projects/-home-user-workspace/{UUID}/subagents/agent-a.jsonl"));
    write(".codex/sessions/2026/10/22/rollout-2026-10-22T10-00-00-abc.jsonl");
    write(".unregistered-agent/sessions/example.jsonl");
    let since: Vec<(&'static str, f64)> = SOURCES.iter().map(|(alias, _, _)| (*alias, 0.0)).collect();
    let mut found: Vec<&str> = transcripts(&home, &since).iter().map(|(alias, _, _)| *alias).collect();
    found.sort();
    assert_eq!(found, ["cc3", "co1"]);
    // A source whose baseline is newer than the file does not count it.
    let later: Vec<(&'static str, f64)> = since.iter().map(|(alias, _)| (*alias, if *alias == "cc3" { f64::MAX } else { 0.0 })).collect();
    assert_eq!(transcripts(&home, &later).len(), 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn every_scope_counts_from_its_own_baseline() {
    let mut watch = watch(1000.0);
    watch.baselines.insert("cc3".into(), 1050.0);
    let mut found = files("cc3", 60, 1001.0);
    found.extend(files("co1", 30, 1001.0));
    found.extend(files("co2", 5, 1001.0));
    assert_eq!(watch.count("all", &found), 95);
    assert_eq!(watch.count("cc3", &found), 10);
    assert_eq!(watch.count("cc4", &found), 0);
    assert!(covers("all", "co2") && !covers("cc3", "co1"));
}

#[test]
fn a_full_scope_opens_one_session_until_it_acknowledges() {
    let home = crate::telemetry::test_home("conversation-escalate");
    let mut watch = watch(1000.0);
    let mut found = files("cc4", 70, 1001.0);
    found.extend(files("co2", 29, 1001.0));
    assert_eq!(watch.absorb(&home, census(5000.0, found.clone(), &[])).unwrap(), None);

    found.extend(files("cc1", 1, 2000.0));
    let id = watch.absorb(&home, census(6000.0, found.clone(), &[])).unwrap().expect("incident");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!((incident["scope"].as_str(), incident["count"].as_u64()), (Some("all"), Some(100)));
    assert_eq!(incident["conversations"].as_array().unwrap().len(), 100);
    assert_eq!(incident["requirements"]["resource_analysis"], 1);
    assert_eq!(watch.baselines["all"], 6000.0);
    assert_eq!(watch.count("all", &found), 0);

    // cc4 reaches 100 on its own baseline, but waits while the first session is unacknowledged.
    found.extend(files("cc4", 30, 7000.0));
    assert_eq!(watch.absorb(&home, census(8000.0, found.clone(), &[])).unwrap(), Some(id.clone()));
    write_json(&root(&home).join("incidents").join(format!("{id}.ack")), &json!({})).unwrap();
    let next = watch.absorb(&home, census(9000.0, found.clone(), &[])).unwrap().expect("cc4 incident");
    assert_ne!(next, id);
    assert_eq!(watch.dispatches.last().unwrap()["scope"], "cc4");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn orca_tabs_open_at_the_baseline_do_not_count() {
    let home = crate::telemetry::test_home("conversation-orca");
    let mut watch = watch(1000.0);
    watch.absorb(&home, census(1000.0, vec![], &["a", "b"])).unwrap();
    assert_eq!(watch.count("orca", &[]), 0);
    // A new tab counts and stays counted after it closes; a closed baseline tab is forgotten.
    watch.absorb(&home, census(2000.0, vec![], &["b", "c"])).unwrap();
    watch.absorb(&home, census(3000.0, vec![], &["b"])).unwrap();
    assert_eq!(watch.count("orca", &[]), 1);
    assert_eq!(watch.tabs.len(), 2);
    let many: Vec<String> = (0..THRESHOLD - 1).map(|i| format!("n{i}")).collect();
    let open: Vec<&str> = many.iter().map(String::as_str).collect();
    let id = watch.absorb(&home, census(4000.0, files("cc5", 3, 1500.0), &open)).unwrap().expect("orca incident");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["scope"], "orca");
    assert_eq!(incident["orca_tabs"].as_array().unwrap().len(), THRESHOLD);
    assert_eq!(incident["conversations"].as_array().unwrap().len(), 3);
    assert_eq!(watch.count("orca", &[]), 0);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn the_daily_cap_holds_further_sessions() {
    let home = crate::telemetry::test_home("conversation-cap");
    let mut watch = watch(0.0);
    let mut at = 10_000.0;
    for _ in 0..MAX_DISPATCH_PER_DAY {
        let id = watch.absorb(&home, census(at, files("cc1", THRESHOLD, at - 5000.0), &[])).unwrap().unwrap();
        write_json(&root(&home).join("incidents").join(format!("{id}.ack")), &json!({})).unwrap();
        at += 10_000.0;
    }
    assert_eq!(watch.absorb(&home, census(at, files("cc1", THRESHOLD, at - 5000.0), &[])).unwrap(), None);
    assert!(watch.absorb(&home, census(at + DAY_MS, files("cc1", THRESHOLD, at + DAY_MS - 5000.0), &[])).unwrap().is_some());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn the_prompt_names_a_route_that_exists() {
    let text = prompt(Path::new("/h"), "81", "all", 100);
    assert!(text.contains("MANDATORY before analysis or edits") && text.contains("retro-recall 81"));
    assert!(text.contains("retro-resources 81") && text.contains("CPU/GPU") && text.contains("resource_analysis"));
    assert!(text.contains("route conversation-retro") && text.contains("monitor-ack 81") && !text.contains('\n'));
    let skill = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".codex/skills/watchdog");
    if skill.exists() {
        assert!(skill.join("references/conversation-retro.md").exists());
        assert!(std::fs::read_to_string(skill.join("SKILL.md")).unwrap().contains("`conversation-retro`"));
    }
}

#[test]
fn the_due_scope_waiting_longest_goes_first() {
    let home = crate::telemetry::test_home("conversation-fair");
    let mut watch = watch(5000.0);
    // cc7 (late in SCOPES) has waited since 1000; all and cc1 are due too but reset at 5000.
    watch.baselines.insert("cc7".into(), 1000.0);
    let mut found = files("cc7", THRESHOLD, 1001.0);
    found.extend(files("cc1", THRESHOLD, 5001.0));
    watch.absorb(&home, census(9000.0, found, &[])).unwrap().expect("incident");
    assert_eq!(watch.dispatches.last().unwrap()["scope"], "cc7");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn bridged_antigravity_sessions_count_in_ag_not_cc1() {
    // Founder decision 2026-11-14 A (conversation-retro-814983970739705856954): ag has its own scope inside all.
    let home = crate::telemetry::test_home("conversation-ag");
    let write = |dir: &str, body: &str| {
        let path = home.join(".claude/projects").join(dir).join(format!("{UUID}.jsonl"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    };
    write("-Users-u-antigravity-orca-bridge", r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#);
    write("-Volumes-MarsWork-Workspaces-Mars-agent", r#"{"type":"user","entrypoint":"cli"}"#);
    let mut watch = watch(1000.0);
    watch.baselines.insert("cc1".into(), f64::MAX);
    watch.baselines.insert("ag".into(), 0.0);
    // cc1's walk must reach back to the ag baseline although cc1 itself starts later.
    assert_eq!(watch.since().iter().find(|(a, _)| *a == "cc1").unwrap().1, 0.0);
    let found = transcripts(&home, &watch.since());
    let mut aliases: Vec<&str> = found.iter().map(|(alias, _, _)| *alias).collect();
    aliases.sort();
    assert_eq!(aliases, ["ag", "cc1"]);
    assert_eq!(watch.count("ag", &found), 1);
    assert_eq!(watch.count("cc1", &found), 0);
    assert!(covers("all", "ag") && !covers("cc1", "ag"));
    std::fs::remove_dir_all(home).unwrap();
}
