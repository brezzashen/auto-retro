use super::*;
use super::super::runtime_retro::parse::timestamp_ms;

const T0: &str = "2026-10-28T01:00:00.000Z";

fn at(secs: f64) -> String {
    let s = timestamp_ms(T0).unwrap() / 1000.0 + secs;
    let rem = s % 86_400.0;
    format!("2026-10-28T{:02}:{:02}:{:06.3}Z", (rem / 3600.0) as u32, ((rem % 3600.0) / 60.0) as u32, rem % 60.0)
}

fn tool(secs: f64, blocks: Value) -> Value {
    json!({"type": "assistant", "timestamp": at(secs), "cwd": "/r", "message": {"model": "claude-opus-5-5", "content": blocks}})
}

fn result(secs: f64) -> Value {
    json!({"type": "user", "timestamp": at(secs), "message": {"content": [{"type": "tool_result"}]}})
}

fn bash(id: &str, command: &str) -> Value {
    json!([{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}])
}

#[test]
fn blocked_spans_count_once_and_skip_reads_questions_and_short_calls() {
    let mut c = Cursor::new("cc5", false, Path::new("/p/x.jsonl"));
    for line in [
        // Parallel Read + Bash: one 600 s wait charged to the Bash call.
        tool(0.0, json!([{"type": "tool_use", "id": "r", "name": "Read"}, {"type": "tool_use", "id": "b1", "name": "Bash", "input": {"command": "cpu-build-gate run -- cargo test"}}])),
        result(600.0), result(601.0),
        // A question to the user is not a wait.
        tool(610.0, json!([{"type": "tool_use", "id": "q", "name": "AskUserQuestion"}])), result(4000.0),
        // Under MIN_MS: ordinary tool work.
        tool(4010.0, bash("b2", "ls")), result(4015.0),
        tool(4020.0, bash("b3", "until job-bus wait --job x; do sleep 30; done")), result(5820.0),
        tool(5830.0, bash("b4", "cd /r && bun run deliver canary")), result(6430.0),
    ] { c.feed(&line); }
    assert_eq!(c.wait_ms, 3_000_000.0);
    assert_eq!(c.waits, 3.0);
    assert_eq!(c.classes["build_test"], 600_000.0);
    assert_eq!(c.classes["polling"], 1_800_000.0);
    assert_eq!(c.classes["release_deploy"], 600_000.0);
    assert_eq!((c.top[0].call.as_str(), c.top[0].seconds), ("b3", 1800.0));
    assert_eq!(c.cwd, "/r");
}

#[test]
fn codex_polls_and_classes() {
    let mut c = Cursor::new("co1", true, Path::new("/p/rollout-1.jsonl"));
    for line in [
        json!({"type": "response_item", "timestamp": at(0.0), "payload": {"type": "custom_tool_call", "name": "exec", "call_id": "c1",
            "input": "text(await tools.write_stdin({session_id: 3, chars: \"\"}))"}}),
        json!({"type": "response_item", "timestamp": at(120.0), "payload": {"type": "custom_tool_call_output"}}),
    ] { c.feed(&line); }
    assert_eq!(c.classes["polling"], 120_000.0);
    assert_eq!(parse::class_of("Bash:bun run release", "bun run release recover-disk"), "release_deploy");
    assert_eq!(parse::class_of("Bash:sim-lease", "sim-lease acquire"), "queue_lease");
    assert_eq!(parse::class_of("Bash:cat", "cat > f.sh <<EOF\nwhile read l; do AB type #e \"$l\"; done\nEOF\nLEASE=$(browser-lease 9225) f.sh"), "browser");
    assert_eq!(parse::class_of("Bash:bun", "bun .claude/skills/product-listing/scripts/lease-page.js --site x --out /tmp/x s.js"), "browser");
    // Label from a CSV-building loop ahead of the run (78e5699b, wait retro 85883150729621603936).
    assert_eq!(parse::class_of("Bash:while", "U=u; { head -1 a.csv; while read -r n; do echo $n; done; } > b.csv\nbun .claude/skills/agent-browser/scripts/lease-page.js --label ads-bulk --out /tmp/d b11.js"), "browser");
    assert_eq!(parse::class_of("Bash:while", "while ! test -f done; do sleep 5; done"), "polling");
    assert_eq!(parse::class_of("Bash:browser-lease", "browser-lease 9225 --label x"), "queue_lease");
    assert_eq!(parse::class_of("Bash:ssh", "ssh win dir"), "network");
    assert_eq!(parse::class_of("Bash:ls", "ls"), "other");
    assert_eq!(parse::class_of("Bash:cpu-build-gate", "cpu-build-gate run -- cargo build --release -p x"), "build_test");
    // Running a built tool from cargo's release output is the tool's work, not a release (incident 814852170675806225103).
    assert_eq!(parse::class_of("Bash:-e", "U=x; $HOME/Library/Caches/t2/release/mars-dev-tools import:stage --user $U"), "other");
    assert_eq!(parse::class_of("Bash:mkdir", "mkdir -p /v; cpu-build-gate run -- cargo build --release -p x; ls $T/release/x"), "build_test");
}

