use super::*;
use parse::{command_label, family, timestamp_ms};

const T0: &str = "2026-10-22T10:00:00.000Z";

fn at(secs: f64) -> String {
    let ms = timestamp_ms(T0).unwrap() + secs * 1000.0;
    let s = ms / 1000.0;
    let (day, rem) = ((s / 86_400.0).floor(), s % 86_400.0);
    assert_eq!(day, (timestamp_ms(T0).unwrap() / 86_400_000.0).floor(), "fixtures stay on one day");
    format!("2026-10-22T{:02}:{:02}:{:06.3}Z", (rem / 3600.0) as u32, ((rem % 3600.0) / 60.0) as u32, rem % 60.0)
}

fn claude_lines() -> Vec<Value> {
    vec![
        json!({"type": "user", "timestamp": at(0.0), "message": {"content": "fix it"}}),
        json!({"type": "assistant", "timestamp": at(3.0), "message": {"id": "m1", "model": "claude-opus-5-5", "usage": {"output_tokens": 10},
            "content": [{"type": "thinking"}]}}),
        json!({"type": "assistant", "timestamp": at(5.0), "message": {"id": "m1", "model": "claude-opus-5-5", "usage": {"output_tokens": 50},
            "content": [{"type": "tool_use", "name": "Read"}, {"type": "tool_use", "name": "Bash", "input": {"command": "cd /r && bun run k8s sql x"}}]}}),
        json!({"type": "user", "timestamp": at(12.0), "message": {"content": [{"type": "tool_result"}]}}),
        json!({"type": "assistant", "timestamp": at(14.0), "message": {"id": "m2", "model": "claude-opus-5-5", "usage": {"output_tokens": 20}, "content": []}}),
        // Idle past the cap: neither the wait nor the next turn counts.
        json!({"type": "user", "timestamp": at(14.0 + 3600.0), "message": {"content": "again"}}),
        json!({"type": "assistant", "timestamp": at(14.0 + 3600.0 + 2000.0), "message": {"id": "m3", "model": "claude-opus-5-5", "usage": {"output_tokens": 5}, "content": []}}),
    ]
}

#[test]
fn claude_turns_split_into_model_time_tool_wait_and_tokens() {
    let mut cursor = Cursor::default();
    cursor.alias = "cc3".into();
    let mut tally = Tally::default();
    for line in claude_lines() { cursor.feed(&line, &mut tally); }
    let key = "cc3 lead opus";
    assert_eq!(tally.model_ms[key], 7000.0, "5 s to the first output plus 2 s after the tool result");
    assert_eq!(tally.turns[key], 2.0);
    assert_eq!(tally.output_tokens[key], 70.0 + 5.0, "streamed records of m1 count once at their largest");
    assert_eq!(tally.tool_ms["Bash:bun run k8s"], 7000.0, "the wait goes to the tool that is not a plain read");
    assert_eq!(cursor.model_ms, 7000.0);
}

#[test]
fn codex_turns_use_response_items() {
    let mut cursor = Cursor::default();
    (cursor.alias, cursor.codex) = ("co1".into(), true);
    let mut tally = Tally::default();
    let call = r#"text(await tools.exec_command({cmd:"rg -n foo src","max_output_tokens":2000}))"#;
    for line in [
        json!({"type": "turn_context", "timestamp": at(0.0), "payload": {"model": "gpt-6.1-sol"}}),
        json!({"type": "response_item", "timestamp": at(1.0), "payload": {"type": "message", "role": "user"}}),
        json!({"type": "response_item", "timestamp": at(4.0), "payload": {"type": "reasoning"}}),
        json!({"type": "response_item", "timestamp": at(5.0), "payload": {"type": "custom_tool_call", "name": "exec", "input": call}}),
        json!({"type": "response_item", "timestamp": at(9.0), "payload": {"type": "custom_tool_call_output"}}),
        json!({"type": "event_msg", "timestamp": at(9.0), "payload": {"type": "token_count", "info": {"last_token_usage": {"output_tokens": 120}}}}),
        json!({"type": "response_item", "timestamp": at(11.0), "payload": {"type": "message", "role": "assistant"}}),
    ] { cursor.feed(&line, &mut tally); }
    assert_eq!(tally.model_ms["co1 lead gpt-6.1-sol"], 6000.0);
    assert_eq!(tally.tool_ms["exec:rg"], 4000.0);
    assert_eq!(tally.output_tokens["co1 lead gpt-6.1-sol"], 120.0);
}

