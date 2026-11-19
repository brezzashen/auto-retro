use super::super::classify::{classify, entry_of, lints, outcome};
use super::*;

#[test]
fn entries_start_a_segment_after_wrappers() {
    assert_eq!(entry_of("cd Mars_CRM && bun run deliver canary --purpose=x --queue=Q1").as_deref(), Some("deliver canary"));
    assert_eq!(entry_of("bun run deliver beta-web --queue=Q1 web/a.tsx").as_deref(), Some("deliver beta-web"));
    // The `mars` launcher runs the same deliver (release retro 811587194803931207163: its calls were uncounted).
    assert_eq!(entry_of("P=\"web/a.ts web/b.ts\"; mars deliver beta-web --purpose=x --queue=Q1 $P 2>&1 | tail").as_deref(), Some("deliver beta-web"));
    assert_eq!(entry_of("~/.local/bin/mars deliver canary --ref=origin/mastra").as_deref(), Some("deliver canary"));
    assert_eq!(entry_of("mars deliver status"), None);
    assert_eq!(entry_of("FOO=1 ~/.local/bin/cpu-build-gate run -- xcodebuild -scheme MarsCRM test").as_deref(), Some("xcodebuild"));
    assert_eq!(entry_of("timeout 600 bun run canary release --dry-run").as_deref(), Some("canary release"));
    assert_eq!(entry_of("docker buildx build -t x .").as_deref(), Some("docker build"));
    // Naming a release command is not running it.
    assert_eq!(entry_of("rg -n 'deliver canary' docs/"), None);
    assert_eq!(entry_of("git commit -m \"bun run deliver ios\""), None);
    assert_eq!(entry_of("agent-memory write --name x <<'EOF'\nbun run deliver ios --queue=Q2\nEOF"), None);
    assert_eq!(entry_of("bun run deliver status"), None);
    assert_eq!(entry_of("xcodebuild -list"), None);
}

#[test]
fn lints_read_the_command_text() {
    assert_eq!(lints("xcodebuild", "xcodebuild -scheme MarsCRM test"), vec!["ungated_build", "shared_derived_data"]);
    assert!(lints("xcodebuild", "cpu-build-gate run -- xcodebuild -derivedDataPath /x test").is_empty());
    assert_eq!(lints("canary release", "bun run canary release"), vec!["off_board"]);
    assert!(lints("canary release", "bun run canary release --dry-run").is_empty());
    assert!(lints("deliver canary", "bun run deliver canary --queue=Q1").is_empty());
}

