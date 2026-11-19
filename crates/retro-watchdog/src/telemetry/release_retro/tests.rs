use super::*;
use serde_json::json;

fn run(id: &str, target: &str, state: &str, end: &str, log: Option<&Path>) -> Value {
    json!({"id": id, "target": target, "queue": "Q2", "purpose": "p", "task": null, "state": state,
        "startedAt": "2026-10-26T00:00:00.000Z", "finishedAt": end, "updatedAt": end, "source": "a".repeat(40),
        "creator": {"pid": 0, "orcaTab": "tab"}, "log": log, "failure": if state == "failed" { json!(["deliver stopped by SIGTERM"]) } else { json!(null) }})
}

fn board(home: &Path, runs: &[Value]) {
    let dir = home.join(".local/state/mars-deliver/releases");
    std::fs::create_dir_all(&dir).unwrap();
    for r in runs { std::fs::write(dir.join(format!("{}.json", r["id"].as_str().unwrap())), r.to_string()).unwrap(); }
}

#[test]
fn run_row_sums_gate_waits_and_counts_untimed_signals() {
    let log = "[cpu-build-gate] starting after 31 s of waiting: 1 s for a slot, 30 s for the CPU window\n\
        [cpu-build-gate] starting after 9 s of waiting: 9 s for a slot, 0 s for the CPU window\n\
        waiting for the TestFlight publish held by pid 4\nqueued for a canary rollout; 2 ahead\n";
    let row = stats::run_row(&run("20261026T000000Z-ios-1", "ios", "succeeded", "2026-10-26T00:10:00.000Z", None), "succeeded", log);
    assert_eq!(row["gate_wait_s"], json!(40.0));
    assert_eq!((row["gate_slot_s"].clone(), row["gate_cpu_s"].clone()), (json!(10.0), json!(30.0)));
    assert_eq!(row["signals"]["testflight_lock"], json!(1));
    assert_eq!(row["signals"]["canary_rollout_queue"], json!(1));
    assert_eq!(row["seconds"], json!(600.0));
}

#[test]
fn failures_classify_by_their_log_tail() {
    assert_eq!(stats::classify("deliver stopped by SIGTERM"), "stopped");
    assert_eq!(stats::classify("error[E0432]: unresolved import"), "rust_compile");
    assert_eq!(stats::classify("error: Cannot find module 'zod/v4'"), "module_missing");
    assert_eq!(stats::classify("Logs: /tmp/mars-crm-release-logs-1\nnothing recognisable"), "other");
    assert_eq!(stats::classify("(fail) the checked-in JSON is what the generator writes today [69691.02ms]"), "generated_drift");
    assert_eq!(stats::classify("Cloud SQL is at 1 but x has pending 2; rerun with --migrate=<versions> naming the ones the user approved"), "migration_gate");
    assert_eq!(stats::classify("offline-validation: error: failed to download lodash@4.18.1: HTTP 5xx\n\"error: script \\\"canary\\\" exited with code 1\""), "package_registry");
    assert_eq!(stats::classify("41af7b21e has the same release inputs as 1746e4895, whose prepare just failed on its code:\npreflight affected agent tests (120) failed after 35s; its cloud build is cancelled:\nerror: expect(received).toBe(expected)\nExpected: 12"), "test");
    assert_eq!(stats::group_of("plan_rejected"), "input");
}

#[test]
fn script_only_rate_counts_runs_no_judge_named() {
    let rows: Vec<Value> = (0..4).map(|i| stats::run_row(&run(&format!("20261026T00000{i}Z-beta-{i}"), "beta", "succeeded",
        "2026-10-26T00:10:00.000Z", None), "succeeded", "")).collect();
    let evidence = stats::Evidence { judges: vec![json!({"started": true, "signatures": ["T4:abandoned:20261026T000001Z-beta-1"]})], ..Default::default() };
    let s = stats::summarize(&rows, &evidence, 0.0, 1.0);
    assert_eq!(s["scripted"]["judged_runs"], json!(1));
    assert_eq!(s["scripted"]["script_only_rate"], json!(75.0));
    assert_eq!(s["scripted"]["judges_by_code"]["T4"], json!(1));
    assert_eq!(s["coverage"]["triggers"]["rate"], json!(66.7));
}

