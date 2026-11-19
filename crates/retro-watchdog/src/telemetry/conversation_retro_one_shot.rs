//! One-shot model calls (founder, 2026-11-02, conversation retro 812679272853355492185): a Claude
//! transcript written in print mode (`"entrypoint":"sdk-cli"`) that holds no tool call is a single
//! prompt and answer, such as a turn-scoring judge, and is not a conversation to review. Print-mode
//! runs that use tools (launchd backstops) still count. The census reruns every sample, so a
//! print-mode run counts as soon as its first tool call is written.
//!
//! Bridged one-shot calls (conversation retro 814983970739705856954, 2026-11-14): Orca writes
//! Antigravity transcripts into `~/.claude/projects/-Users-<user>-antigravity-orca-bridge` without
//! any `entrypoint` field, so the print-mode check never matched them. 111 of the 199 transcripts
//! that opened that cc1 retro were single prompt-and-answer calls with no tool call (102 hourly SEO
//! red-team advisor prompts from `seo_competitor/watch_advisor.rs`, 9 READY/ok probes). A bridged
//! transcript with no tool call is now a one-shot model call; bridged sessions that use tools count.
//!
//! Harness runs (founder decision 2026-11-12, report conversation-retro-87553225377583142572 item 1 A):
//! a transcript whose first working directory lies inside another session's scratchpad (A/B evals,
//! nested probes) runs that session's synthetic prompt and is not a conversation either; 18 of cc5's
//! 109 were such runs (incident 87553225377583142572). The memory dream leaves out the same runs.
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Project directory suffix of the transcripts Orca bridges from Antigravity.
const BRIDGE_SUFFIX: &str = "-antigravity-orca-bridge";
/// Scope and source alias of bridged Antigravity sessions (founder decision 2026-11-14, report
/// conversation-retro-814983970739705856954 item 1 A): they leave cc1, which counts only Claude cc1,
/// and count in their own scope with its own baseline, still inside `all`.
pub(super) const BRIDGE_ALIAS: &str = "ag";

pub(super) fn is_bridged(path: &Path) -> bool {
    path.parent().and_then(|d| d.file_name()).and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(BRIDGE_SUFFIX))
}

/// Rows read before giving up on finding a working directory.
const CWD_ROWS: usize = 200;

pub(super) fn is_harness_run(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    for line in BufReader::new(file).lines().take(CWD_ROWS) {
        let Ok(line) = line else { return false };
        if !line.contains("\"cwd\":") { continue; }
        let Some(cwd) = serde_json::from_str::<Value>(&line).ok().and_then(|row| row["cwd"].as_str().map(str::to_string)) else { continue };
        return crate::sweep::scratchpad_session(&cwd).is_some_and(|owner| owner != stem);
    }
    false
}

pub(super) fn is_one_shot_model_call(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    // Bridged transcripts carry no entrypoint; their project directory marks them instead.
    let mut print_mode = is_bridged(path).then_some(true);
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { return false };
        if print_mode.is_none() && line.contains("\"entrypoint\":") {
            print_mode = Some(line.contains("\"entrypoint\":\"sdk-cli\""));
            if print_mode == Some(false) {
                return false;
            }
        }
        if line.contains("\"type\":\"tool_use\"") {
            return false;
        }
    }
    print_mode == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, lines: &[&str]) -> std::path::PathBuf {
        let dir = crate::telemetry::test_home(name);
        let path = dir.join("t.jsonl");
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    #[test]
    fn a_print_mode_prompt_without_tools_is_one_shot() {
        let path = write("one-shot-judge", &[r#"{"type":"user","entrypoint":"sdk-cli","message":{"content":"You grade one AI assistant answer"}}"#, r#"{"type":"assistant","entrypoint":"sdk-cli","message":{"content":[{"type":"text","text":"{}"}]}}"#]);
        assert!(is_one_shot_model_call(&path));
    }

    #[test]
    fn print_mode_with_a_tool_call_or_an_interactive_session_still_counts() {
        let tools = write("one-shot-tools", &[r#"{"type":"user","entrypoint":"sdk-cli"}"#, r#"{"type":"assistant","entrypoint":"sdk-cli","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#]);
        assert!(!is_one_shot_model_call(&tools));
        let interactive = write("one-shot-cli", &[r#"{"type":"user","entrypoint":"cli"}"#]);
        assert!(!is_one_shot_model_call(&interactive));
        let unknown = write("one-shot-none", &[r#"{"type":"summary"}"#]);
        assert!(!is_one_shot_model_call(&unknown));
    }

    #[test]
    fn a_bridged_antigravity_call_without_tools_is_one_shot() {
        let dir = crate::telemetry::test_home("one-shot-bridge").join("-Users-u-antigravity-orca-bridge");
        std::fs::create_dir_all(&dir).unwrap();
        let at = |stem: &str, rows: &[&str]| {
            let path = dir.join(format!("{stem}.jsonl"));
            std::fs::write(&path, rows.join("\n")).unwrap();
            path
        };
        let advisor = at("1dfe212a-0000-0000-0000-000000000000", &[r#"{"type":"user","message":{"role":"user","content":"red-team advisor prompt"}}"#, r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"answer"}]}}"#]);
        assert!(is_one_shot_model_call(&advisor), "bridged prompt and answer without tools");
        let session = at("9386e31f-0000-0000-0000-000000000000", &[r#"{"type":"user","message":{"content":"hi"}}"#, r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#]);
        assert!(!is_one_shot_model_call(&session), "a bridged session with a tool call still counts");
        let claude = write("one-shot-claude-no-entrypoint", &[r#"{"type":"user","message":{"content":"hi"}}"#]);
        assert!(!is_one_shot_model_call(&claude), "outside the bridge, a transcript without entrypoint still counts");
    }

    #[test]
    fn a_run_started_in_another_session_scratchpad_is_a_harness_run() {
        let dir = crate::telemetry::test_home("harness-run");
        let owner = "d57581ea-5c2e-477f-be1d-e4b65255b68f";
        let at = |stem: &str, cwd: &str| {
            let path = dir.join(format!("{stem}.jsonl"));
            std::fs::write(&path, format!("{{\"type\":\"summary\"}}\n{{\"type\":\"user\",\"cwd\":\"{cwd}\"}}")).unwrap();
            path
        };
        let ab = format!("/private/tmp/claude-501/-Volumes-MarsWork-Workspaces-Mars-agent/{owner}/scratchpad/ab-new");
        assert!(is_harness_run(&at("34864922-6e86-44bc-b62b-7af3b5acab86", &ab)), "A/B eval in d57581ea's scratchpad");
        assert!(!is_harness_run(&at(owner, &ab)), "a session working in its own scratchpad still counts");
        assert!(!is_harness_run(&at("12255026-bf31-498d-971a-faddf8d68005", "/Volumes/MarsWork/Workspaces/Mars_agent")), "a repository session counts");
        assert!(!is_harness_run(&at("0fbfe56b-6b1e-43ea-9bc9-842c2099082d", "/tmp/advtest")), "a scratch dir outside an agent tmp is not a scratchpad");
    }
}
