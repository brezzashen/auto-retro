use super::*;

fn claude(home: &Path, session: &str, rows: &[Value]) -> String {
    let dir = home.join(".claude/projects/p");
    std::fs::create_dir_all(&dir).unwrap();
    let lines: Vec<String> = rows.iter().enumerate().map(|(i, row)| {
        let mut row = row.clone();
        row["timestamp"] = json!(format!("2026-10-26T08:00:{i:02}Z"));
        row.to_string()
    }).collect();
    let path = dir.join(format!("{session}.jsonl"));
    std::fs::write(&path, lines.join("\n")).unwrap();
    path.display().to_string()
}

fn typed(text: &str) -> Value { json!({"type": "user", "origin": {"kind": "human"}, "message": {"role": "user", "content": text}}) }
fn said(text: &str) -> Value { json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}) }

#[test]
fn classifies_claude_and_codex_rows() {
    let queued = json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": "用模拟器看", "origin": {"kind": "human"}}});
    assert_eq!(classify(&queued), Some(("mid_turn", "用模拟器看".into())));
    assert_eq!(classify(&typed("[Request interrupted by user]")).unwrap().0, "interrupt");
    let meta = json!({"type": "user", "isMeta": true, "message": {"content": "[Image: x]"}});
    assert_eq!(classify(&meta), None);
    let result = json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}});
    assert_eq!(classify(&result), None);
    let ask = json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "AskUserQuestion", "input": {}}]}});
    assert_eq!(classify(&ask).unwrap().0, "asked");
    let codex = json!({"payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "分析失败"}]}});
    assert_eq!(classify(&codex), Some(("human", "分析失败".into())));
    let aborted = json!({"payload": {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "<turn_aborted>\nx"}]}});
    assert_eq!(classify(&aborted).unwrap().0, "interrupt");
}

#[test]
fn a_correction_marks_the_instruction_before_it() {
    let home = crate::telemetry::test_home("retro-clarity");
    let path = claude(&home, "11111111-2222-3333-4444-555555555555", &[
        typed("<command-name>/clear</command-name>"),
        typed("排查一下 iPhone duo 语音输入"),
        said("我来看 Duo 模拟器"),
        typed("不是模拟器，我说的是真机"),
        typed("好的继续"),
        json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": "顺便看日志", "origin": {"kind": "human"}}}),
    ]);
    let census = census(&home, 0.0, f64::MAX);
    assert_eq!(census["sessions"], 1);
    assert_eq!(census["human_turns"], 4);
    let candidates = census["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    let first = &candidates[0];
    assert_eq!(first["at"], format!("{path}:4"));
    assert_eq!(first["prior_at"], format!("{path}:2"));
    assert_eq!(first["prior"], "排查一下 iPhone duo 语音输入");
    assert_eq!(first["agent_before"], "我来看 Duo 模拟器");
    assert_eq!(first["signals"], json!(["correction"]));
    assert_eq!(candidates[1]["signals"], json!(["mid_turn"]));
    assert_eq!(census["required"], json!([format!("{path}:4")]), "a mid-turn note alone is listed, not required");
}

#[test]
fn ack_needs_a_verdict_per_required_candidate_and_a_summary() {
    let home = crate::telemetry::test_home("retro-clarity-ack");
    let path = claude(&home, "11111111-2222-3333-4444-555555555555", &[typed("修一下按钮"), said("改了颜色"), typed("[Request interrupted by user]"), typed("我要的是位置")]);
    let at = format!("{path}:4");
    let incidents = root(&home).join("incidents");
    std::fs::create_dir_all(&incidents).unwrap();
    let incident = incidents.join("7.json");
    std::fs::write(&incident, json!({"requirements": {"clarity": 1}, "window": {"from_ms": 0.0, "to_ms": 9e15}}).to_string()).unwrap();
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("retro-clarity"));
    cli(&home, vec!["7".into()]).unwrap();
    let analysis = root(&home).join("analysis/7");
    let write = |report: Value, md: &str| {
        std::fs::write(analysis.join("report.json"), report.to_string()).unwrap();
        std::fs::write(analysis.join("report.md"), md).unwrap();
    };
    write(json!({"instruction_clarity": {"summary": "一处", "reviewed": []}}), "## 指令清晰度");
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("no verdict"));
    let entry = json!({"at": at, "verdict": "instruction", "analysis": "没说改哪一项", "cause": "incomplete",
        "instruction": "修一下按钮", "intended": "调整位置", "interpretation": "改了颜色", "cost": "一轮返工"});
    write(json!({"instruction_clarity": {"summary": "一处", "reviewed": [entry.clone()]}}), "## 指令清晰度");
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("suggestion"));
    let mut done = entry;
    done["suggestion"] = json!("修一下按钮的位置");
    write(json!({"instruction_clarity": {"summary": "一处", "reviewed": [done.clone()]}}), "无此节");
    assert!(validate_ack(&home, "7", &incident).unwrap_err().to_string().contains("指令清晰度"));
    write(json!({"instruction_clarity": {"summary": "一处", "reviewed": [done]}}), "## 指令清晰度");
    validate_ack(&home, "7", &incident).unwrap();
    let old = incidents.join("8.json");
    std::fs::write(&old, json!({"requirements": {"repeats": 1}}).to_string()).unwrap();
    validate_ack(&home, "8", &old).unwrap();
}

#[test]
fn questions_and_pasted_paths_are_not_corrections() {
    assert!(corrects("胶囊样式不对呀，应该纵置"));
    assert!(corrects("我的意思是还在浏览器中打开"));
    assert!(!corrects("你看一下这个文件对不对。"));
    assert!(!corrects("/Users/b/orca-paste-1-不对.txt"));
    assert!(!corrects("现在可用的应该是 CC2、CC5 和 CC6"));
}

#[test]
fn relayed_session_messages_are_not_the_founders() {
    assert!(machine("[Release coordinator mars-agent-3e] Disk gate CLEAR"));
    assert!(machine("<command-name>/clear</command-name>"));
    assert!(!machine("[Image #1] 这个边框不对"));
    assert!(!machine("[Pasted text #1 +20 lines] 看一下"));
    assert!(!machine("修一下按钮"));
}
