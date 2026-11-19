use super::*;
use serde_json::json;

fn row(at: &str, body: Value) -> String {
    let mut row = body;
    row["timestamp"] = json!(format!("2026-11-10T01:{at}:00.000Z"));
    row.to_string()
}

fn injected(at: &str, note: &str) -> String {
    row(at, json!({"type": "attachment", "attachment": {"type": "hook_additional_context",
        "content": [format!("Shared memory recall (agent-memory; best matches):\n\n### {note} (0.61 · project/host · /m/{note}.md)\nbody")]}}))
}

fn call(at: &str, name: &str, input: Value) -> String {
    row(at, json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": name, "input": input}]}}))
}

fn result(at: &str, text: &str) -> String {
    row(at, json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": text}]}}))
}

fn transcript(home: &Path, id: char, lines: &[String]) -> std::path::PathBuf {
    let dir = home.join(".claude/projects/p");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{0}{0}{0}{0}{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}.jsonl", id));
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

#[test]
fn census_requires_notes_and_skills_that_met_failures_in_two_sessions() {
    let home = crate::telemetry::test_home("memory-dream-sessions");
    for id in ['a', 'b'] {
        transcript(&home, id, &[json!({"type": "custom-title", "customTitle": format!("deploy {id}")}).to_string(),
            injected("01", "deploy-note"), call("02", "Skill", json!({"skill": "report"})), result("03", "loaded"),
            call("04", "Bash", json!({"command": "deliver beta"})), result("05", "Exit code 1\nrefused")]);
    }
    // One session only, and one that never failed: neither recurs.
    transcript(&home, 'c', &[injected("01", "solo-note"), call("02", "Read", json!({"file_path": "/r/.claude/skills/commit/SKILL.md"})), result("03", "ok")]);

    // A retro reads other sessions' failures; a result that only quotes an error is not one.
    transcript(&home, 'd', &[injected("01", "deploy-note"), call("02", "Bash", json!({"command": "cpu-watchdog retro-recall 1 x"})), result("03", "Exit code 1")]);
    transcript(&home, 'e', &[call("01", "Bash", json!({"command": "job-bus tail"})), result("02", "release failed: script exited with code 1")]);

    // The user approved one run; the first request, a pasted document or a notification is not a reaction.

    transcript(&home, 'f', &[row("00", json!({"type": "user", "message": {"content": "改一下首页文案"}})), call("01", "Bash", json!({"command": "build"})), result("02", "ok"),
        row("03", json!({"type": "user", "message": {"content": "很好，就按这个方式"}})),
        row("04", json!({"type": "user", "message": {"content": format!("{} 完美", "长文".repeat(80))}})),
        row("05", json!({"type": "user", "message": {"content": "<task-notification>perfect</task-notification>"}}))]);
    // Four delivered runs of one recurring task: one needed 2 calls, the others 5 or 6.
    for (id, calls) in [('1', 2), ('2', 5), ('3', 5), ('4', 6)] {
        let mut lines = vec![row("00", json!({"type": "user", "message": {"content": format!("SEO hourly retro for 2026-11-10 run {calls}")}}))];
        lines.extend((0..calls).map(|_| call("01", "Bash", json!({"command": "seo-watch status"}))));
        lines.push(row("09", json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "https://reports.example.com/seo/"}]}})));
        transcript(&home, id, &lines);
    }
    // An A/B harness run of the same prompt inside another session's scratchpad: 1 call, but not a task of its own.
    let harness: Vec<String> = [row("00", json!({"type": "user", "message": {"content": "SEO hourly retro for 2026-11-10 run 1"}})),
        call("01", "Bash", json!({"command": "seo-watch status"})),
        row("09", json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "https://reports.example.com/seo/"}]}}))]
        .iter().map(|l| { let mut v: Value = serde_json::from_str(l).unwrap(); v["cwd"] = json!("/private/tmp/claude-501/p/99999999-9999-9999-9999-999999999999/scratchpad/ab/new"); v.to_string() }).collect();
    // Its project directory carries the owner's id, as Claude Code names it after the working directory.
    let dir = home.join(".claude/projects/-private-tmp-claude-501-p-99999999-9999-9999-9999-999999999999-scratchpad-ab-new");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("66666666-6666-6666-6666-666666666666.jsonl"), harness.join("\n") + "\n").unwrap();
    // A session working in its own scratchpad is a task of its own.
    let mut own: Value = serde_json::from_str(&call("01", "Bash", json!({"command": "ls"}))).unwrap();
    own["cwd"] = json!("/private/tmp/claude-501/p/77777777-7777-7777-7777-777777777777/scratchpad");
    transcript(&home, '7', &[own.to_string()]);
    // A run still working after it named the link has not delivered, however few calls it made so far.
    transcript(&home, '5', &[row("00", json!({"type": "user", "message": {"content": "SEO hourly retro for 2026-11-10 run 1"}})),
        row("01", json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "https://reports.example.com/seo/"}]}})), call("02", "Bash", json!({"command": "seo-watch status"}))]);
    // Four canary releases succeeded: one in 3 minutes, the others in 7 or 8; a failed fast one does not count.
    let releases = home.join(".local/state/mars-deliver/releases");
    std::fs::create_dir_all(&releases).unwrap();
    // A 1-minute re-release of a live commit rolled nothing and does not count either; the typical one waited
    // beside 6 gated builds.
    let logs = [(3, "[cpu-build-gate] 4 compile tasks: 10 of 30 cores busy, 6 gated build(s)\napi: a built, will roll\n"),
        (6, "api: a built, unchanged\ncrm: b built, unchanged\n")];
    for (n, minutes, state) in [(1, 3, "succeeded"), (2, 7, "succeeded"), (3, 7, "succeeded"), (4, 8, "succeeded"), (5, 1, "failed"), (6, 1, "succeeded")] {
        let id = format!("20261110T010{n}00Z-canary-{n}");
        let mut run = json!({"id": id, "target": "canary", "state": state, "purpose": format!("release {n}"),
            "creator": {"session": "s"}, "startedAt": format!("2026-11-10T01:0{n}:00.000Z"), "finishedAt": format!("2026-11-10T01:{:02}:00.000Z", n + minutes)});
        if let Some((_, text)) = logs.iter().find(|(m, _)| *m == n) {
            let log = releases.join(format!("{id}.log"));
            std::fs::write(&log, text).unwrap();
            run["log"] = json!(log);
        }
        std::fs::write(releases.join(format!("{id}.json")), run.to_string()).unwrap();
    }
    // Five iOS releases: three waited 15 minutes in the foreground; a session's release returned after 4 minutes
    // while its publisher ran 16, and another's publisher is still running. Neither is a fast release.
    for (n, minutes, closed) in [(1, 15, None), (2, 15, None), (3, 15, None), (4, 4, Some(16)), (5, 4, Some(0))] {
        let id = format!("20261110T020{n}00Z-ios-{n}");
        let mut run = json!({"id": id, "target": "ios", "state": "succeeded", "purpose": format!("ios {n}"), "creator": {"session": "s"},
            "startedAt": format!("2026-11-10T02:0{n}:00.000Z"), "finishedAt": format!("2026-11-10T02:{:02}:00.000Z", n + minutes)});
        if let Some(after) = closed {
            run["backgroundPid"] = json!(1);
            if after > 0 { run["closed"] = json!({"at": format!("2026-11-10T02:{:02}:00.000Z", n + after)}); }
        }
        std::fs::write(releases.join(format!("{id}.json")), run.to_string()).unwrap();
    }
    // A 1-minute release-watch run found its commit already in an uploading build and published nothing
    // (dream 816633343410489013143: 20261112T122421Z-ios-15517, 63 s against a median of 986 s).
    let id = "20261110T020600Z-ios-6";
    let log = releases.join(format!("{id}.log"));
    std::fs::write(&log, "abc is in build 12043, which is uploading; waiting for that upload\nabc already ships in build 12043 (abc); nothing to publish\n").unwrap();
    let run = json!({"id": id, "target": "ios", "state": "succeeded", "purpose": "ios 6", "creator": {"session": "s"}, "log": log,
        "startedAt": "2026-11-10T02:06:00.000Z", "finishedAt": "2026-11-10T02:07:00.000Z"});
    std::fs::write(releases.join(format!("{id}.json")), run.to_string()).unwrap();

    let census = sessions::census(&home, 0.0, f64::MAX, &[]);
    assert_eq!(census["fast"].as_array().map(Vec::len), Some(1), "a release measured to its foreground return is not fast");
    assert_eq!(census["fast"][0]["group"], "fast:20261110T010100Z-canary-1");
    assert_eq!((census["fast"][0]["fast"]["seconds"].as_f64(), census["fast"][0]["median_seconds"].as_f64(), census["fast"][0]["runs"].as_u64()), (Some(180.0), Some(420.0), Some(4)));
    assert_eq!((census["fast"][0]["typical"]["gated_builds"].as_u64(), census["fast"][0]["busier_typical"].as_bool()), (Some(6), Some(true)));
    assert_eq!(census["harness_sessions"], 1, "the A/B harness run is left out of the families");
    assert_eq!(census["best"][0]["group"], "best:11111111");
    assert_eq!(census["best"][0]["family"], "SEO hourly retro for #-#-# run #");
    assert_eq!((census["best"][0]["runs"].as_u64(), census["best"][0]["median_calls"].as_u64(), census["best"][0]["costliest"]["calls"].as_u64()), (Some(4), Some(5), Some(6)));
    assert_eq!(census["praised"][0]["group"], "praise:ffffffff");
    assert_eq!(census["praised"][0]["praised"], 1);
    assert_eq!(census["praised"][0]["said"][0], "很好，就按这个方式");
    assert_eq!(census["sessions"], 11, "the retro session and the harness run are left out");
    assert_eq!(census["notes"][0]["sessions"], 2);
    assert_eq!(census["stuck"], json!([]));
    assert_eq!(census["notes"][0]["group"], "note:deploy-note");
    assert_eq!(census["notes"][0]["failing_sessions"], 2);
    assert_eq!(census["notes"][0]["rows"][0]["title"], "deploy a");
    assert!(census["notes"][0]["rows"][0]["failed_at"][0].as_str().unwrap().ends_with(".jsonl:6"));
    assert_eq!(census["required"], json!(["note:deploy-note", "skill:report", "best:11111111", "fast:20261110T010100Z-canary-1", "praise:ffffffff"]), "solo-note and the commit skill do not recur");
    assert_eq!(census["skills"][0]["clean_at"], json!([]), "both sessions failed after the load");

    // A lesson in the dream golden set is followed up for as long as its case exists, whatever the previous reports hold.
    let golden = home.join("automation/crates/agent-memory/eval");
    std::fs::create_dir_all(&golden).unwrap();
    std::fs::write(golden.join("golden-memory-dream.jsonl"), json!({"id": "g1", "set": "dream", "query": "deploy", "targets": ["deploy-note"]}).to_string() + "\n").unwrap();
    assert_eq!(sessions::census(&home, 0.0, f64::MAX, &[])["required"][0], "lesson:deploy-note");
    std::fs::remove_dir_all(home.join("automation")).unwrap();

    // An earlier dream's lesson is followed up instead; a group it audited is not required again.
    let previous = home.join("previous.json");
    std::fs::write(&previous, json!({"lessons": [{"note": "deploy-note"}], "audits": [{"group": "skill:report"}]}).to_string()).unwrap();
    let census = sessions::census(&home, 0.0, f64::MAX, &[json!({"report": previous})]);
    assert_eq!(census["required"], json!(["lesson:deploy-note", "best:11111111", "fast:20261110T010100Z-canary-1", "praise:ffffffff"]));
    assert_eq!(census["rotated"], json!(["skill:report"]));
    assert_eq!(census["skills"][0]["group"], "skill:report", "still listed");
}