#[test]
fn labels_timestamps_and_families() {
    assert_eq!(command_label("cd /a && FOO=1 bun scripts/x.ts --y"), "bun x.ts");
    assert_eq!(command_label("git -C ~/r status --short"), "git status");
    assert_eq!(command_label("/Users/b/.local/bin/session-title set x"), "session-title");
    assert_eq!(command_label("cargo run -p a"), "cargo run a");
    assert_eq!(command_label("S=.claude/skills/report/scripts; bun $S/publish.ts r"), "bun publish.ts");
    assert_eq!(command_label("timeout 120 xcrun simctl list"), "xcrun simctl");
    assert_eq!(command_label("until curl -s x; do sleep 5; done"), "until");
    assert_eq!(command_label("bun test ./scripts/tests/"), "bun test");
    assert_eq!(command_label("bun \"$S/run.ts\" brief.json"), "bun run.ts");
    // Setup and light reads yield to the program that waits (runtime retro 811163801920064345384).
    assert_eq!(command_label("S=/tmp/x; mkdir -p $S; bun .claude/skills/report/scripts/capture.js --out $S"), "bun capture.js");
    assert_eq!(command_label("export ASC_READ_ONLY=1 ASC_KEY_ID=K\nasc api GET '/v1/apps' 2>&1 | head -c 300"), "asc");
    assert_eq!(command_label("cat > $S/q.sql <<'EOF'\nselect 1;\nEOF\nmars k8s sql \"$(cat $S/q.sql)\""), "mars");
    assert_eq!(command_label("for i in 0 1 2; do echo \"== $i\"; MARS_RAW_KUBECTL=1 kubectl --context d get pods; done"), "kubectl get");
    assert_eq!(command_label("cat /tmp/t.output; date; bun run --cwd Mars_CRM ui-tools ios simRun"), "bun run ui-tools");
    assert_eq!(command_label("perl -0pi -e 's/a/b/' f.swift && cpu-build-gate run -- xcodebuild test"), "cpu-build-gate");
    assert_eq!(command_label("grep -n x f | head -5"), "grep", "a command of reads only keeps its first");
    assert_eq!(command_label("cat <<<\"$x\" | jq ."), "cat", "a here-string is not a heredoc");
    assert_eq!(command_label("cat > f <<'JSON'\n{\"a\": 1}\nJSON"), "cat", "a heredoc body is never a program");
    assert_eq!(command_label("cat /tmp/t.output 2>&1 | tail -3"), "cat", "the 1 of 2>&1 is not a program");
    assert_eq!(command_label("jq -r '.a | .b' f; bun -e 'await run()\nerror: x'"), "bun -e", "quoted text is not split");
    assert_eq!(command_label("udid=$(sim-lease acquire --family ipad); echo $udid"), "sim-lease");
    assert_eq!(command_label("ls -la; $S/render.sh a.swift"), "render.sh");
    assert_eq!(family("claude-haiku-5-5"), "haiku");
    assert_eq!(family("gpt-6-luna"), "gpt-6-luna");
    assert_eq!(timestamp_ms("1970-01-02T00:00:01.500Z"), Some(86_401_500.0));
    assert_eq!(timestamp_ms("2026-10-22T14:57:39.174Z"), Some(1_792_681_059_174.0));
    assert_eq!(timestamp_ms("yesterday"), None);
}

#[test]
fn scans_read_only_what_each_transcript_gained() {
    let home = crate::telemetry::test_home("runtime-scan");
    let dir = home.join(".claude-profiles/team/projects/-r");
    std::fs::create_dir_all(dir.join("u1/subagents")).unwrap();
    let lines: Vec<String> = claude_lines().iter().map(|l| l.to_string()).collect();
    std::fs::write(dir.join("u1.jsonl"), lines[..3].join("\n") + "\n").unwrap();
    std::fs::write(dir.join("u1/subagents/agent-a.jsonl"), lines[..3].join("\n") + "\n" + &lines[3][..20]).unwrap();
    let now = crate::unix_now() * 1000.0;
    let (files, tally) = parse::scan(&home, BTreeMap::new(), 0.0, 0.0, now);
    assert_eq!(tally.model_ms["cc3 lead opus"], 5000.0);
    assert_eq!(tally.model_ms["cc3 sub opus"], 5000.0, "a partial last line waits for its newline");
    // Appending the rest counts only the new lines.
    std::fs::write(dir.join("u1.jsonl"), lines[..5].join("\n") + "\n").unwrap();
    let (files, tally) = parse::scan(&home, files, 0.0, 0.0, now);
    assert_eq!((tally.model_ms["cc3 lead opus"], tally.tool_ms["Bash:bun run k8s"]), (2000.0, 7000.0));
    // A transcript created before the window starts at its end.
    let (_, tally) = parse::scan(&home, BTreeMap::new(), now + 1e9, 0.0, now);
    assert!(tally.model_ms.is_empty());
    assert_eq!(files.len(), 2);
    std::fs::remove_dir_all(home).unwrap();
}