#[test]
fn a_window_opens_only_at_the_threshold_and_moves_the_baseline() {
    let home = crate::telemetry::test_home("release-retro");
    let runs: Vec<Value> = (0..THRESHOLD + 1).map(|i| run(&format!("20261026T{:06}Z-beta-{i}", i), "beta",
        if i % 10 == 0 { "failed" } else { "succeeded" }, &format!("2026-10-26T01:{:02}:{:02}.000Z", i / 60, i % 60), None)).collect();
    board(&home, &runs[..THRESHOLD - 1]);
    let mut retro = ReleaseRetro { loaded: true, ..Default::default() };
    retro.observe(&home, 1.0).unwrap();
    assert!(retro.dispatches.is_empty());
    board(&home, &runs);
    retro.observe(&home, 1.0 + SAMPLE_SECS).unwrap();
    assert_eq!(retro.dispatches.len(), 1);
    let id = retro.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join(format!("incidents/{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["stats"]["window"]["runs"], json!(THRESHOLD));
    assert_eq!(incident["stats"]["failures"]["stopped"]["n"], json!(10));
    // The newest THRESHOLD of the THRESHOLD + 1 runs form the window; the oldest one is skipped.
    assert_eq!(incident["window"]["skipped"], json!(1));
    assert_eq!(retro.baseline_ms, finished(&home)[THRESHOLD].2);
    // Nothing more opens while the incident is unacknowledged.
    retro.observe(&home, 1.0 + 2.0 * SAMPLE_SECS).unwrap();
    assert_eq!(retro.dispatches.len(), 1);
}

#[test]
fn a_fresh_install_starts_counting_at_install_time() {
    let home = crate::telemetry::test_home("release-retro-fresh");
    let mut retro = ReleaseRetro::default();
    retro.observe(&home, 1_800_000_000.0).unwrap();
    assert_eq!(retro.baseline_ms, 1_800_000_000_000.0);
}

#[test]
fn tails_of_the_2026_10_27_to_29_windows_classify() {
    let cases = [
        ("web/src/a.tsx is neither in the workspace nor in the serving source /x/web-source", "missing_source"),
        ("web/src/a.ts differs from origin/mastra 4d777eab07; commit and push it first: the gateway image builds from the pushed SHA.", "unpushed"),
        ("751a1bc is not on origin/mastra; push it first", "unpushed"),
        ("Runtime manifest agent/package.json differs from support dependencies. Supply the matching release support manifests and lockfile.", "dependency_manifest"),
        ("  ✗ rollout did not complete: agent\n  ✗ rollout failed; previous revisions restored where available", "rollout"),
        ("Deployment agent is mid-rollout; wait for `bun run k8s status` to settle before preparing.", "rollout"),
        ("✗ kubectl apply failed: error validating data: net/http: TLS handshake timeout", "beta_cluster"),
        ("Error: Mac archive is missing MarsCRMWidgets.appex", "mac_archive"),
        ("fatal: '/x/MarsCRM-archive-tree.noindex' is a missing but locked worktree;", "archive_worktree"),
        ("Error: App Store Connect refused the 1.0 (8836) for Mac package: Redundant Binary Upload.", "store_duplicate"),
        ("tests/shimmer.test.ts:\ntimed out after 180000ms", "test"),
        ("kinds.social: social 「zhiwei_lin」: left inset 7.05px to the glyph ink, expected 8px", "geometry"),
        ("[cpu-build-gate] reclaiming build slot 13 from pid 79242 (cargo test)\n[cpu-build-gate] write /s/build-slot-13/owner.10154.tmp", "gate_race"),
        ("closed: publisher pid 44999 exited; no uploaded TestFlight build contains its source", "publisher_exited"),
    ];
    for (tail, class) in cases { assert_eq!(stats::classify(tail), class, "{tail}"); }
}

#[test]
fn a_run_closed_as_abandoned_counts_once_at_its_last_log_line() {
    let home = crate::telemetry::test_home("release-retro-closed");
    let mut closed = run("20261027T125211Z-canary-1", "canary", "failed", "2026-10-29T02:44:46.949Z", None);
    closed["finishedAt"] = json!(null);
    closed["progress"] = json!({"at": "2026-10-27T13:26:48.701Z"});
    closed["closed"] = json!({"outcome": "abandoned", "by": "deliver hygiene"});
    board(&home, &[closed]);
    let runs = finished(&home);
    assert_eq!((runs[0].1, runs[0].2), ("abandoned", stats::ms(Some("2026-10-27T13:26:48.701Z")).unwrap()));
}

#[test]
fn a_run_past_the_gate_is_no_gate_wait_end() {
    let refused = "[cpu-build-gate] CPU reached or exceeded 95% within the past 30 seconds\n[cpu-build-gate] CPU ready: 30-second average 72.5%\nUnsupported image WorkingDir for api: /app; expected its workspace directory.";
    assert_eq!(stats::classify(refused), "image_layout");
    let later = "[cpu-build-gate] CPU ready: 30-second average 72.5%\nnothing recognisable";
    assert_eq!(stats::classify(later), "other");
    let waiting = "[cpu-build-gate] CPU ready: 30-second average 70%\n[cpu-build-gate] CPU reached or exceeded 95% within the past 30 seconds";
    assert_eq!(stats::classify(waiting), "gate_wait_end");
}

#[test]
fn canary_tails_name_the_test_the_rollout_and_the_exact_migrate_rerun() {
    // Offline validation runs inside Cloud Build: its failing test outranks the build failure line.
    assert_eq!(stats::classify("✗ canary build FAILURE:\noffline-validation: (fail) undo restores every CronJob [1.71ms]\n\
        offline-validation: MARS_CRM_RELEASE_FAILED step=offline-validation exit=1"), "test");
    assert_eq!(stats::classify("✗ canary build FAILURE:\noffline-validation: MARS_CRM_RELEASE_FAILED step=offline-validation exit=1"), "cloud_build");
    assert_eq!(stats::classify("✗ canary rollout FAILURE:\nrollout: error: deployment \"api\" exceeded its progress deadline\n\
        rollout: MARS_CRM_RELEASE_FAILED step=rollout exit=1"), "rollout");
    assert_eq!(stats::classify("Cloud SQL is at 1 but x has pending 2; rerun with --migrate=2 (Canary auto-delivery …)"), "migration_gate");
    // Canary 20261118T020502Z-canary-92386, before and after 2de76faba4 added the version hint.
    for refusal in ["  \u{1b}[31m✗\u{1b}[0m --migrate takes 14-digit migration versions, not 20270324000000_payment_order_withdrawal_refund",
        "--migrate takes 14-digit migration versions, not 20270324000000_x; use --migrate=20270324000000"] {
        assert_eq!(stats::classify(refusal), "migrate_flag");
    }
    assert_eq!(stats::group_of("migrate_flag"), "input");
    // Release retro 810781605592223812110: the core regression gate rolled back a broken commit.
    let regression = "  \u{1b}[31m✗\u{1b}[0m canary rollout FAILURE:\nrollout: regression gate: core-regression-release-1794714212 \
        {\"severity\":\"ERROR\",\"event\":\"regression_run\",\"status\":\"failed\",\"failed\":\"health-me\"}\n\
        rollout: MARS_CRM_RELEASE_FAILED step=rollout exit=1";
    assert_eq!(stats::classify(regression), "regression_gate");
    assert_eq!(stats::group_of("regression_gate"), "code");
}

#[test]
fn a_run_closed_long_after_it_started_does_not_pull_earlier_windows_in() {
    // Retro 85869459281164395933: a beta-web run SIGTERMed on 10-27 and closed by hygiene on 10-29
    // set the window's start 31 h before the previous window ended.
    let home = crate::telemetry::test_home("release-retro-floor");
    let mut stale = run("20261027T073045Z-beta-web-1", "beta-web", "succeeded", "2026-10-29T02:44:46.949Z", None);
    stale["startedAt"] = json!("2026-10-27T07:30:45.950Z");
    board(&home, &[stale]);
    let runs = finished(&home);
    let floor = stats::ms(Some("2026-10-28T14:29:19.510Z")).unwrap();
    assert_eq!(measure(&home, &runs, floor)["window"]["from_ms"], json!(floor));
    assert_eq!(measure(&home, &runs, f64::NEG_INFINITY)["window"]["from_ms"], json!(stats::ms(Some("2026-10-27T07:30:45.950Z")).unwrap()));
}

#[test]
fn a_backlog_reviews_the_newest_window_and_counts_the_skipped_runs() {
    // Founder, 2026-11-02 (retro 85869459281164395933, 657 runs behind): skip the backlog as canary_retro does.
    let home = crate::telemetry::test_home("release-retro-backlog");
    let runs: Vec<Value> = (0..THRESHOLD + 7).map(|i| run(&format!("20261026T{:06}Z-beta-{i}", i), "beta", "succeeded",
        &format!("2026-10-26T02:{:02}:{:02}.000Z", i / 60, i % 60), None)).collect();
    board(&home, &runs);
    let mut retro = ReleaseRetro { loaded: true, ..Default::default() };
    retro.observe(&home, 1.0).unwrap();
    let id = retro.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join(format!("incidents/{id}.json"))).unwrap()).unwrap();
    let all = finished(&home);
    assert_eq!(incident["window"]["skipped"], json!(7));
    assert_eq!(incident["stats"]["window"]["runs"], json!(THRESHOLD));
    assert_eq!(incident["window"]["to_ms"], json!(all[THRESHOLD + 6].2));
    assert_eq!(incident["stats"]["window"]["from_ms"], json!(all[6].2));
    assert_eq!(retro.baseline_ms, all[THRESHOLD + 6].2);
}