#[test]
fn a_build_chained_past_the_excerpt_still_classes_as_build() {
    let mut c = Cursor::new("cc1", false, Path::new("/p/s.jsonl"));
    let long = format!("cd /w; perl -0pi -e '{}' doc.md; cpu-build-gate run -- cargo test -p mars-core", "s/a/b/;".repeat(60));
    c.feed(&tool(0.0, bash("b1", &long)));
    c.feed(&result(600.0));
    assert_eq!(c.classes["build_test"], 600_000.0);
    assert!(c.top[0].command.chars().count() <= 240);
}

fn waiting(minutes: f64) -> Cursor {
    let mut c = Cursor::new("cc5", false, Path::new("/p/a.jsonl"));
    let mut t = 0.0;
    while c.wait_ms < minutes * 60_000.0 {
        c.feed(&tool(t, bash(&format!("b{t}"), "sim-lease acquire --wait")));
        c.feed(&result(t + 1200.0));
        t += 1210.0;
    }
    c
}

#[test]
fn a_session_over_the_threshold_opens_one_retro_and_restarts_its_count() {
    let home = crate::telemetry::test_home("wait-retro");
    let mut retro = WaitRetro { loaded: true, ..Default::default() };
    let mut files = BTreeMap::new();
    files.insert("/p/short.jsonl".to_string(), waiting(40.0));
    let id = retro.absorb(&home, Scan { at: 1.0, files: files.clone() }).unwrap();
    assert!(id.is_none() && retro.dispatches.is_empty(), "40 minutes stays under the threshold");
    files.insert("/p/long.jsonl".to_string(), waiting(THRESHOLD_MIN));
    let id = retro.absorb(&home, Scan { at: 2.0, files }).unwrap().expect("the long session opens a retro");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["trigger"], "agent_wait_reached");
    assert_eq!(incident["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(incident["sessions"][0]["path"], "/p/long.jsonl");
    assert!(incident["sessions"][0]["top"].as_array().is_some_and(|t| !t.is_empty()));
    assert!(incident["queues"]["build_gate"].is_object());
    assert_eq!(retro.files["/p/long.jsonl"].wait_ms, 0.0, "a reviewed session counts again from zero");
    assert_eq!(retro.files["/p/short.jsonl"].wait_ms, 40.0 * 60_000.0);
    // While the incident is unacknowledged a second crossing only waits.
    let mut files = retro.files.clone();
    files.insert("/p/third.jsonl".to_string(), waiting(THRESHOLD_MIN));
    assert_eq!(retro.absorb(&home, Scan { at: 3.0, files }).unwrap(), Some(id.clone()));
    assert_eq!(retro.dispatches.len(), 1);
    assert!(retro.files["/p/third.jsonl"].flagged);
}

#[test]
fn profile_copies_of_one_session_count_once_and_reset_together() {
    let home = crate::telemetry::test_home("wait-retro-copies");
    let mut retro = WaitRetro { loaded: true, ..Default::default() };
    let copy = |minutes: f64, mtime: f64| { let mut c = waiting(minutes); c.mtime_ms = mtime; c };
    let mut files = BTreeMap::new();
    files.insert("/h/team/projects/P/s1.jsonl".to_string(), copy(THRESHOLD_MIN, 1.0));
    files.insert("/h/team3/projects/P/s1.jsonl".to_string(), copy(THRESHOLD_MIN + 40.0, 2.0));
    files.insert("/h/team2/projects/P/s1.jsonl".to_string(), copy(THRESHOLD_MIN + 60.0, 3.0));
    let id = retro.absorb(&home, Scan { at: 1.0, files }).unwrap().expect("one retro");
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["sessions"].as_array().unwrap().len(), 1, "three profile copies are one session");
    assert_eq!(incident["sessions"][0]["path"], "/h/team2/projects/P/s1.jsonl", "the copy written last");
    assert!(retro.files.values().all(|c| c.wait_ms == 0.0 && !c.flagged), "every copy restarts from zero");
}

