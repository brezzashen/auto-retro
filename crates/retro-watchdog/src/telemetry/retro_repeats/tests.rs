use super::*;

#[test]
fn signatures_name_the_action_not_its_values() {
    assert_eq!(signatures("cd /a && git status --short | head -20"), ["git status"]);
    assert_eq!(signatures("bun .claude/skills/report/scripts/read.ts https://reports.example.com/x-20261025/"),
        ["url:reports.example.com", "bun read.ts"]);
    assert_eq!(signatures("FOO=1 bun scripts/spec.ts set --file - x"), ["bun spec.ts set"]);
    assert_eq!(signatures("sed -n 1,40p /Users/b/Mars_agent/.claude/skills/report/SKILL.md"), ["file:report/SKILL.md"]);
    assert_eq!(signatures(r#"{"file_path":"/w/other/report/SKILL.md"}"#), ["file:report/SKILL.md"]);
    assert!(signatures(r#"{"file_path":"/w/a/b.rs","old_string":"x","new_string":"y"}"#).is_empty());
    assert!(signatures(r#"{"file_path":"/w/a/b.rs","content":"fn main() {}"}"#).is_empty());
    assert_eq!(signatures(r#"{"file_path":"/w/a/b.rs","offset":10,"limit":5}"#), ["file:a/b.rs"]);
    assert_eq!(signatures(r#"{"skill":"report"}"#), ["skill:report"]);
    assert_eq!(signatures(r#"{"cmd":"kubectl get pods -n crm","workdir":"/a"}"#), ["kubectl get"]);
    assert_eq!(signatures("~/.local/bin/cpu-watchdog monitor-ack 84025817754507256052"), ["cpu-watchdog monitor-ack"]);
    // Loop control words are not actions (retro 81740295292268599208 ranked `break` second).
    assert_eq!(signatures("git status && break; continue"), ["git status"]);
}

#[test]
fn variables_set_in_the_call_name_the_script() {
    assert_eq!(signatures("S=/w/Mars_CRM/scripts/demo-session.ts; bun $S request GET /api/me"), ["bun demo-session.ts request"]);
    assert_eq!(signatures("R=/tmp/run; C=\"$R/capture.js\"; bun \"$C\" shots"), ["bun capture.js shots"]);
    assert_eq!(signatures("export S=/tmp/pad\nbun ${S}/probe.mjs"), ["bun probe.mjs"]);
    // A variable set elsewhere stays unresolved rather than guessed.
    assert_eq!(signatures("bun $S request"), ["bun"]);
}

#[test]
fn a_quoted_assignment_value_is_not_a_program() {
    // Retro 87103953672303196097 ranked `Macintosh;` (42 calls) from this user-agent assignment.
    let fetch = "UA='Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/130'; curl -sL -m 20 -A \"$UA\" https://aistart.ai/submit | sed 's/<[^>]*>/ /g'";
    assert_eq!(signatures(fetch), ["url:aistart.ai", "curl aistart.ai"]);
    assert_eq!(signatures("export NOTE=\"a b; c\" D=/tmp/x; git status"), ["git status"]);
    // A command substitution runs its command.
    assert_eq!(signatures("B=$(gcloud builds list --region=x --format='value(id)' | head -1); echo $B"), ["gcloud builds"]);
    assert_eq!(signatures("FOO=1 BAR=\"x y\" bun scripts/spec.ts set --file - x"), ["bun spec.ts set"]);
}

#[test]
fn inline_scripts_count_by_the_apis_they_call() {
    let a = signatures("curl -s https://reports.example.com/a/ > /tmp/a.html; bun -e 'const k = await crypto.subtle.deriveKey(x); const p = await crypto.subtle.decrypt(y); console.log(p.map(String))'");
    assert_eq!(a, ["url:reports.example.com", "bun -e", "api:subtle.deriveKey", "api:subtle.decrypt", "curl reports.example.com"]);
    let b = signatures("node -e \"globalThis.crypto.subtle.decrypt(alg, key, data)\"");
    assert!(b.contains(&"api:subtle.decrypt".to_string()));
    assert_eq!(signatures(r#"tools.exec_command({cmd:"git log -1"})"#), ["git log"]);
    assert!(signatures("*** Begin Patch\n*** Update File: a.rs").is_empty());
    // A Codex code-mode cell without a shell command signs as `codex js` plus its APIs, not `const`.
    assert_eq!(signatures(r#"const t=eval(load("tap"));await eval(load("capture"))("avatar",t(440,102))"#), ["codex js"]);
    assert_eq!(signatures("// @exec: {\"yield_time_ms\": 1000}\nconst r=await tools.write_stdin({session_id:1});store(\"p\",r);"), ["codex js", "api:tools.write_stdin"]);
    assert_eq!(signatures("let n = load(\"x\"); text(n)"), ["codex js"]);
    assert_eq!(signatures("let i=i+1; git status"), ["let", "git status"]);
    assert_eq!(signatures("cat > /tmp/a.ts <<'EOF'\nconst x = 1\nlet y = 2\nEOF\ntimeout 30 git status | grep M"), ["file:tmp/a.ts", "git status"]);
}

fn transcript(home: &Path, session: &str, rows: &[(u32, bool, &str)]) {
    let dir = home.join(".claude/projects/p");
    std::fs::create_dir_all(&dir).unwrap();
    let lines: Vec<String> = rows.iter().map(|(second, result, text)| {
        let item = if *result { json!({"type": "tool_result", "content": text}) } else { json!({"type": "tool_use", "input": {"command": text}}) };
        json!({"timestamp": format!("2026-10-25T08:00:{second:02}Z"), "message": {"content": [item]}}).to_string()
    }).collect();
    std::fs::write(dir.join(format!("{session}.jsonl")), lines.join("\n")).unwrap();
}

#[test]
fn census_ranks_stuck_and_cross_session_repeats() {
    let home = crate::telemetry::test_home("retro-repeats");
    let decrypt = "bun -e 'crypto.subtle.decrypt(a)'";
    transcript(&home, "00000000-0000-0000-0000-000000000001", &[(1, false, decrypt), (2, true, "Exit code 1\nOperationError"),
        (3, false, decrypt), (4, true, "Exit code 1"), (5, false, decrypt), (6, true, "ok"), (7, false, "git status")]);
    for n in 2..=3 {
        transcript(&home, &format!("00000000-0000-0000-0000-00000000000{n}"), &[(1, false, "git status"), (2, false, decrypt)]);
    }
    let from = crate::cause_tree::attribute::parse_time("2026-10-25T08:00:00Z").unwrap();
    let value = census(&home, from, from + 60_000.0, Some("api:subtle.decrypt"));
    assert_eq!((value["sessions"].as_u64(), value["calls"].as_u64()), (Some(3), Some(8)));
    let top = value["top"].as_array().unwrap();
    let api = top.iter().find(|r| r["signature"] == "api:subtle.decrypt").unwrap();
    assert_eq!((api["sessions"].as_u64(), api["extra_calls"].as_u64(), api["stuck_sessions"].as_u64(), api["failed_calls"].as_u64()), (Some(3), Some(2), Some(1), Some(2)));
    assert_eq!(top[0]["signature"], "api:subtle.decrypt");
    assert!(api["examples"][0].as_str().unwrap().ends_with("000000000001.jsonl:1"));
    let shown = value["shown"]["calls"].as_array().unwrap();
    let first = shown.iter().find(|c| c["at"].as_str().unwrap().ends_with("000000000001.jsonl:1")).unwrap();
    assert_eq!((shown.len(), first["result"].as_str()), (5, Some("Exit code 1\nOperationError")));
    let git = top.iter().find(|r| r["signature"] == "git status").unwrap();
    assert_eq!((git["sessions"].as_u64(), git["stuck_sessions"].as_u64()), (Some(3), Some(0)));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn ack_needs_a_deep_dive_per_required_signature() {
    let home = crate::telemetry::test_home("retro-repeats-ack");
    let refs = home.join(".codex/skills/watchdog/references");
    std::fs::create_dir_all(&refs).unwrap();
    std::fs::write(refs.join("retro-probes.json"), json!({"probes": [{"id": "R9"}]}).to_string()).unwrap();
    std::fs::write(refs.join("retro-necessary.json"), json!({"signatures": [{"signature": "git *", "reason": "state check"}]}).to_string()).unwrap();
    let census = json!({"top": [{"signature": "git status", "sessions": 9, "calls": 9}, {"signature": "api:subtle.decrypt"}, {"signature": "url:reports.example.com"}]});
    let receipt = receipt(&home, Some("7"), census.clone(), TOP);
    assert_eq!(receipt["required"], json!(["api:subtle.decrypt", "url:reports.example.com"]));
    std::fs::write(refs.join("retro-focus.json"), json!({"signatures": [{"signature": "url:reports.example.com", "concern": "reports decrypted by hand"}, {"signature": "curl reports.example.com"}]}).to_string()).unwrap();
    let focused = super::receipt(&home, Some("7"), census, 1);
    assert_eq!(focused["required"], json!(["url:reports.example.com", "api:subtle.decrypt"]));
    assert_eq!((focused["focus"][1]["recurring"].as_bool(), focused["top"].as_array().unwrap().len()), (Some(false), 3));
    std::fs::remove_file(refs.join("retro-focus.json")).unwrap();
    assert_eq!(receipt["known_necessary"][0]["signature"], "git status");
    let incident = root(&home).join("incidents/7.json");
    std::fs::create_dir_all(incident.parent().unwrap()).unwrap();
    std::fs::write(&incident, json!({"trigger": "model_runtime_reached", "requirements": {"repeats": 1}}).to_string()).unwrap();
    let analysis = root(&home).join("analysis/7");
    std::fs::create_dir_all(&analysis).unwrap();
    let report = |value: Value| std::fs::write(analysis.join("report.json"), value.to_string()).unwrap();
    report(json!({"repeats": []}));
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("retro-repeats"));
    write_json(&analysis.join("repeats.json"), &receipt).unwrap();
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("api:subtle.decrypt"));
    let decrypt = json!({"signature": "api:subtle.decrypt", "verdict": "existing_entry", "why": "read.ts decrypts; the skill line was missed", "evidence": ["a.jsonl:1"]});
    report(json!({"repeats": [decrypt, {"signature": "url:reports.example.com", "verdict": "necessary", "why": "reads", "evidence": ["a.jsonl:2"]}]}));
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("change"));
    let mut decrypt = decrypt;
    decrypt["change"] = json!("report SKILL.md first line points at read.ts");
    decrypt["probe"] = json!("R9");
    report(json!({"repeats": [decrypt.clone(), {"signature": "url:reports.example.com", "verdict": "necessary", "why": "reads", "evidence": ["a.jsonl:2"]}]}));
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("retro-necessary.json"));
    report(json!({"repeats": [decrypt, {"signature": "url:reports.example.com", "verdict": "deferred", "why": "reads", "evidence": ["a.jsonl:2"], "next": "measure read.ts share"}]}));
    validate_ack(&home, "7", &incident).unwrap();
    std::fs::write(&incident, json!({"trigger": "model_runtime_reached"}).to_string()).unwrap();
    std::fs::remove_file(analysis.join("repeats.json")).unwrap();
    validate_ack(&home, "7", &incident).unwrap();
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn text_view_prints_one_line_per_call_or_row() {
    let row = json!({"signature": "jq", "calls": 3, "sessions": 2, "extra_calls": 1, "stuck_sessions": 0, "failed_calls": 1});
    let long = format!("jq -r '.a'\n   {}", "x".repeat(300));
    let drilled = json!({"calls": 9, "sessions": 2, "top": [row.clone()], "shown": {"signature": "jq", "calls": [
        {"at": "/t/a.jsonl:4", "label": "fresh", "call": long, "result": "Exit code 1\nparse error"}]}});
    let lines: Vec<String> = super::text::render(&drilled).lines().map(String::from).collect();
    assert_eq!(lines[0], "jq: 3 calls in 2 sessions, 1 extra, 0 stuck, 1 failed, 0 duplicate, 0 retry");
    assert!(lines[1].starts_with("/t/a.jsonl:4 [fresh] jq -r '.a' xxx") && lines[1].ends_with("… => Exit code 1 parse error"), "{}", lines[1]);
    assert_eq!(lines.len(), 2);
    let ranked = json!({"calls": 9, "sessions": 2, "required": ["jq"], "top": [row]});
    assert_eq!(super::text::render(&ranked), "9 calls in 2 sessions\nrequired: jq\njq: 3 calls in 2 sessions, 1 extra, 0 stuck, 1 failed, 0 duplicate, 0 retry");
}