#[test]
fn three_retros_may_start_in_a_day() {
    // Founder, 2026-11-02: at most 3 release retros per 24 h (was 2).
    let home = crate::telemetry::test_home("release-retro-cap");
    let runs: Vec<Value> = (0..THRESHOLD).map(|i| run(&format!("20261026T{:06}Z-beta-{i}", i), "beta", "succeeded",
        &format!("2026-10-26T03:{:02}:{:02}.000Z", i / 60, i % 60), None)).collect();
    board(&home, &runs);
    std::fs::create_dir_all(root(&home).join("incidents")).unwrap();
    for i in 0..3 { std::fs::write(root(&home).join(format!("incidents/x{i}.ack")), "").unwrap(); }
    let started = |n: usize| (0..n).map(|i| json!({"incident": format!("x{i}"), "at_ms": 1000.0, "started": true})).collect::<Vec<_>>();
    let mut capped = ReleaseRetro { loaded: true, dispatches: started(3), ..Default::default() };
    capped.observe(&home, 2.0).unwrap();
    assert_eq!(capped.dispatches.len(), 3);
    let mut open = ReleaseRetro { loaded: true, dispatches: started(2), ..Default::default() };
    open.observe(&home, 2.0).unwrap();
    assert_eq!(open.dispatches.len(), 3);
}

