use super::*;

fn transcript(dir: &Path, name: &str, rows: &[(&str, &str)]) {
    let lines: Vec<String> = rows.iter().enumerate().map(|(i, (ts, cmd))| json!({"type": "assistant", "timestamp": ts,
        "message": {"content": [{"type": "tool_use", "id": format!("call-{i}"), "name": "Bash", "input": {"command": cmd}}]}}).to_string()).collect();
    std::fs::write(dir.join(name), lines.join("\n") + "\n").unwrap();
}

#[test]
fn bridge_slug_follows_home() {
    assert_eq!(bridge_dir(Path::new("/home/user")), Path::new("/home/user/.claude/projects/-home-user-antigravity-orca-bridge"));
}

#[test]
fn reads_bridged_bash_calls_inside_the_window() {
    let home = crate::telemetry::test_home("unhooked-calls");
    let dir = bridge_dir(&home);
    std::fs::create_dir_all(&dir).unwrap();
    transcript(&dir, "d10f9eef.jsonl", &[
        ("2026-11-04T12:00:00Z", "rg -n early ~/.agent-memory"),
        ("2026-11-04T12:57:05Z", "rg -n 'home IP' ~/.agent-memory ~/.claude/skills"),
        ("2026-11-04T14:00:00Z", "rg -n late ~/.agent-memory"),
    ]);
    let from = crate::cause_tree::attribute::parse_time("2026-11-04T12:30:00Z").unwrap();
    let to = crate::cause_tree::attribute::parse_time("2026-11-04T13:30:00Z").unwrap();
    let found = calls(&dir, from, to);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].session, "d10f9eef");
    assert!(found[0].locator.ends_with("d10f9eef.jsonl:2"), "{}", found[0].locator);
}

#[test]
fn would_be_blocks_score_as_unguarded_sweeps() {
    let call = Call { session: "d10f9eef".into(), at_ms: 1.0, locator: "x.jsonl:7".into(), command: String::new() };
    let lists = vec![vec![
        json!({"kind": "grep", "target": "history", "scope": "tree", "pattern": "regex", "verdict": "blocked"}),
        json!({"kind": "grep", "target": "code", "scope": "tree", "pattern": "phrase", "verdict": "blocked"}),
        json!({"kind": "local_search", "target": "code", "verdict": "allowed"}),
    ]];
    let events = tag(&[call], lists);
    assert!(events.iter().all(|e| e["agent"] == "antigravity" && e["locator"] == "x.jsonl:7"));
    assert_eq!(events[0]["verdict"], "unguarded");
    let s = super::super::score::score(&events);
    // history_report 0/(0+1), no_tree_grep 1/(1+1), phrase_routing 1/(1+1).
    assert_eq!(s["components"], json!({"history_report": 0.0, "no_tree_grep": 0.5, "phrase_routing": 0.5}));
    assert_eq!(s["counts"]["unguarded"], 2);
    assert_eq!(s["by_agent"]["antigravity/bridge"]["grep_tree_ran"], 1);
}

#[test]
fn sessions_the_hook_logged_are_not_metered_again() {
    let hooked = vec![
        json!({"agent": "antigravity", "profile": "hook", "session": "8a2c464f", "kind": "grep", "verdict": "blocked"}),
        json!({"agent": "claude", "session": "c1", "kind": "local_search"}),
    ];
    let skip = hooked_sessions(&hooked);
    assert_eq!(skip.into_iter().collect::<Vec<_>>(), vec!["8a2c464f".to_string()]);
    let home = crate::telemetry::test_home("unhooked-skip");
    let dir = bridge_dir(&home);
    std::fs::create_dir_all(&dir).unwrap();
    transcript(&dir, "8a2c464f.jsonl", &[("2026-11-05T06:42:54Z", "rg -n x ~/.cache/local-search/reports/")]);
    let from = crate::cause_tree::attribute::parse_time("2026-11-05T06:00:00Z").unwrap();
    let to = crate::cause_tree::attribute::parse_time("2026-11-05T07:00:00Z").unwrap();
    assert_eq!(calls(&dir, from, to).len(), 1);
    let (events, summary) = events(&home, from, to, &hooked);
    assert!(events.is_empty());
    assert_eq!(summary["calls"], 0);
}

#[test]
fn a_line_by_line_filter_with_more_than_two_pipe_buffers_of_io_does_not_deadlock() {
    // 300 000 lines in and out is about 1.2 MB each way, far beyond the two 64 KiB pipe buffers.
    let input: String = (0..300_000).map(|i| format!("{{\"command\":\"c{i}\"}}\n")).collect();
    let mut command = std::process::Command::new("/usr/bin/sed");
    command.args(["-l", "s/.*/[]/"]);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = sender.send(super::pipe(command, input)); });
    let out = receiver.recv_timeout(std::time::Duration::from_secs(30)).expect("pipe deadlocked").unwrap();
    assert_eq!(String::from_utf8_lossy(&out).lines().count(), 300_000);
}