#[test]
fn audit_gate_needs_every_required_group_and_recallable_lessons() {
    let home = crate::telemetry::test_home("memory-dream-audit");
    let (id, store) = ("md2", home.join(".agent-memory"));
    let dir = root(&home).join("analysis").join(id);
    for d in [&dir, &store, &root(&home).join("incidents")] { std::fs::create_dir_all(d).unwrap(); }
    let incident = root(&home).join("incidents").join(format!("{id}.json"));
    std::fs::write(&incident, json!({"trigger": "memory_dream_due", "contract": 2, "store": store}).to_string()).unwrap();
    let eval = json!({"sets": {"single": {"top5_pct": 94.7, "hook_injected_pct": 92.7}, "multi": {"coverage_top5_pct": 82.1}, "dream": {"top5_pct": 100.0}}});
    let scan = json!({"stats": {"broken_link": 0, "superseded_link": 0, "missing_path": 0, "near_duplicate": 9}});
    let w = |name: &str, v: &Value| std::fs::write(dir.join(name), v.to_string()).unwrap();
    for name in ["eval-before.json", "eval-after.json"] { w(name, &eval); }
    for name in ["scan-before.json", "scan-after.json"] { w(name, &scan); }
    let mut actions: serde_json::Map<String, Value> = validate::ACTIONS.iter().map(|k| (k.to_string(), json!(0))).collect();
    actions.insert("lessons_written".into(), json!(1));
    let line = format!("{}:2", transcript(&home, 'a', &[injected("01", "deploy-note"), call("02", "Bash", json!({"command": "x"}))]).display());
    let mut report = json!({"report_url": "https://reports.example.com/x/", "eval": {"before": eval["sets"], "after": eval["sets"]},
        "scan": {"before": scan["stats"], "after": scan["stats"]}, "actions": actions, "misses_reviewed": 0, "gaps": [], "hubs": [],
        "audits": [{"group": "skill:report", "verdict": "lesson", "lessons": ["memo-report"], "evidence": [line],
            "answers": {"missing_info": "none: the skill was loaded", "friction": "publish refused twice", "repetition": "2 sessions", "instruction_gap": "the skill does not say --encrypt needs a slug"}}],
        "lessons": [{"note": "memo-report", "kind": "gotcha", "query": "发布加密报告", "evidence": [line]}],
        "lesson_eval": {"top5_pct": 100.0, "hook_injected_pct": 100.0}});
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "no dream-sessions receipt");
    w("dream-sessions.json", &json!({"incident": id, "required": ["skill:report", "lesson:old-memo"]}));
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "the lesson follow-up is not answered");
    report["audits"].as_array_mut().unwrap().push(json!({"group": "lesson:old-memo", "verdict": "ignored", "why": "injected, the agent retried anyway", "evidence": [line]}));
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "ignored needs next");
    report["audits"][1]["next"] = json!("move the first step to the top of the memo");
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "the lesson note does not exist yet");
    std::fs::write(store.join("memo-report.md"), "memo").unwrap();
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "lessons were not measured");
    w("eval-lessons.json", &json!({"sets": {"dream": {"cases": 1, "top5_pct": 0.0, "hook_injected_pct": 0.0}}}));
    report["lesson_eval"] = json!({"top5_pct": 0.0, "hook_injected_pct": 0.0});
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "a lesson its own request cannot recall");
    w("eval-lessons.json", &json!({"sets": {"dream": {"cases": 1, "top5_pct": 100.0, "hook_injected_pct": 100.0}}}));
    report["lesson_eval"] = json!({"top5_pct": 100.0, "hook_injected_pct": 100.0});
    w("report.json", &report);
    validate::validate_ack(&home, id, &incident).unwrap();

    // A helpful lesson says where it was promoted; an approved run says what worked and whether it carries over.
    report["audits"][1]["verdict"] = json!("helped");
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "helped needs promoted");
    report["audits"][1]["promoted"] = json!("moved to the first line of memo-report");
    w("dream-sessions.json", &json!({"incident": id, "required": ["skill:report", "lesson:old-memo", "best:11111111"]}));
    report["audits"].as_array_mut().unwrap().push(json!({"group": "best:11111111", "verdict": "no_lesson", "why": "the cheap run had no new keywords to review", "evidence": [line],
        "answers": {"what_worked": "it read the day's state file once instead of every source"}}));
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "reusable is not answered");
    report["audits"][2]["answers"]["reusable"] = json!("no: the other runs had new data to read");
    w("report.json", &report);
    validate::validate_ack(&home, id, &incident).unwrap();

    // A lesson judged wrong is rewritten or retired in the same dream: an unchanged body or a verify stamp is refused.
    git(&store, &["init", "-q", "-b", "main"]).unwrap();
    std::fs::write(store.join("memo-old.md"), "---\nname: memo-old\n---\n\n- entry a\n- entry b\n").unwrap();
    git(&store, &["add", "-A"]).unwrap();
    git(&store, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "seed"]).unwrap();
    let head = git(&store, &["rev-parse", "HEAD"]).unwrap();
    std::fs::write(&incident, json!({"trigger": "memory_dream_due", "contract": 2, "store": store, "head": head}).to_string()).unwrap();
    w("dream-sessions.json", &json!({"incident": id, "required": ["lesson:memo-old"]}));
    report["audits"] = json!([{"group": "lesson:memo-old", "verdict": "wrong", "why": "entry b names a flag the script dropped", "next": "removed entry b", "evidence": [line]}]);
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "a memo verdict names its entry");
    report["audits"][0]["entry"] = json!("entry b");
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "the note is unchanged");
    std::fs::write(store.join("memo-old.md"), "---\nname: memo-old\nverified: 2026-11-10\n---\n\n- entry a\n- entry b\n").unwrap();
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "a verify stamp is not a change");
    std::fs::write(store.join("memo-old.md"), "---\nname: memo-old\n---\n\n- entry a\n").unwrap();
    validate::validate_ack(&home, id, &incident).unwrap();
    std::fs::remove_file(store.join("memo-old.md")).unwrap();
    validate::validate_ack(&home, id, &incident).unwrap();

    // A lesson that left the store is listed with its reason, or the golden set hides the loss.
    let mut after = eval.clone();
    after["sets"]["dream"]["gone"] = json!(1);
    w("eval-after.json", &after);
    assert!(validate::validate_ack(&home, id, &incident).is_err(), "gone grew without retired[]");
    report["retired"] = json!([{"note": "memo-old", "reason": "the script now rejects the flag with a clear message", "verdict": "obsolete"}]);
    w("report.json", &report);
    validate::validate_ack(&home, id, &incident).unwrap();
    w("eval-after.json", &eval);

    // A citation that names no real line, or an earlier lesson lost from recall, is refused.
    let mut after = eval.clone();
    after["sets"]["dream"]["top5_pct"] = json!(80.0);
    w("eval-after.json", &after);
    assert!(validate::validate_ack(&home, id, &incident).is_err());
    w("eval-after.json", &eval);
    report["lessons"][0]["evidence"] = json!(["/nowhere.jsonl:9"]);
    w("report.json", &report);
    assert!(validate::validate_ack(&home, id, &incident).is_err());
}