#[test]
fn a_detached_publish_joins_the_window_in_which_hygiene_closed_it() {
    // Retro 81781503530293676883: finished 09:57:39 while still uploading, closed 10:22:56 after the 10:09 cut.
    let home = crate::telemetry::test_home("release-retro-detached");
    let mut detached = run("20261102T095319Z-ios-45581", "ios", "succeeded", "2026-11-02T09:57:39.462Z", None);
    detached["backgroundPid"] = json!(i32::MAX);
    detached["closed"] = json!({"outcome": "shipped", "at": "2026-11-02T10:22:56.015Z", "by": "deliver hygiene"});
    let plain = run("20261102T095000Z-beta-1", "beta", "succeeded", "2026-11-02T09:58:00.000Z", None);
    board(&home, &[detached, plain]);
    let runs = finished(&home);
    assert_eq!(runs[0].0["id"], json!("20261102T095000Z-beta-1"));
    assert_eq!(runs[1].2, stats::ms(Some("2026-11-02T10:22:56.015Z")).unwrap());
}

#[test]
fn a_detached_publish_waits_for_hygiene_even_after_its_uploader_exits() {
    let home = crate::telemetry::test_home("release-retro-detached-open");
    let mut open = run("20261102T101925Z-ios-9493", "ios", "detached", "2026-11-02T10:24:00.000Z", None);
    open["backgroundPid"] = json!(i32::MAX);
    board(&home, &[open]);
    assert!(finished(&home).is_empty());
}

