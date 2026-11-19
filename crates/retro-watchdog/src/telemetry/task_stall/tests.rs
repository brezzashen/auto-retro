use super::*;
use super::super::runtime_retro::parse::timestamp_ms;

const T0: &str = "2026-10-29T01:00:00.000Z";

struct Home(std::path::PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("watchdog-task-stall-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }
    fn path(&self) -> &Path { &self.0 }
}

impl Drop for Home {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn ms(secs: f64) -> f64 { timestamp_ms(T0).unwrap() + secs * 1000.0 }

fn at(secs: f64) -> String {
    let s = ms(secs) / 1000.0;
    let rem = s % 86_400.0;
    format!("2026-10-29T{:02}:{:02}:{:06.3}Z", (rem / 3600.0) as u32, ((rem % 3600.0) / 60.0) as u32, rem % 60.0)
}

fn prompt(secs: f64, text: &str) -> Value { json!({"type": "user", "timestamp": at(secs), "cwd": "/r", "message": {"content": text}}) }

fn call(secs: f64, id: &str, name: &str, command: &str) -> Value {
    json!({"type": "assistant", "timestamp": at(secs), "message": {"stop_reason": "tool_use", "content": [{"type": "tool_use", "id": id, "name": name, "input": {"command": command}}]}})
}

fn result(secs: f64, id: &str, error: bool) -> Value {
    json!({"type": "user", "timestamp": at(secs), "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": error}]}})
}

fn done(secs: f64) -> Value {
    json!({"type": "assistant", "timestamp": at(secs), "message": {"stop_reason": "end_turn", "content": [{"type": "text", "text": "ok"}]}})
}

fn cursor(lines: &[Value]) -> TaskCursor {
    let mut c = TaskCursor::new("cc6", false, false);
    for line in lines { c.feed(line); }
    c.mtime_ms = c.last_ms;
    c
}