#[test]
fn classify_reads_refusals_and_heads() {
    assert_eq!(classify(r#"{"target":"canary","ok":false,"refused":"unknown flag --migrations for deliver canary; usage: bun run deliver"}"#), "unknown_flag");
    let warned = format!("{}{}", "warning: field `domain` is never read\n".repeat(80), r#"{"ok":false,"refused":"deliver beta-web needs --purpose=x"}"#);
    assert_eq!(classify(&warned), "missing_flag");
    assert_eq!(classify("Exit code 1\n(eval):2: unmatched '"), "shell_syntax");
    assert_eq!(classify("PreToolUse:Bash hook error: [guard]: Blocked: rm -rf against a dangerous path"), "guard_blocked");
    let signing = format!("{}No profiles for 'local.autoretro.MarsCRM'. To enable automatic signing, pass -allowProvisioningUpdates to xcodebuild.", "setting\n".repeat(400));
    assert_eq!(classify(&signing), "xcode_input");
    // Retro 81781503530293676883: 3 calls ran the UI tests on My Mac below their deployment target.
    assert_eq!(classify("token-ok\nxcodebuild: error: Failed to build project MarsCRM with scheme MarsCRMPrivacyUITests.: Cannot test target \u{201c}MarsCRMUITests\u{201d} on \u{201c}My Mac\u{201d}: My Mac\u{2019}s macOS 27.0.1 doesn\u{2019}t match MarsCRMUITests\u{2019}s macOS 27.2 deployment target."), "xcode_input");
    // A long build failure naming a missing file deep in its log is not an instruction error.
    let long = format!("{}clang: error: no such file or directory: foo.o", "compiling …\n".repeat(400));
    assert_eq!(classify(&long), "other");
    // Retro 81623886145556566588: ship-web ran its checks for 89 s, then named the typed path no tree holds.
    let missing = format!("{}{}", "[ship-web] geometry passed\n".repeat(200), r#""web/src/screens/promo/AppPreviewHome.tsx is neither in the workspace nor in the serving source /M/Mars_CRM","error: script \"release\" exited with code 1"]}"#);
    assert_eq!(classify(&missing), "wrong_place");
    assert_eq!(classify("Deployment api is mid-rollout; wait for `mars k8s status` to settle before preparing."), "other", "an environment refusal is no instruction error");
}

#[test]
fn outcomes_from_claude_and_codex() {
    assert!(outcome("Exit code 2\nrefused", true, None).0);
    assert!(outcome(r#"{"output":"usage: x","metadata":{"exit_code":2}}"#, false, None).0);
    assert!(!outcome("Exit code: 0\nWall time: 1s", false, None).0);
    // `| tail` hides the exit code; the receipt still says it failed.
    assert!(outcome(r#"{"target":"ios","ok":false,"refused":"x"}"#, false, None).0);
    assert!(outcome("done", false, Some(1)).0);
    // A tab that missed the release notice is no failed release (release retro 84705495676337590512).
    let notified = r#"{"ok":true,"notified":[{"session":"s","ok":false,"receipt":"no connected tab matches \"a]b\""}],"x":1}"#;
    assert!(!outcome(&format!("{notified}\n\n[exited with code 0]"), false, None).0);
    assert!(outcome(r#"{"ok":false,"notified":[]}"#, false, None).0);
    // `| tail -c 400` cut the `"notified":[` key; the rest is still one tab's notice (release retro 810781605592223812110).
    let cut = r#"ok":true,"receipt":"delivered term_7"},{"session":"f348","title":"t","commits":["96498b9625"],"ok":false,"receipt":"Error: 2 terminals match"}]}}"#;
    assert!(!outcome(&format!("{cut}\n [exited with code 0]"), false, None).0);
    // A disk check before an S0 cleanup the run passed is no failed release, even cut by `| cut -c1-400`
    // (canary 20261113T151338Z-canary-31669, release retro 814732438991896643353).
    let recovered = r#"{"target":"canary","ok":true,"seconds":680,"release":"20261113T151338Z-canary-31669","diskRecovery":{"dryRun":false,"initialSpace":{"ok":false,"output":"[{\"path\":\"/System/Volumes/Data\",\"free_bytes\":179471994880},{\"p"#;
    assert!(!outcome(&format!("{recovered}\n\n[exited with code 0]"), false, None).0);
    let closed = r#"{"ok":true,"diskRecovery":{"initialSpace":{"ok":false},"after":{"ok":true}},"x":1}"#;
    assert!(!outcome(closed, false, None).0);
    assert!(outcome(r#"{"ok":false,"diskRecovery":{"initialSpace":{"ok":false}}}"#, false, None).0);
}

fn call(t: &str, id: &str, cmd: &str) -> String {
    json!({"type": "assistant", "timestamp": t, "message": {"content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}}).to_string()
}

fn result(t: &str, id: &str, out: &str, err: bool) -> String {
    json!({"type": "user", "timestamp": t, "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": out, "is_error": err}]}}).to_string()
}

fn tally_of(dir_name: &str, files: &[(&str, Vec<String>)]) -> Value {
    let dir = std::env::temp_dir().join(format!("release-commands-{dir_name}-{}", std::process::id()));
    let mut tally = Tally::default();
    for (i, (name, lines)) in files.iter().enumerate() {
        let sub = dir.join(i.to_string());
        std::fs::create_dir_all(&sub).unwrap();
        let path = sub.join(name);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        read(&path, "cc5", false, 0.0, f64::MAX, &mut tally);
    }
    std::fs::remove_dir_all(&dir).unwrap();
    summarize(&tally.calls, &tally.retries)
}

#[test]
fn a_flag_retry_counts_once_across_copied_transcripts() {
    let lines = vec![
        call("2026-10-28T01:00:00.000Z", "t1", "bun run deliver canary --queue=Q1"),
        result("2026-10-28T01:00:02.000Z", "t1", "Exit code 2\n{\"ok\":false,\"refused\":\"deliver canary needs --purpose=\\\"<…>\\\" and --queue=Q1|Q2\"}", true),
        call("2026-10-28T01:01:00.000Z", "t2", "bun run deliver canary --queue=Q1 --purpose=fix"),
        result("2026-10-28T01:20:00.000Z", "t2", "{\"ok\":true}", false),
    ];
    // The same session resumed under another account: one copy per profile.
    let out = tally_of("retry", &[("s.jsonl", lines.clone()), ("s.jsonl", lines)]);
    assert_eq!(out["calls"], 2);
    assert_eq!(out["instruction_errors"], 1);
    assert_eq!(out["classes"]["missing_flag"]["n"], 1);
    assert_eq!(out["flag_retries"]["n"], 1);
    assert_eq!(out["flag_retries"]["examples"][0]["added"][0], "--purpose=fix");
}

#[test]
fn a_background_run_resolves_at_its_notification() {
    let note = "<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>t9</tool-use-id>\n<output-file>/nonexistent/b1.output</output-file>\n<status>failed</status>\n<summary>Background command \"deliver\" failed with exit code 2</summary>\n</task-notification>";
    let out = tally_of("background", &[("s.jsonl", vec![
        call("2026-10-28T02:00:00.000Z", "t9", "bun run deliver ios --queue=Q2"),
        result("2026-10-28T02:00:01.000Z", "t9", "Command running in background with ID: b1. Output is being written to: /nonexistent/b1.output.", false),
        json!({"type": "user", "timestamp": "2026-10-28T02:10:00.000Z", "message": {"content": note}}).to_string(),
    ])]);
    assert_eq!(out["calls"], 1);
    assert_eq!(out["background"], 1);
    assert_eq!(out["failed"], 1);
    // No needle matched: the retro gets the call as an unclassified example to recall by meaning.
    assert_eq!(out["unclassified"]["n"], 1);
}

#[test]
fn a_failed_background_output_outlives_its_file() {
    let dir = std::env::temp_dir().join(format!("release-outputs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("b2.output");
    std::fs::write(&file, "deliver ios needs --purpose=\"<…>\" and --queue=Q1|Q2\n{\"ok\":false}").unwrap();
    // The wrapper's `echo "exit $?"` makes the notification report exit code 0.
    let note = format!("<task-notification>\n<task-id>b2</task-id>\n<tool-use-id>t7</tool-use-id>\n<output-file>{}</output-file>\n<status>completed</status>\n<summary>Background command \"deliver\" completed (exit code 0)</summary>\n</task-notification>", file.display());
    let transcript = dir.join("s.jsonl");
    std::fs::write(&transcript, [
        call("2026-10-28T03:00:00.000Z", "t7", "bun run deliver ios > /tmp/x.log 2>&1; echo \"exit $?\""),
        result("2026-10-28T03:00:01.000Z", "t7", "Command running in background with ID: b2. Output is being written to: x.", false),
        json!({"type": "user", "timestamp": "2026-10-28T03:05:00.000Z", "message": {"content": note}}).to_string(),
    ].join("\n") + "\n").unwrap();
    let cache = dir.join("release-retro-outputs.json");
    let pass = || {
        let mut tally = Tally { outputs: Outputs::from_path(cache.clone()), ..Tally::default() };
        read(&transcript, "cc3", false, 0.0, f64::MAX, &mut tally);
        std::mem::take(&mut tally.outputs).save(super::super::stats::ms(Some("2026-10-29T00:00:00.000Z")).unwrap());
        summarize(&tally.calls, &tally.retries)
    };
    let first = pass();
    assert_eq!(first["classes"]["missing_flag"]["n"], 1);
    // /private/tmp purged the output file; the recomputed window still counts the refusal.
    std::fs::remove_file(&file).unwrap();
    let again = pass();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(again["failed"], 1);
    assert_eq!(again["classes"]["missing_flag"]["n"], 1);
}

#[test]
fn a_missing_build_input_is_no_instruction_error() {
    let out = "rc=65 error: Error opening input file '/x/ios/MarsCRM/Features/People/Avatar/AvatarColorScheme.swift' (No such file or directory)";
    assert_eq!(classify(out), "other");
    assert_eq!(classify("Exit code 1 (eval):cd:1: no such file or directory: /x/first-sign-in"), "wrong_place");
}

#[test]
fn quoted_text_is_data_but_a_quoted_substitution_runs() {
    // A grep pattern and a commit summary were counted as ungated builds (2026-10-31).
    assert_eq!(entry_of(r#"grep -n -i -E "only-testing|unit test|xcodebuild test|ios test" SKILL.md | head -10"#), None);
    assert_eq!(entry_of(r#"bun scripts/utils/commit_owned_paths.ts --mint-summary "fix; xcodebuild -scheme X test (gated)" -- a"#), None);
    assert_eq!(entry_of(r#"OUT="$(xcodebuild -scheme MarsCRM test 2>&1)"; echo "$OUT""#).as_deref(), Some("xcodebuild"));
    assert_eq!(entry_of(r#"U=$(sim-lease acquire) && cpu-build-gate run -- xcodebuild -scheme M test"#).as_deref(), Some("xcodebuild"));
    assert_eq!(entry_of(r#"echo "a\"b" && bun run deliver ios --queue=Q2"#).as_deref(), Some("deliver ios"));
}

#[test]
fn a_destination_naming_no_device_is_an_xcode_input_error() {
    let out = "xcodebuild: error: Unable to find a device matching the provided destination specifier:\n\t\t{ platform:iOS Simulator, id:E2547D17 }";
    assert_eq!(classify(out), "xcode_input");
}

/// Release retro 82097144209858832811: every `unclassified` call was a receipt of a run that failed on
/// the board (a test, a typecheck); the board's failure class counts those, the instruction review does not.
#[test]
fn a_receipt_of_a_failed_board_run_is_no_unclassified_call() {
    let receipt = "{\"target\":\"beta-web\",\"ok\":false,\"seconds\":120,\"steps\":[{\"step\":\"ship-web\",\"exitCode\":1}],\"release\":\"20261118T105351Z-beta-web-46417\",\"failure\":[\"(fail) the iPad card\"]} [exited with code 0]";
    assert_eq!(classify(receipt), "release_failed");
    let out = tally_of("board-run", &[("s.jsonl", vec![
        call("2026-11-18T10:53:50.000Z", "t1", "mars deliver beta-web --purpose=x --queue=Q2 web/a.ts 2>&1 | tail -1"),
        result("2026-11-18T10:55:51.000Z", "t1", receipt, false),
        call("2026-11-18T10:56:00.000Z", "t2", "mars deliver beta-web --purpose=x --queue=Q1 web/a.ts"),
        result("2026-11-18T11:00:00.000Z", "t2", "{\"ok\":true}", false),
    ])]);
    assert_eq!(out["failed"], 1);
    assert_eq!(out["instruction_errors"], 0);
    assert_eq!(out["unclassified"]["n"], 0);
    assert_eq!(out["release_failed"]["n"], 1);
    assert_eq!(out["release_failed"]["runs"][0], "20261118T105351Z-beta-web-46417");
    assert_eq!(out["flag_retries"]["after_instruction_error"], 0);
}