/// Tails of the 2026-11-02..03 window (release retro 84705495676337590512) that fell to `other` or
/// to the wrong class.
#[test]
fn tails_of_the_2026_11_02_to_03_window_classify() {
    let cases = [
        ("kubectl --context docker-desktop -n mars-crm get deployment api -o jsonpath={.x} exited -1 after 7ms\nstdout: (empty)\nstderr: kubectl: No such file or directory (os error 2)", "tool_missing"),
        ("Unable to connect to the server: net/http: TLS handshake timeout", "beta_cluster"),
        ("stderr: error during connect: Get \"http://docker.example.com/v1.56/images/x/json\": command [ssh -o ConnectTimeout=30 -T -- win docker system dial-stdio] has exited with exit status 255", "beta_cluster"),
        ("Missing test parameter GOOGLE_VERTEX_PROJECT. Supply an offline test value in plan.environment.", "plan_rejected"),
        ("  ✗ cannot CREATE INDEX \"person_contact\" because it has pending trigger events\nerror: script \"canary\" exited with code 1", "migration"),
    ];
    for (tail, class) in cases { assert_eq!(stats::classify(tail), class, "{tail}"); }
}

/// Tails of the 2026-11-03 window (release retro 89813052108966030084) that fell to `other`.
#[test]
fn tails_of_the_2026_11_03_window_classify() {
    let cases = [
        ("[ship-web] latest interval CPU 53.4% ending 2026-11-03T08:01:43.641Z\nDocker VM probe failed: timed out after 10000ms\nerror: script \"release\" exited with code 1", "beta_cluster"),
        ("Error: builds upload: failed to create upload record: request failed: Post \"https://api.appstoreconnect.apple.com/v1/buildUploads\": context deadline exceeded\nError: asc builds upload --pkg failed for 1.0.10154 (10154) for Mac", "store_upload"),
        ("Error: builds upload: commit upload file for build upload \"ae5e\"; reconciliation returned upload state \"AWAITING_UPLOAD\"\nError: asc builds upload --pkg failed for 1.0.10186 (10186) for Mac", "store_upload"),
        ("Caused by:\n  failed to parse manifest at `/x/Mars_CRM/rust/crates/mars-poster/Cargo.toml`\nCaused by:\n  no targets specified in the manifest", "workspace_manifest"),
    ];
    for (tail, class) in cases { assert_eq!(stats::classify(tail), class, "{tail}"); }
}

/// The 2026-11-06..07 window's `other` runs (release retro 87388137676522951755), from their real tails.
#[test]
fn tails_of_the_2026_11_07_window_classify() {
    let cases = [
        ("\u{1b}[2mhost ports route to docker-desktop; the beta edge uses 8035\u{1b}[0m\n  \u{1b}[31m✗\u{1b}[0m no local image mars-crm/crm:430a70745d75\nmars-ops k8s deploy --api=mars-crm/api:gw-test-tag-1 exited 1", "engine_image_missing"),
        ("deployment \"crm\" successfully rolled out\n  \u{1b}[32m✓\u{1b}[0m http://127.0.0.1:8035/api/health ok\n  \u{1b}[31m✗\u{1b}[0m sentinels have not discovered every node yet (replicas 2, peers 3)\n  \u{1b}[31m✗\u{1b}[0m k8s build-crm exited 1", "redis_sentinel"),
        ("error: Missing media image dimensions: wiki-wechat-qr-binding\n      at readWikiMedia (/x/web/scripts/changelogMedia.ts:34:10)\nerror: script \"release\" exited with code 1", "test"),
    ];
    for (tail, class) in cases { assert_eq!(stats::classify(tail), class, "{tail}"); }
}

/// The 2026-11-04 window's `other` runs (release retro 811587194803931207163), from their real tails and log.
#[test]
fn tails_of_the_2026_11_04_window_classify() {
    let cluster = "[ship-web] gateway api · building on win through dockerd's own socket\n  \u{1b}[31m✗\u{1b}[0m local cluster desktop is unavailable; run `mars k8s up` first\nmars-ops k8s deploy --api=mars-crm/api:gw-test-tag-2 exited 1";
    assert_eq!(stats::classify(cluster), "beta_cluster");
    let ios = " 31   -[IBCLIErrorForwarder forwardErrorOutputToDescriptor:whileInvokingBlock:] (in ibtoold)\nCommand CompileAssetCatalogVariant failed with a nonzero exit code\n** ARCHIVE FAILED **";
    assert_eq!(stats::classify(ios), "xcode_tool_crash");
    let mac_tail = "warning: The \"lucide-grid-2x2\" image asset name resolves to the symbol \"lucideGrid2X2\" which already exists.\n** ARCHIVE FAILED **\nError: xcodebuild archive for Mac Catalyst failed";
    assert_eq!(stats::classify(mac_tail), "other");
    let mac_log = format!("CoreSVG has logged an error.\nCommand CompileAssetCatalogVariant failed with a nonzero exit code\n{mac_tail}");
    assert_eq!(stats::classify_with_log(mac_tail, &mac_log), "xcode_tool_crash");
    assert_eq!(stats::classify_with_log("deliver stopped by SIGINT", &mac_log), "stopped", "a tail that names its class keeps it");
    assert_eq!(stats::classify_with_log(mac_tail, "Logs: /tmp/x\nerror[E0432]: unresolved import"), "other", "only LOG_ONLY classes read the log");
}