/// Wait retro 86145098471064008529: 4.8 MB of cursors were refused at every restart, old transcripts
/// were read again from 0 and nine reviewed sessions came back with identical totals.
#[test]
fn cursors_past_4_mib_survive_a_restart_and_lost_ones_restart_at_the_end() {
    let home = crate::telemetry::test_home("wait-retro-restart");
    let mut files = BTreeMap::new();
    for i in 0..1200 { let mut c = waiting(30.0); (c.offset, c.cwd) = (7, "/w".repeat(2000)); files.insert(format!("/p/s{i}.jsonl"), c); }
    write_json(&root(&home).join("wait-retro-cursors.json"), &files).unwrap();
    assert!(std::fs::metadata(root(&home).join("wait-retro-cursors.json")).unwrap().len() > 4 * 1024 * 1024);
    let mut retro = WaitRetro::default();
    retro.load(&home, 1.0);
    assert_eq!((retro.files.len(), retro.load_error.as_deref()), (1200, None));
    // A reviewed transcript whose cursor is gone starts at its end, not at 0.
    let dir = home.join(".claude/projects/P");
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = [tool(0.0, bash("b1", "sim-lease acquire")), result(1200.0)].iter().map(|v| format!("{v}\n")).collect();
    std::fs::write(dir.join("old.jsonl"), &body).unwrap();
    let now = crate::unix_now() * 1000.0;
    let lost = parse::scan(&home, BTreeMap::new(), 0.0, now + 600_000.0, now + 600_000.0);
    let old = lost.values().next().expect("the transcript is tracked");
    assert_eq!((old.wait_ms, old.offset), (0.0, body.len() as u64));
    let fresh = parse::scan(&home, BTreeMap::new(), 0.0, now - 600_000.0, now);
    assert_eq!(fresh.values().next().unwrap().wait_ms, 1_200_000.0, "a transcript created after the last scan is read whole");
}

#[test]
fn an_acknowledged_retro_with_the_same_leader_does_not_open_again() {
    let home = crate::telemetry::test_home("wait-retro-acked");
    let mut retro = WaitRetro { loaded: true, ..Default::default() };
    let mut files = BTreeMap::new();
    files.insert("/p/long.jsonl".to_string(), waiting(THRESHOLD_MIN));
    let id = retro.absorb(&home, Scan { at: 1.0, files: files.clone() }).unwrap().expect("first retro");
    std::fs::write(root(&home).join("incidents").join(format!("{id}.ack")), "").unwrap();
    assert_eq!(retro.absorb(&home, Scan { at: 2.0, files }).unwrap(), None);
    assert_eq!(retro.dispatches.len(), 1, "no second session for the same review");
    assert_eq!(retro.files["/p/long.jsonl"].wait_ms, 0.0);
}

#[test]
fn ack_gate_needs_session_verdicts_the_three_parts_and_the_ledger_row() {
    let home = crate::telemetry::test_home("wait-retro-ack");
    let id = "42";
    let incident = home.join("incident.json");
    std::fs::write(&incident, json!({"trigger": "agent_wait_reached", "requirements": {"wait_review": 1}, "sessions": [{"path": "/p/long.jsonl"}]}).to_string()).unwrap();
    let dir = root(&home).join("analysis").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let ledger = home.join("ledger.md");
    std::fs::write(&ledger, "| 2026-10-28 | wait-retro 42 | --queue=Q3 typo |").unwrap();
    let mut report = json!({"incident": id, "sessions": [{"path": "/p/long.jsonl", "verdict": "input_error", "cause": "wrong queue"}],
        "input_errors": [{"input": "--queue=Q3", "cost": "one rebuild, 14 min", "guard": "deliver rejects unknown queues"}],
        "infra_ledger": {"path": ledger},
        "queues": [{"mechanism": "sim-lease", "finding": "unreleased", "evidence": "lease expired 40 min", "change": "expiry sweep"}],
        "scripts": [], "scripts_none_reason": "no repeated chain"});
    std::fs::write(dir.join("report.md"), "## 信息填写\n## 队列机制\n## 脚本优化\n").unwrap();
    let write = |r: &Value| std::fs::write(dir.join("report.json"), r.to_string()).unwrap();
    write(&report);
    validate_ack(&home, id, &incident).unwrap();
    report["sessions"] = json!([]);
    write(&report);
    assert!(validate_ack(&home, id, &incident).unwrap_err().to_string().contains("no verdict"));
    report["sessions"] = json!([{"path": "/p/long.jsonl", "verdict": "stuck", "cause": "c"}]);
    std::fs::write(&ledger, "no row").unwrap();
    write(&report);
    assert!(validate_ack(&home, id, &incident).unwrap_err().to_string().contains("wait ledger"));
    std::fs::write(&ledger, "row 42").unwrap();
    report["queues"] = json!([]);
    write(&report);
    assert!(validate_ack(&home, id, &incident).is_err());
}