fn kinds(c: &TaskCursor, now_secs: f64) -> Vec<String> {
    let home = Home::new("kinds");
    rows::stalls(home.path(), "s-test", c, ms(now_secs), &BTreeMap::new()).iter().map(|s| s["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_short_or_finished_task_is_not_a_stall() {
    let c = cursor(&[prompt(0.0, "fix it"), call(10.0, "a", "Bash", "job-bus wait --job x"), result(20.0, "a", false), done(30.0)]);
    assert!(!c.open());
    assert!(kinds(&c, 3_600.0).is_empty());
    let c = cursor(&[prompt(0.0, "fix it"), call(10.0, "a", "Bash", "job-bus wait --job x")]);
    assert!(kinds(&c, 20.0 * 60.0).is_empty(), "under TASK_MIN");
}

#[test]
fn a_call_blocked_past_block_min_in_a_long_task_stalls() {
    let c = cursor(&[prompt(0.0, "ship it"), call(10.0, "a", "Bash", "ls"), result(20.0, "a", false),
        call(15.0 * 60.0, "b", "Bash", "job-bus wait --job 20261028T034118Z-canary-38207 --timeout-min 60")]);
    assert_eq!(c.prompt, "ship it");
    assert!(kinds(&c, 29.0 * 60.0).is_empty(), "the task is under TASK_MIN");
    assert_eq!(kinds(&c, 31.0 * 60.0), vec!["blocked_call"], "blocked 16 min; a pending call is not also silent");
}

#[test]
fn a_long_task_waiting_ten_minutes_stalls_and_one_waiting_less_passes() {
    // Founder, 2026-11-02: open 30+ min and waiting 10+ min -> analyse; otherwise pass.
    let c = cursor(&[prompt(0.0, "ship it"), call(25.0 * 60.0, "b", "Bash", "cpu-build-gate run -- cargo build")]);
    assert!(kinds(&c, 34.0 * 60.0).is_empty(), "waiting 9 min passes");
    assert_eq!(kinds(&c, 35.5 * 60.0), vec!["blocked_call"], "waiting 10.5 min");
    let c = cursor(&[prompt(0.0, "plan"), call(60.0, "a", "Bash", "ls"), result(25.0 * 60.0, "a", false)]);
    assert!(kinds(&c, 34.0 * 60.0).is_empty(), "quiet 9 min passes");
    assert_eq!(kinds(&c, 35.5 * 60.0), vec!["silent"], "quiet 10.5 min");
}

#[test]
fn questions_and_subagents_are_not_blocked_calls_and_a_quiet_open_task_is_silent() {
    let c = cursor(&[prompt(0.0, "plan"), call(60.0, "q", "AskUserQuestion", "")]);
    assert!(kinds(&c, 40.0 * 60.0).is_empty());
    let c = cursor(&[prompt(0.0, "plan"), call(60.0, "a", "Bash", "ls"), result(70.0, "a", false)]);
    assert_eq!(kinds(&c, 40.0 * 60.0), vec!["silent"]);
}

#[test]
fn a_codex_task_waiting_on_an_async_question_is_not_silent_until_it_moves_on() {
    // Incident 82196204334637140999: request_user_input_async returns {"accepted":true} at once.
    let item = |secs: f64, payload: Value| json!({"type": "response_item", "timestamp": at(secs), "payload": payload});
    let started = json!({"type": "event_msg", "timestamp": at(0.0), "payload": {"type": "task_started"}});
    let ask = item(60.0, json!({"type": "function_call", "name": "request_user_input_async", "call_id": "q", "arguments": "{}"}));
    let accepted = item(61.0, json!({"type": "function_call_output", "call_id": "q", "output": "{\"accepted\":true}"}));
    let reasoning = item(66.0, json!({"type": "reasoning"}));
    let mut c = TaskCursor::new("co1", true, false);
    for line in [&started, &ask, &accepted, &reasoning] { c.feed(line); }
    c.mtime_ms = c.last_ms;
    assert!(c.open() && c.pending.is_empty());
    assert!(kinds(&c, 40.0 * 60.0).is_empty(), "waits on the user's answer");
    for line in [item(120.0, json!({"type": "function_call", "name": "exec", "call_id": "e", "arguments": "{}"})),
                 item(130.0, json!({"type": "function_call_output", "call_id": "e", "output": "ok"}))] { c.feed(&line); }
    c.mtime_ms = c.last_ms;
    assert_eq!(kinds(&c, 40.0 * 60.0), vec!["silent"], "a later call ends the question wait");
}

#[test]
fn the_same_failing_command_three_times_is_a_repeat_failure_and_a_new_prompt_resets_it() {
    let mut lines = vec![prompt(0.0, "deploy")];
    for i in 0..3 {
        let t = 60.0 + i as f64 * 600.0;
        lines.push(call(t, &format!("c{i}"), "Bash", &"bun run deliver canary --purpose x"));
        lines.push(result(t + 5.0, &format!("c{i}"), true));
    }
    lines.push(call(1_900.0, "ok", "Bash", "ls"));
    lines.push(result(1_901.0, "ok", false));
    let c = cursor(&lines);
    assert_eq!(kinds(&c, 32.0 * 60.0), vec!["repeat_failure"]);
    lines.push(prompt(1_950.0, "try again"));
    assert!(cursor(&lines).failures.is_empty());
}

#[test]
fn different_commands_behind_one_shared_preamble_are_not_a_repeat_failure() {
    let pre = "S=/private/tmp/claude-501/-Volumes-MarsWork-Workspaces-Mars-agent/2add1ea8/scratchpad\nLEASE=$(cat $S/lease.txt)\n";
    let run = |cores: [&str; 3]| {
        let mut lines = vec![prompt(0.0, "submit")];
        for (i, core) in cores.iter().enumerate() {
            let t = 60.0 + i as f64 * 60.0;
            lines.push(call(t, &format!("c{i}"), "Bash", &format!("{pre}{core}")));
            lines.push(result(t + 5.0, &format!("c{i}"), true));
        }
        kinds(&cursor(&lines), 32.0 * 60.0).contains(&"repeat_failure".to_string())
    };
    assert!(!run(["ab eval 'quill'", "ab eval 'checkbox'", "ab screenshot $S/a.jpg"]));
    assert!(run(["ab screenshot $S/a.jpg"; 3]));
}

#[test]
fn a_bus_orphan_is_closed_from_its_board_or_dead_pids_and_a_late_verdict_keeps_it_finished() {
    let home = Home::new("heal");
    let state = home.path().join(".local/state");
    std::fs::create_dir_all(state.join("job-bus")).unwrap();
    std::fs::create_dir_all(state.join("mars-deliver/releases")).unwrap();
    let old = ms(0.0);
    let lines = [
        json!({"ms": old, "topic": "release", "job": "r-done", "event": "finished", "pid": 1}),
        json!({"ms": old, "topic": "release", "job": "r-done", "event": "verdict", "pid": 1}),
        json!({"ms": old, "topic": "release", "job": "r-closed", "event": "progress", "pid": 1}),
        json!({"ms": old, "topic": "release", "job": "r-running", "event": "progress", "pid": 1}),
        json!({"ms": old, "topic": "offload", "job": "docker/1", "event": "admitted", "pid": 999_999_999}),
        json!({"ms": old, "topic": "offload", "job": "docker/2", "event": "admitted", "pid": std::process::id()}),
    ];
    std::fs::write(state.join("job-bus/events.jsonl"), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    std::fs::write(state.join("mars-deliver/releases/r-closed.json"), r#"{"state":"failed"}"#).unwrap();
    std::fs::write(state.join("mars-deliver/releases/r-running.json"), r#"{"state":"running"}"#).unwrap();
    let healed = heal::heal_bus(home.path(), ms(31.0 * 60.0));
    let got: Vec<(String, String)> = healed.iter().map(|h| (h["job"].as_str().unwrap().into(), h["emitted"].as_str().unwrap().into())).collect();
    assert_eq!(got, vec![("docker/1".into(), "cancelled".into()), ("r-closed".into(), "failed".into())]);
    assert!(heal::heal_bus(home.path(), ms(10.0 * 60.0)).is_empty(), "younger than ORPHAN_MIN");
    let bus = heal::jobs(&state.join("job-bus/events.jsonl"));
    let w = heal::waits_on(home.path(), "job-bus wait --job r-done --timeout-min 9", &bus);
    assert_eq!((w["state"].as_str(), w["ended"].as_bool()), (Some("finished"), Some(true)));
}

#[test]
fn escalation_reports_each_stalled_task_once() {
    let home = Home::new("escalate");
    std::fs::create_dir_all(root(home.path()).join("incidents")).unwrap();
    let c = cursor(&[prompt(0.0, "ship"), call(60.0, "a", "Bash", "bun run deliver canary")]);
    let now = ms(40.0 * 60.0);
    let mut files = BTreeMap::new();
    files.insert("/p/a.jsonl".to_string(), c);
    let rows = vec![rows::row("/p/a.jsonl", &files["/p/a.jsonl"], now, rows::stalls(home.path(), "s-test", &files["/p/a.jsonl"], now, &BTreeMap::new()))];
    let mut watch = TaskStall { loaded: true, ..TaskStall::default() };
    watch.absorb(home.path(), Scan { at: now, files: files.clone(), heals: vec![], rows: rows.clone() }).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    std::fs::write(root(home.path()).join("incidents").join(format!("{id}.ack")), "").unwrap();
    let files = watch.files.clone();
    watch.absorb(home.path(), Scan { at: now + 1.0, files, heals: vec![], rows }).unwrap();
    assert_eq!(watch.dispatches.len(), 1, "the same task start is not reported twice");
}

#[test]
fn a_new_stalled_task_is_dispatched_while_an_earlier_incident_is_unacknowledged() {
    let home = Home::new("escalate-parallel");
    std::fs::create_dir_all(root(home.path()).join("incidents")).unwrap();
    let now = ms(40.0 * 60.0);
    let mut files = BTreeMap::new();
    files.insert("/p/a.jsonl".to_string(), cursor(&[prompt(0.0, "ship"), call(60.0, "a", "Bash", "bun run deliver canary")]));
    files.insert("/p/b.jsonl".to_string(), cursor(&[prompt(0.0, "build"), call(60.0, "b", "Bash", "xcodebuild")]));
    let row = |p: &str, f: &BTreeMap<String, TaskCursor>| rows::row(p, &f[p], now, rows::stalls(home.path(), "s-test", &f[p], now, &BTreeMap::new()));
    let mut watch = TaskStall { loaded: true, ..TaskStall::default() };
    let first = vec![row("/p/a.jsonl", &files)];
    watch.absorb(home.path(), Scan { at: now, files: files.clone(), heals: vec![], rows: first }).unwrap();
    let files = watch.files.clone();
    let both = vec![row("/p/a.jsonl", &files), row("/p/b.jsonl", &files)];
    watch.absorb(home.path(), Scan { at: now + 1.0, files, heals: vec![], rows: both }).unwrap();
    assert_eq!(watch.dispatches.len(), 2, "no wait for the first incident's ack");
    assert_eq!(watch.dispatches[1]["summary"]["tasks"], 1, "only the new task");
}

fn limit(secs: f64) -> Value {
    json!({"type": "assistant", "timestamp": at(secs), "message": {"model": "<synthetic>", "stop_reason": "stop_sequence",
        "content": [{"type": "text", "text": "You've hit your weekly limit · resets Nov 1 at 3am"}]}})
}

#[test]
fn a_backstop_job_ending_on_limit_replies_for_task_min_is_quota_blocked() {
    let run = |secs: f64| cursor(&[prompt(secs, "You are the residue backstop for the Mars Agent repository"), limit(secs + 2.0)]);
    let first = run(0.0);
    assert!(!first.open(), "a synthetic limit reply hands the turn back");
    assert_eq!(first.limit_ms, ms(2.0));
    let mut files = BTreeMap::new();
    files.insert("/p/a.jsonl".to_string(), first);
    files.insert("/p/b.jsonl".to_string(), run(20.0 * 60.0));
    let home = Home::new("quota");
    assert!(rows::open_rows(home.path(), &files, ms(25.0 * 60.0), TASK_MIN).is_empty(), "two replies 20 min apart");
    files.insert("/p/c.jsonl".to_string(), run(40.0 * 60.0));
    let got = rows::open_rows(home.path(), &files, ms(41.0 * 60.0), TASK_MIN);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["stalls"][0]["kind"], "quota_blocked");
    assert_eq!(got[0]["stalls"][0]["hits"], 3);
    assert_eq!(got[0]["path"], "/p/c.jsonl");
    assert!(rows::escalating(&got[0]));
}

#[test]
fn a_compaction_summary_neither_opens_nor_restarts_a_task() {
    let summary = |secs: f64| json!({"type": "user", "isCompactSummary": true, "isVisibleInTranscriptOnly": true, "timestamp": at(secs),
        "message": {"content": "This session is being continued from a previous conversation that ran out of context."}});
    // Idle compaction after the turn ended (incident 815992744594064557055).
    let c = cursor(&[prompt(0.0, "ship it"), done(60.0), summary(55.0 * 60.0)]);
    assert!(!c.open());
    assert!(kinds(&c, 90.0 * 60.0).is_empty());
    // Auto-compaction inside a working turn keeps the task and its pending call.
    let c = cursor(&[prompt(0.0, "ship it"), call(60.0, "a", "Bash", "job-bus wait --job x"), summary(120.0)]);
    assert_eq!((c.task_ms, c.prompt.as_str(), c.pending.len()), (ms(0.0), "ship it", 1));
}

#[test]
fn a_transcript_only_notification_neither_opens_nor_restarts_a_task() {
    // Incident 88333634518963469813: on resume Claude Code recorded a stale background-task
    // notification with `queueTranscriptOnly`; no turn ran, the session was done.
    let note = |secs: f64, only: bool| json!({"type": "user", "timestamp": at(secs), "promptSource": "system",
        "origin": {"kind": "task-notification"}, "queueTranscriptOnly": only,
        "message": {"role": "user", "content": "<task-notification> <status>stopped</status> </task-notification>"}});
    let c = cursor(&[prompt(0.0, "ship it"), done(60.0), note(15.0 * 60.0, true)]);
    assert!(!c.open());
    assert!(kinds(&c, 60.0 * 60.0).is_empty());
    // A live notification still starts a turn the model must answer.
    let c = cursor(&[prompt(0.0, "ship it"), done(60.0), note(15.0 * 60.0, false)]);
    assert!(c.open());
}

#[test]
fn a_codex_call_overrunning_its_own_clock_marks_the_turn_gone() {
    // Incident 811852163878791633152: a handoff child closed the co1 tab 0.25 s after it called
    // sleep {"duration_ms":8000}; the call stayed open in the rollout for 12+ min.
    let item = |secs: f64, payload: Value| json!({"type": "response_item", "timestamp": at(secs), "payload": payload});
    let started = json!({"type": "event_msg", "timestamp": at(0.0), "payload": {"type": "task_started"}});
    let sleep = item(60.0, json!({"type": "function_call", "name": "sleep", "call_id": "s", "arguments": "{\"duration_ms\":8000}"}));
    let exec = item(60.0, json!({"type": "custom_tool_call", "name": "exec", "call_id": "x", "input": "text(await tools.exec_command({cmd:'deliver'}))"}));
    let mut c = TaskCursor::new("co1", true, false);
    for line in [&started, &sleep] { c.feed(line); }
    assert!(!rows::codex_turn_gone(&c, ms(60.0 + 9.0 * 60.0)), "inside clock plus BLOCK_MIN");
    assert!(rows::codex_turn_gone(&c, ms(60.0 + 8.0 + 10.0 * 60.0)), "overran its own clock");
    let mut long = TaskCursor::new("co1", true, false);
    for line in [&started, &exec] { long.feed(line); }
    assert!(!rows::codex_turn_gone(&long, ms(3600.0)), "a call without its own clock may wait");
}

#[test]
fn liveness_reads_only_the_session_own_row() {
    // Incident 84611516150271090992: the closed parent b7a9586d ranked second behind its live handoff
    // child, whose prompt quoted the parent's scratchpad path.
    let answer = json!([
        {"id": "cd1d3980-child", "live": {"pid": 97156, "status": "busy", "name": "mars-agent-a9"}},
        {"id": "b7a9586d-parent", "live": null}]);
    assert_eq!(rows::live_of(&answer, "b7a9586d-parent"), (false, Value::Null));
    assert_eq!(rows::live_of(&answer, "cd1d3980-child"), (true, json!("mars-agent-a9")));
    let shell = json!({"id": "s", "live": {"status": "shell", "name": "t"}});
    assert!(!rows::live_of(&shell, "s").0);
    assert_eq!(rows::live_of(&json!([]), "missing"), (true, Value::Null));
}

#[test]
fn a_build_gate_call_queued_for_a_simulator_names_the_lease_holder() {
    // Incident 818084050210556831742: simRun --device <udid> waited 22 min behind another
    // session's lease; waits_on said only build_test.
    let home = Home::new("simq");
    let dir = home.path().join(".local/state/sim-lease");
    std::fs::create_dir_all(dir.join("queue")).unwrap();
    std::fs::create_dir_all(dir.join("leases")).unwrap();
    let now_s = ms(31.0 * 60.0) / 1000.0;
    std::fs::write(dir.join("queue/t1.json"), json!({"ticket": "t1", "pid": 1, "owner": "6bc58d81-x", "device": "57ac12d6",
        "family": null, "on_busy": "wait", "purpose": "free-qa"}).to_string()).unwrap();
    std::fs::write(dir.join("leases/57AC12D6.json"), json!({"udid": "57AC12D6", "owner": "b4a6bbcc-y", "purpose": "photos e2e",
        "expires": now_s + 3000.0}).to_string()).unwrap();
    let c = cursor(&[prompt(0.0, "ship"), call(20.0 * 60.0, "b", "Bash", "cpu-build-gate run -- bun run ui-tools ios simRun --device 57ac12d6")]);
    let s = rows::stalls(home.path(), "6bc58d81-x", &c, ms(31.0 * 60.0), &BTreeMap::new());
    let w = &s[0]["waits_on"];
    assert_eq!((w["kind"].as_str(), w["class"].as_str(), w["device"].as_str()), (Some("sim-lease"), Some("build_test"), Some("57AC12D6")));
    assert_eq!((w["holder"]["owner"].as_str(), w["holder"]["expires_in_s"].as_f64()), (Some("b4a6bbcc-y"), Some(3000.0)));
    let other = rows::stalls(home.path(), "someone-else", &c, ms(31.0 * 60.0), &BTreeMap::new());
    assert_eq!(other[0]["waits_on"]["kind"].as_str(), Some("build_test"), "another session's ticket is not this wait");
}

#[test]
fn a_transcript_path_maps_to_the_session_sim_lease_records() {
    assert_eq!(simq::session_of("/p/projects/r/6bc58d81-63c5.jsonl"), "6bc58d81-63c5");
    assert_eq!(simq::session_of("/p/projects/r/6bc58d81-63c5/subagents/agent-a1.jsonl"), "6bc58d81-63c5");
    assert_eq!(simq::session_of("/s/2026/11/04/rollout-2026-11-04T10-00-00-0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000.jsonl"),
        "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000");
}

#[test]
fn an_interruption_marker_closes_the_task_instead_of_opening_one() {
    // Incident 812669328284927824950: a feature-verifier subagent finished, was interrupted, and the
    // marker line read as a new prompt that stayed "silent" for 31 minutes.
    let c = cursor(&[prompt(0.0, "verify"), done(60.0), prompt(65.0, "[Request interrupted by user]")]);
    assert!(!c.open());
    assert!(kinds(&c, 45.0 * 60.0).is_empty());
    // Interrupting a pending tool call ends the turn and drops the call.
    let c = cursor(&[prompt(0.0, "ship it"), call(60.0, "a", "Bash", "job-bus wait --job x"),
        prompt(90.0, "[Request interrupted by user for tool use]")]);
    assert!(!c.open());
    assert!(c.pending.is_empty());
}