#[test]
fn a_duplicate_upload_stays_a_store_duplicate() {
    assert_eq!(stats::classify("Error: builds upload: The bundle version must be higher than the previously uploaded version"), "store_duplicate");
}

/// The 2026-11-04..05 window's `other` runs (release retro 82883418505664604761), from their real tails.
#[test]
fn tails_of_the_2026_11_05_window_classify() {
    let reauth = "receipt /x/canary-releases/2026-11-04T13-16-24.948Z-896a97589.json\n  \u{1b}[31m✗\u{1b}[0m gcloud builds describe a029bde7 failed: ERROR: (gcloud.builds.describe) There was a problem refreshing your current auth tokens: Reauthentication failed. cannot prompt during non-interactive execution.\nPlease run:\n  $ gcloud auth login\nerror: script \"canary\" exited with code 1";
    assert_eq!(stats::classify(reauth), "gcloud_auth");
    let prebuilt = "[ship-web] geometry skipped: Storybook serves the main checkout, not /tmp/wt2/Mars_CRM\n[ship-web] gateway api · building on win through dockerd's own socket\n/Users/u/Library/Caches/cpu-build-gate/tools/mars-ops/99f2523532609f34a0d0a864/mars-ops: No such file or directory (os error 2)\nerror: script \"release\" exited with code 1";
    assert_eq!(stats::classify(prebuilt), "tool_missing");
    assert_eq!(stats::group_of("gcloud_auth"), "environment");
}

#[test]
fn tails_of_retro_82675048650038770105_classify() {
    // The three `other` tails of the 2026-11-05 02:25-10:06 window.
    let hang = "scripts/tests/page-map.test.ts:\n(pass) every Figma screen on page 973:2 has its routed screen declared by a file under web/src/screens [0.09ms]\ntimed out after 180000ms\nFull logs: /x/18-bun.stdout.log\nerror: script \"release\" exited with code 1";
    assert_eq!(stats::classify(hang), "test_timeout");
    let export = "** EXPORT FAILED **\nerror: exportArchive The request expected results but none were found\n  Error while generating code signature.\nError: xcodebuild -exportArchive for Mac Catalyst failed (team MS8684M3D6)";
    assert_eq!(stats::classify(export), "signing");
    let plan = "[cpu-build-gate] tooling origin/mastra 473eaea43b for mars-ops\nMissing source path: /w/Mars_CRM/rust/target/release/build/aws-lc-sys-fe87/out/v3_purp.o.tmp. Restore it in the explicit baseline or select an existing file.\nerror: script \"release\" exited with code 1";
    assert_eq!(stats::classify(plan), "plan_rejected");
    assert_eq!(stats::group_of("test_timeout"), "code");
}

/// The 2026-11-10..11 window's `other` run (release retro 86965964993744652745), from its real tail.
#[test]
fn an_app_store_connect_api_timeout_is_store_api() {
    let tail = "build 1.0.11580 (11580) uploaded; waiting for App Store Connect processing\nasc exited 1: Error: api: request failed: Get \"https://api.appstoreconnect.apple.com/v1/builds?filter%5Bapp%5D=6820882374\": context deadline exceeded\nHint: Increase the request timeout (e.g. set `ASC_TIMEOUT=90s`).";
    assert_eq!(stats::classify(tail), "store_api");
    // An upload refusal names the same host and stays store_upload.
    assert_eq!(stats::classify("Error: builds upload: failed to create file reservation: request failed: Post \"https://api.appstoreconnect.apple.com/v1/buildUploadFiles\": context deadline exceeded"), "store_upload");
}

