//! Repeat-failure keys for file tools (Read, Edit, Write): the path is the identity.
use super::*;

const T0: &str = "2026-11-10T07:00:00.000Z";
const SP: &str = "/private/tmp/claude-501/-Volumes-MarsWork-Workspaces-Mars-agent/9bc616d0-659e-43e1-b1ad-7108b1f30305/scratchpad";

fn at(secs: u32) -> String { format!("2026-11-10T07:{:02}:{:02}.000Z", secs / 60, secs % 60) }

fn failing_reads(paths: &[String]) -> parse::TaskCursor {
    let mut c = parse::TaskCursor::new("cc5", false, false);
    c.feed(&json!({"type": "user", "timestamp": T0, "cwd": "/r", "message": {"content": "load history faster"}}));
    for (i, path) in paths.iter().enumerate() {
        let (id, t) = (format!("r{i}"), 60 + i as u32 * 60);
        c.feed(&json!({"type": "assistant", "timestamp": at(t), "message": {"stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": path}}]}}));
        c.feed(&json!({"type": "user", "timestamp": at(t + 1), "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": true}]}}));
    }
    c
}

#[test]
fn reads_of_different_files_under_one_long_scratchpad_are_not_a_repeat_failure() {
    let files = ["run4/sheetA.png", "run4/sheetB.png", "mac-open.png"].map(|f| format!("{SP}/{f}"));
    let c = failing_reads(&files);
    assert_eq!(c.failures.len(), 3, "each path keys its own signature: {:?}", c.failures);
    assert!(c.failures.values().all(|n| *n == 1));
}

#[test]
fn the_same_missing_file_read_three_times_is_still_a_repeat_failure() {
    let c = failing_reads(&vec![format!("{SP}/run4/sheetA.png"); 3]);
    assert_eq!(c.failures.get(&format!("Read: {SP}/run4/sheetA.png")), Some(&3));
}

#[test]
fn a_long_first_word_stays_whole_while_trailing_arguments_still_merge() {
    let long = format!("{SP}/tool");
    assert_eq!(parse::signature_of("Bash:x", &long), format!("Bash:x: {long}"));
    let a = parse::signature_of("Bash:bun", &format!("bun run deliver canary --purpose {} --run a", "x".repeat(90)));
    let b = parse::signature_of("Bash:bun", &format!("bun run deliver canary --purpose {} --run b", "x".repeat(90)));
    assert_eq!(a, b, "the same command with different trailing arguments still repeats");
}
