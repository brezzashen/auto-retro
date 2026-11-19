use super::*;

const SID: &str = "4fce2e90-1196-4a8b-beb1-97a94deadb32";

fn at(secs: u64) -> String {
    format!("2026-11-02T08:{:02}:{:02}.000Z", secs / 60, secs % 60)
}

fn ms(secs: u64) -> f64 {
    crate::telemetry::transcript_ms(&at(secs)).unwrap()
}

fn search(kind: &str, secs: u64) -> Value {
    json!({"agent": "claude", "session": SID, "kind": kind, "ts_ms": ms(secs)})
}

#[test]
fn counts_searches_and_minutes_from_the_prompt_to_the_first_edit() {
    let home = std::env::temp_dir().join(format!("pace-{}", std::process::id()));
    let dir = home.join(".claude-profiles/team5/projects/-r");
    std::fs::create_dir_all(&dir).unwrap();
    let edit = |s| json!({"type": "assistant", "timestamp": at(s), "message": {"content": [{"type": "tool_use", "name": "Edit"}]}});
    let lines = [
        json!({"type": "user", "timestamp": at(0), "message": {"content": "统一筛选弹窗"}}),
        json!({"type": "user", "timestamp": at(10), "message": {"content": [{"type": "tool_result"}]}}),
        json!({"type": "user", "timestamp": at(20), "message": {"content": "<task-notification>done</task-notification>"}}),
        edit(180),
        edit(240),
        json!({"type": "user", "timestamp": at(300), "message": {"content": [{"type": "text", "text": "截图在哪"}]}}),
        json!({"type": "user", "timestamp": at(320), "isMeta": true, "message": {"content": "meta"}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(dir.join(format!("{SID}.jsonl")), text).unwrap();
    let events = vec![search("grep", 30), search("local_search", 60), search("grep", 200), search("grep", 310)];
    let pace = measure(&home, &events, ms(0), ms(600));
    assert_eq!(pace["tasks"], json!(2));
    assert_eq!(pace["edited"], json!(1));
    assert_eq!(pace["searches_to_edit"]["p50"], json!(2.0));
    assert_eq!(pace["minutes_to_edit"]["p50"], json!(3.0));
    assert_eq!(pace["local_search_share"], json!(0.5));
    assert!(pace["examples"][0]["transcript"].as_str().unwrap().ends_with(".jsonl:1"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn injected_lines_are_not_prompts() {
    for text in ["<command-name>/clear</command-name>", "[hostq] beta passed", "This session is being continued from x", "cpu-watchdog incident 1: agents made", ""] {
        assert!(human_prompt(&json!({"type": "user", "message": {"content": text}})).is_none(), "{text}");
    }
    assert_eq!(human_prompt(&json!({"type": "user", "message": {"content": "  好的继续 "}})).as_deref(), Some("好的继续"));
}