/// The 2026-11-11..12 window's two `other` runs (release retro 811725492735605841448), from their real tails.
#[test]
fn a_short_volume_after_cleanup_and_a_page_url_path_are_classified() {
    let disk = "[ship-web] Stage 1 (S0) watchdog cleanup finished (386 item(s) reclaimed) and watched volume is still below 100 GiB free. Hand off to AI Stage 2\n  \u{1b}[31m✗\u{1b}[0m k8s build-crm exited 1\nerror: script \"k8s\" exited with code 1";
    assert_eq!(stats::classify(disk), "disk_guard");
    let url = "[cpu-build-gate] tooling origin/mastra dba42730cc for mars-ops (sources: /Volumes/MarsWork/Workspaces/Mars_agent/Mars_CRM)\n/compatibility-quiz-for-couples is outside /Volumes/MarsWork/Workspaces/Mars_agent/Mars_CRM\nerror: script \"release\" exited with code 1";
    assert_eq!(stats::classify(url), "outside_checkout");
    assert_eq!(stats::group_of("outside_checkout"), "input");
}

/// The 2026-11-12 window's `other` run (release retro 817307802574154054683), from its real tail: Xcode's
/// export packaging step decoded an HTML reply from App Store Connect as JSON.
#[test]
fn an_export_that_reads_a_non_json_store_reply_is_store_api() {
    let tail = "2026-11-12 20:26:25.027 xcodebuild[23757:173232426] [MT] IDEDistribution: -[IDEDistributionLogging _createLoggingBundleAtPath:]: Created bundle at path \"/var/folders/x/MarsCRM.xcdistributionlogs\".\nerror: exportArchive The data couldn’t be read because it isn’t in the correct format.\n** EXPORT FAILED **\nError: xcodebuild -exportArchive for Mac Catalyst failed (team MS8684M3D6)";
    assert_eq!(stats::classify(tail), "store_api");
    assert_eq!(stats::group_of("store_api"), "environment");
}

#[test]
fn tails_of_the_2026_11_14_other_runs_classify() {
    // Release retro 88275358900259155475: the four `other` runs of that window, real tail lines.
    let cases = [
        ("#31 15.31 Built CSS verified: 97 Mars design tokens emitted.\ntime=\"2026-11-14T19:29:20+08:00\" level=error msg=\"error waiting for container: unexpected EOF\"", "beta_cluster"),
        ("git -C /x/Mars_CRM fetch --quiet origin mastra exited 128 after 15494ms\nstdout: (empty)\nstderr: fatal: unable to access 'https://github.com/Mars-CRM-Agent/Mars_agent.git/': LibreSSL SSL_connect: SSL_ERROR_SYSCALL in connection to github.com:443 ", "git_network"),
        ("Output directory already exists: /x/scratchpad/release-cache-exp. Choose a fresh release directory.", "plan_rejected"),
    ];
    for (tail, class) in cases {
        assert_eq!(stats::classify(tail), class, "{tail}");
    }
    assert_eq!(stats::group_of("git_network"), "environment");
}

/// The 2026-11-17..18 window's `other` run (release retro 82097144209858832811), from its real tail.
#[test]
fn a_bun_offline_cache_miss_is_dependency_cache() {
    let tail = "error: --offline: \"is-weakmap\" is not in the cache\nerror: --offline: \"is-weakset\" is not in the cache\nFull logs: /x/4-bun.stdout.log, /x/4-bun.stderr.log\nAudit logs: /x\nLogs: /x\nerror: script \"release\" exited with code 1";
    assert_eq!(stats::classify(tail), "dependency_cache");
    assert_eq!(stats::group_of("dependency_cache"), "environment");
}

/// A SIGTERM'd beta prepare whose gate printed a tooling note naming "not a git repository" (same retro).
#[test]
fn a_bun_sigterm_is_stopped_not_plan_rejected() {
    let tail = "[cpu-build-gate] tooling: git rev-parse --path-format=absolute --git-common-dir failed: fatal: not a git repository (or any of the parent directories): .git; running this checkout's own build\n[cpu-build-gate] starting after 13 s of waiting: 0 s for a slot, 12 s for the CPU window\nerror: script \"release\" was terminated by signal SIGTERM (Polite quit request)";
    assert_eq!(stats::classify(tail), "stopped");
}