fn hours(h: f64) -> Tally {
    let mut tally = Tally::default();
    tally.model_ms.insert("cc1 lead opus".into(), h * 3_600_000.0);
    tally.tool_ms.insert("Bash:rg".into(), 3_600_000.0);
    tally.tool_calls.insert("Bash:rg".into(), 10.0);
    tally
}

#[test]
fn a_full_window_opens_one_session_until_it_acknowledges() {
    let home = crate::telemetry::test_home("runtime-escalate");
    let mut watch = RuntimeRetro { loaded: true, window_from: 1000.0, ..RuntimeRetro::default() };
    let mut files = BTreeMap::new();
    let mut busy = Cursor::default();
    (busy.alias, busy.model_ms) = ("cc1".into(), 7_200_000.0);
    files.insert("/t/a.jsonl".to_string(), busy);
    assert_eq!(watch.absorb(&home, Scan { at: 2000.0, files: files.clone(), tally: hours(60.0) }).unwrap(), None);
    let id = watch.absorb(&home, Scan { at: 3000.0, files: files.clone(), tally: hours(40.0) }).unwrap().expect("incident");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["model_hours"], 100.0);
    assert_eq!(incident["breakdown"]["tool_wait_top"][0]["avg_s"], 360.0);
    assert_eq!(incident["transcripts"][0]["model_hours"], 2.0);
    assert_eq!((watch.window_from, watch.tally.model_hours()), (3000.0, 0.0));
    // The next window fills, but waits for the acknowledgement and then for the daily cap.
    assert_eq!(watch.absorb(&home, Scan { at: 4000.0, files: files.clone(), tally: hours(100.0) }).unwrap(), Some(id.clone()));
    write_json(&root(&home).join("incidents").join(format!("{id}.ack")), &json!({})).unwrap();
    assert_eq!(watch.absorb(&home, Scan { at: 5000.0, files: files.clone(), tally: Tally::default() }).unwrap(), None);
    let next = watch.absorb(&home, Scan { at: 5000.0 + DAY_MS, files, tally: Tally::default() }).unwrap().expect("next day");
    assert_ne!(next, id);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn calls_trigger_first_and_reset_both_counters() {
    let home = crate::telemetry::test_home("runtime-calls");
    let mut watch = RuntimeRetro { loaded: true, window_from: 1000.0, ..RuntimeRetro::default() };
    let mut busy = hours(30.0);
    busy.turns.insert("cc1 lead opus".into(), THRESHOLD_CALLS - 1.0);
    assert_eq!(watch.absorb(&home, Scan { at: 2000.0, files: BTreeMap::new(), tally: busy }).unwrap(), None);
    let mut one = Tally::default();
    one.turns.insert("cc1 sub haiku".into(), 1.0);
    let id = watch.absorb(&home, Scan { at: 3000.0, files: BTreeMap::new(), tally: one }).unwrap().expect("incident");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!((incident["reached"].as_str(), incident["model_calls"].as_f64(), incident["model_hours"].as_f64()), (Some("model_calls"), Some(THRESHOLD_CALLS), Some(30.0)));
    assert_eq!((watch.tally.model_hours(), watch.tally.model_calls()), (0.0, 0.0), "one trigger restarts both counters");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn the_prompt_names_a_route_that_exists() {
    let text = prompt(Path::new("/h"), "81", 100.0, 9000.0, "model_hours");
    assert!(text.contains("MANDATORY before analysis or edits") && text.contains("retro-recall 81"));
    assert!(text.contains("route runtime-retro") && text.contains("monitor-ack 81") && !text.contains('\n'));
    let skill = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".codex/skills/watchdog");
    if skill.exists() {
        assert!(skill.join("references/runtime-retro.md").exists());
        assert!(std::fs::read_to_string(skill.join("SKILL.md")).unwrap().contains("`runtime-retro`"));
    }
}

/// Calibration against the real transcripts: `cargo test -p cpu-watchdog calibrate -- --ignored --nocapture`.
#[test]
#[ignore]
fn calibrate_on_this_host() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let from = timestamp_ms(&std::env::var("RUNTIME_FROM").unwrap_or("2026-10-14T00:00:00.000Z".into())).unwrap();
    let started = std::time::Instant::now();
    let (files, tally) = parse::scan(&home, BTreeMap::new(), from, 0.0, crate::unix_now() * 1000.0);
    let watch = RuntimeRetro { files, tally, ..RuntimeRetro::default() };
    println!("{:.1} model hours, {} files, {:?}", watch.tally.model_hours(), watch.files.len(), started.elapsed());
    println!("{}", serde_json::to_string_pretty(&watch.breakdown()).unwrap());
}