/// Calibration against the real transcripts: `cargo test -p cpu-watchdog wait_calibrate -- --ignored --nocapture`.
#[test]
#[ignore]
fn wait_calibrate_on_this_host() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    cli(&home, vec!["--hours".into(), std::env::var("WAIT_HOURS").unwrap_or("24".into())]).unwrap();
}

fn bridged(secs: f64, created: f64, completed: Option<f64>) -> Value {
    // Bridged results carry the tool's own stamps in local time (+08:00).
    let local = |s: f64| at(s + 8.0 * 3600.0).trim_end_matches('Z').to_string() + "+08:00";
    let head = match completed { Some(c) => format!("Created At: {}\nCompleted At: {}\nCommand exited with code 0.", local(created), local(c)),
        None => format!("Created At: {}\nTool is running as a background task with task id: x/task-1", local(created)) };
    json!({"type": "user", "timestamp": at(secs), "message": {"content": [{"type": "tool_result", "content": head}]}})
}

#[test]
fn bridged_antigravity_steps_count_their_stamped_tool_time_not_model_time() {
    let path = Path::new("/h/.claude/projects/-Users-u-antigravity-orca-bridge/s.jsonl");
    let mut c = Cursor::new("cc1", false, path);
    for line in [
        // Planner step starts at 0; the model writes for 90 s, then a 1 s read_file runs: no wait.
        tool(0.0, json!([{"type": "tool_use", "id": "a", "name": "read_file", "input": {"Path": "/x"}}])), bridged(90.0, 90.0, Some(91.0)),
        // Parallel read_file (1 s) and Bash (40 s) on separate result lines: one 40 s wait on the Bash.
        tool(100.0, json!([{"type": "tool_use", "id": "r", "name": "read_file", "input": {}}, {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "cpu-build-gate run -- cargo test"}}])),
        bridged(160.0, 160.0, Some(161.0)), bridged(160.0, 160.0, Some(200.0)),
        // A background task returns at once and is not charged; nor is model time before it.
        tool(210.0, bash("bg", "bun run deliver canary")), bridged(300.0, 300.0, None),
        tool(310.0, bash("z", "ls")),
    ] { c.feed(&line); }
    assert_eq!(c.waits, 1.0);
    assert_eq!(c.wait_ms, 40_000.0);
    assert_eq!((c.top[0].call.as_str(), c.classes["build_test"]), ("b", 40_000.0));
    assert_eq!(parse::bridge_ms_for_test("2026-11-11T01:18:48+08:00"), timestamp_ms("2026-11-10T17:18:48Z"));
}

#[test]
fn bridged_cursors_saved_with_model_time_start_again_once() {
    let home = crate::telemetry::test_home("wait-retro-bridge-reset");
    let dir = home.join(".claude/projects/-Users-u-antigravity-orca-bridge");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(&path, "").unwrap();
    let key = path.to_string_lossy().into_owned();
    let mut old: Cursor = serde_json::from_value(json!({"offset": 0, "alias": "cc1", "sub": false, "codex": false, "mtime_ms": 0.0, "cwd": "",
        "wait_ms": 90.0 * 60_000.0, "waits": 50.0, "first_ms": 1.0, "last_ms": 2.0, "classes": {"other": 5_400_000.0}, "top": [], "flagged": false,
        "last_output": 0.0, "calls": []})).unwrap();
    old.offset = 0;
    let now = crate::unix_now() * 1000.0;
    let files = parse::scan(&home, BTreeMap::from([(key.clone(), old)]), 0.0, 0.0, now);
    assert_eq!(files[&key].wait_ms, 0.0, "the old model-time total is dropped");
    let saved = serde_json::to_value(&files[&key]).unwrap();
    let mut again: Cursor = serde_json::from_value(saved).unwrap();
    again.wait_ms = 60_000.0;
    let files = parse::scan(&home, BTreeMap::from([(key.clone(), again)]), 0.0, 0.0, now);
    assert_eq!(files[&key].wait_ms, 60_000.0, "a stamped total is kept");
}
