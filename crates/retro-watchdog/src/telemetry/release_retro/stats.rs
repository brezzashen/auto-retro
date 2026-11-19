//! Release retro statistics: pure functions over one window of finished release-board runs and the
//! evidence around them — `cpu-build-gate` waits printed in each run's log, the measured queue
//! waits release_watch records, gated builds (`slot-events.jsonl`, `docker-builds/`), TestFlight
//! publishes (`performance.jsonl`), Docker restarts and the release judges the watchdog opened.
//! Every figure is counted, never estimated; a wait the logs do not time is reported as a count.
use regex::Regex;
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, sync::LazyLock};

/// Escalation rows the watchdog detects by script; the others are still judged by hand.
pub(crate) const SCRIPTED_TRIGGERS: [&str; 8] = ["T1", "T4", "T5", "T6", "T8", "T9", "T10", "T11"];
pub(crate) const MANUAL_TRIGGERS: [&str; 4] = ["T2", "T3", "T7", "T12"];

static GATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"\[cpu-build-gate\] starting after (\d+) s of waiting: (\d+) s for a slot, (\d+) s for the CPU window").unwrap());
static RUN_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{8}T\d{6}Z-[a-z-]+-\d+").unwrap());
/// Queue signals a log names without timing them: `(key, lowercase needle)`.
const SIGNALS: [(&str, &str); 9] = [
    ("deploy_lease", "before it expires) — waiting up to"),
    ("build_lease", "[build-lease] waiting for"),
    ("verify_lease", "[ship-web] waiting for ship-web-verify"),
    ("testflight_lock", "waiting for the testflight"),
    ("canary_rollout_queue", "queued for a canary rollout"),
    ("shared_prepare", "is being prepared by pid"),
    ("upload_join", "which is uploading; waiting for that upload"),
    ("merged_build", "identical image build in flight"),
    // The release's own tooling waited for a tool-prebuild another process runs (cpu-build-gate latest_tool.rs).
    ("tool_prebuild", "tooling: waiting for the running prebuild"),
];
/// `(class, owner group, lowercase needles)`; first match wins over the failure tail (the last
/// lines of the run's log). Needles come from the board's real tails (2026-10-25..29); a bare
/// word such as "lease" is unsafe because every tail names its `mars-crm-release-logs` directory.
const FAILURES: [(&str, &str, &[&str]); 47] = [
    // Bun names a SIGTERM its own way; the gate's "not a git repository" tooling note above it read as
    // plan_rejected (20261117T141624Z-beta-75796, release retro 82097144209858832811).
    ("stopped", "operator", &["stopped by sigterm", "stopped by sigint", "terminated by signal sigterm"]),
    // Xcode's ibtool/actool service crashed mid-archive; the same commit's iOS and Mac archives both failed
    // and the next commit, with no asset change, archived (release retro 811587194803931207163).
    ("xcode_tool_crash", "environment", &["compileassetcatalogvariant failed", "ibabstractplatformtoolproxy"]),
    ("publisher_exited", "operator", &["closed: publisher pid"]),
    ("candidate_t6", "policy", &["t6 "]),
    // A migration file stem passed as --migrate, refused before any build (20261118T020502Z-canary-92386,
    // canary-retro 87924700070451635977, which classified it `other`).
    ("migrate_flag", "input", &["takes 14-digit migration versions"]),
    ("migration_gate", "policy", &["--migrate=<versions>", "rerun with --migrate="]),
    // "watched volume is still below": ship-web's own S0 cleanup ran and the volume stayed short
    // (20261111T181507Z-beta-crm-11810, release retro 811725492735605841448).
    ("disk_guard", "environment", &["disk-space guard refused", "cleanup left the docker vm short", "no space left", "watched volume is still below"]),
    ("generated_drift", "tooling", &["the checked-in json is what the generator writes today"]),
    ("stale_base", "input", &["prepare again"]),
    ("plan_rejected", "input", &["unsupported removed path", "unsupported scoped", "unsupported source symlink", "expected a regular source file",
        "missing declared dependency", "requires a validated package", "not a git repository",
        // An agent plan without an offline test value (beta 20261102T112601Z-beta-59510).
        "missing test parameter",
        // A plan whose output directory a previous run of the session had already written (20261114T172056Z-beta-17021).
        "output directory already exists",
        // A plan naming a transient cargo build file that was gone at release time (20261105T092820Z-beta-99108).
        "restore it in the explicit baseline"]),
    ("unpushed", "input", &["commit and push it first", "push it first"]),
    // A page URL passed where deliver beta-web takes source paths (20261112T003759Z-beta-web-80904, release retro 811725492735605841448).
    ("outside_checkout", "input", &[" is outside /"]),
    ("missing_source", "input", &["is neither in the workspace nor in the serving source"]),
    ("dependency_manifest", "input", &["needs a full image build", "differs from support dependencies"]),
    // A release CLI that could not spawn its tool is tooling, not the cluster: the beta_cluster needle
    // "kubectl --context docker-desktop" also names that command (20261102T101004Z-beta-web-25592).
    ("tool_missing", "tooling", &["stderr: kubectl: no such file", "stderr: docker: no such file", "stderr: gcloud: no such file",
        // cpu-build-gate's prebuilt mars-ops was gone when ship-web spawned it (20261104T062152Z-beta-web-44846).
        "/mars-ops: no such file or directory"]),
    // The gcloud user credential needed reauthentication mid-release (20261104T131556Z-canary-27338,
    // 20261104T131716Z-canary-44638, 20261029T110503Z-canary-26421); `gcp-reauth` restores it.
    ("gcloud_auth", "environment", &["reauthentication failed", "problem refreshing your current auth tokens"]),
    // The npm registry mirror answered HTTP 5xx during Cloud Build offline validation (Canary
    // 20261109T145533Z-canary-79731, 20261109T151033Z-canary-46553; fixed in Mars_CRM e1f7179bbc).
    ("package_registry", "environment", &["error: failed to download "]),
    // A beta prepare's `bun install --offline` found 897 packages missing from the host Bun cache
    // (20261118T090720Z-beta-6032, release retro 82097144209858832811); cause not proven.
    ("dependency_cache", "environment", &["is not in the cache"]),
    // The path to Google Cloud stalled during Canary releases (2026-11-11 05:34..06:38 UTC, incident
    // 87437886529223453292): the GKE API, a GCS source upload and the canary health URL. Ahead of
    // beta_cluster, whose "tls handshake timeout" needle would otherwise name the local beta cluster.
    ("gcp_network", "environment", &["--context gke_", "gcloud.builds.submit) 400 could not upload file",
        "gcloud builds submit timed out", "error sending request for url (https://canary.example.com"]),
    // The host could not reach GitHub while a release fetched origin/mastra (20261114T160204Z-beta-web-655,
    // 20261114T160640Z-beta-web-39091: LibreSSL SSL_ERROR_SYSCALL).
    ("git_network", "environment", &["ssl_error_syscall", "unable to access 'https://github.com/"]),
    ("beta_cluster", "environment", &["kubectl --context docker-desktop", "invalid tar header", "executable not found in $path",
        "tls handshake timeout", "no such object: mars-crm-agent", "error response from daemon",
        // The beta Docker engine over its ssh context was down (20261103T073835Z-beta-17909).
        "docker system dial-stdio",
        // The Windows Docker host restarted mid-build (20261114T112821Z-beta-web-67086; docker-heals remote_host_restart 11:29Z).
        "error waiting for container: unexpected eof",
        // ship-web's Docker VM probe during the same beta host outage (20261103T075656Z-beta-99960).
        "docker vm probe failed",
        // mars-ops k8s deploy found no local cluster (20261104T032429Z-beta-web-47848).
        "is unavailable; run `mars k8s up` first"]),
    // The beta engine had lost a kept service's image the node still ran (5 beta-web runs,
    // 20261107T031131Z..033717Z; mars-ops ensure_loaded now keeps it from the node).
    ("engine_image_missing", "environment", &["no local image mars-crm/"]),
    // The post-rollout Redis check ran before the sentinels saw every node (3 runs, 2026-11-06 12:39 UTC).
    ("redis_sentinel", "environment", &["sentinels have not discovered every node"]),
    // A migration the database refused during a Canary release (20261102T120223Z-canary-62313).
    ("migration", "code", &["pending trigger events", "cannot create index"]),
    // The core regression suite failed against the rolled-out commit, which was then rolled back: the
    // product broke, not the rollout (20261115T033907Z-canary-89669: /api/me 503, crm.plan_price not
    // granted to crm_app, fixed by 2e9dc1fdb7; release retro 810781605592223812110). Ahead of rollout,
    // whose "canary rollout failure" line the same tail prints.
    ("regression_gate", "code", &["\"event\":\"regression_run\""]),
    ("rollout", "environment", &["is mid-rollout", "rollout did not complete", "rollout failed; previous revisions restored",
        "canary rollout failure", "exceeded its progress deadline"]),
    ("storybook", "environment", &["restore the resident storybook", "localhost:6006"]),
    // Another session's half-added crate in the shared Cargo workspace (20261103T143820Z-beta-web-7407).
    ("workspace_manifest", "input", &["failed to parse manifest"]),
    ("rust_compile", "code", &["could not compile", "error[e"]),
    ("typecheck", "code", &["error ts"]),
    ("module_missing", "tooling", &["cannot find module"]),
    ("tooling_parse", "tooling", &["eof while parsing", "missing field `"]),
    ("ios_standard", "code", &["shared standard broken"]),
    ("mac_archive", "code", &["mac archive is missing"]),
    ("archive_worktree", "tooling", &["is a missing but locked worktree"]),
    // Mac Catalyst -exportArchive could not sign (20261105T065122Z-mac-30682).
    ("signing", "tooling", &["codesign exited", "error while generating code signature"]),
    ("store_duplicate", "tooling", &["redundant binary upload", "bundle version must be higher"]),
    ("store_processing", "environment", &["still processing after"]),
    // App Store Connect refused or timed out the package upload (3 Mac runs, 2026-11-03).
    ("store_upload", "environment", &["builds upload: ", "asc builds upload --pkg failed", "asc builds upload --ipa failed"]),
    // An App Store Connect API read timed out while the build waited for processing (20261110T133658Z-ios-29398;
    // mars-ui-tools poll_build now polls through it, Mars_CRM e78816e797).
    // Xcode's export packaging step read an HTML page where App Store Connect JSON was due
    // (20261112T122527Z-mac-18650: IDEDistribution.verbose.log "DecodingError.dataCorrupted ... Unexpected character '<'").
    ("store_api", "environment", &["https://api.appstoreconnect.apple.com/", "exportarchive the data couldn’t be read because it isn’t in the correct format", "exportarchive the data couldn't be read because it isn't in the correct format"]),
    // A test that failed inside Cloud Build offline validation is a test, not a build failure (2026-10-28..11-01).
    // A test file that threw while loading wiki media (20261107T032738Z-beta-web-65734) is a test too, and so is a
    // Canary refusal quoting a prepare whose bun assertion failed (20261116T082128Z-canary-96012, its tail cut before "(fail)").
    ("test", "code", &["(fail)", "expect() calls", "error: expect(received)", " test across", " tests across", ".test.ts:\ntimed out after", "missing media image dimensions"]),
    // A selected bun test file never reported within ship-web's 180 s test step (20261105T053420Z-beta-web-19882).
    ("test_timeout", "code", &["timed out after 180000ms"]),
    ("cloud_build", "code", &["canary build failure", "failed to solve", "mars_crm_release_failed"]),
    ("geometry", "code", &["px, expected ", "glyph ink, expected "]),
    // The release CLI refused an image whose WorkingDir it does not know (4 beta-web runs, 2026-10-27).
    ("image_layout", "tooling", &["unsupported image workingdir"]),
    // After the classes above: a gate that lost this race may fall back to an ungated build that fails later.
    ("gate_race", "tooling", &["[cpu-build-gate] write ", "[cpu-build-gate] rename onto "]),
    // Only a run that never got past the gate; ship-web echoes gate lines before any later failure.
    ("gate_wait_end", "operator", &["[cpu-build-gate] cpu", "[cpu-build-gate] disk warning"]),
];

const GATE_READY: &str = "[cpu-build-gate] cpu ready";

pub(crate) fn group_of(class: &str) -> &'static str {
    FAILURES.iter().find(|(c, _, _)| *c == class).map_or("unknown", |(_, g, _)| g)
}

pub(crate) fn ms(text: Option<&str>) -> Option<f64> {
    text.and_then(crate::cause_tree::attribute::parse_time)
}

/// When a run left the board: its finish time, else its last log line, else its last update. An
/// abandoned run keeps its last log line when `deliver hygiene` closes it days later and rewrites
/// `updatedAt`, so it is not counted a second time in a later window.
pub(crate) fn end_ms(run: &Value) -> Option<f64> {
    ms(run["finishedAt"].as_str()).or_else(|| ms(run["progress"]["at"].as_str())).or_else(|| ms(run["updatedAt"].as_str()))
}

pub(crate) fn spread(mut values: Vec<f64>) -> Value {
    if values.is_empty() { return json!({"n": 0}); }
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    json!({"n": values.len(), "p50": at(0.5).round(), "p90": at(0.9).round(), "max": at(1.0).round(), "total": values.iter().sum::<f64>().round()})
}

pub(crate) fn rate(part: usize, whole: usize) -> Value {
    if whole == 0 { Value::Null } else { json!((part as f64 * 1000.0 / whole as f64).round() / 10.0) }
}

pub(crate) fn classify(tail: &str) -> &'static str {
    let lower = tail.to_lowercase();
    // Its last gate line admitted the run: a later failure is not a gate wait that ended.
    let passed_gate = lower.rfind(GATE_READY).is_some_and(|at| lower.rfind("[cpu-build-gate] cpu") == Some(at));
    FAILURES.iter().filter(|(class, _, _)| !(passed_gate && *class == "gate_wait_end"))
        .find(|(_, _, needles)| needles.iter().any(|n| lower.contains(n))).map_or("other", |(class, _, _)| class)
}

/// Classes whose cause a Mac archive prints far above the tail it ends with (xcodebuild streams
/// its warnings after the failing step; 20261104T041714Z-mac-90382 had it 37 lines up).
const LOG_ONLY: [&str; 1] = ["xcode_tool_crash"];

/// `classify(tail)`, and for a tail that names nothing, the `LOG_ONLY` classes over the whole log.
pub(crate) fn classify_with_log(tail: &str, log: &str) -> &'static str {
    match classify(tail) {
        "other" => {
            let lower = log.to_lowercase();
            FAILURES.iter().filter(|(class, _, _)| LOG_ONLY.contains(class))
                .find(|(_, _, needles)| needles.iter().any(|n| lower.contains(n))).map_or("other", |(class, _, _)| class)
        }
        class => class,
    }
}

/// One run's outcome and its waits read from its log text.
pub(crate) fn run_row(run: &Value, outcome: &str, log: &str) -> Value {
    let (mut gate, mut slot, mut cpu) = (Vec::new(), 0.0, 0.0);
    for c in GATE.captures_iter(log) {
        let n = |i: usize| c[i].parse::<f64>().unwrap_or(0.0);
        gate.push(n(1));
        slot += n(2);
        cpu += n(3);
    }
    let lower = log.to_lowercase();
    let signals: Map<String, Value> = SIGNALS.iter().map(|(k, needle)| (k.to_string(), json!(lower.matches(needle).count()))).collect();
    let seconds = match (ms(run["startedAt"].as_str()), end_ms(run)) { (Some(a), Some(b)) => json!(((b - a) / 1000.0).round()), _ => Value::Null };
    let tail = run["failure"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("\n");
    json!({"id": run["id"], "target": run["target"], "queue": run["queue"], "outcome": outcome, "seconds": seconds,
        "gate_wait_s": gate.iter().sum::<f64>(), "gate_slot_s": slot, "gate_cpu_s": cpu, "gate_waits": gate.len(), "signals": signals,
        "failure": (outcome == "failed").then(|| classify_with_log(&tail, log)),
        "meta": {"purpose": run["purpose"].as_str().is_some_and(|s| !s.is_empty()), "task": !run["task"].is_null(),
            "source": run["source"].as_str().is_some_and(|s| s.len() == 40), "tab": !run["creator"]["orcaTab"].is_null()}})
}

/// Everything outside the board that the window's time range selects.
#[derive(Default)]
pub(crate) struct Evidence {
    pub slot_events: Vec<Value>,
    pub docker_actions_ms: Vec<f64>,
    pub judges: Vec<Value>,
    pub testflight: Vec<Value>,
    pub waits: Vec<Value>,
    /// `startedAt` of every iOS and Mac board run, to tell which publishes the board saw.
    pub board_ios_starts: Vec<String>,
}

fn group(rows: &[Value], key: &str) -> Value {
    let mut out: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for r in rows { out.entry(r[key].as_str().unwrap_or("?").to_string()).or_default().push(r); }
    json!(out.into_iter().map(|(k, rs)| {
        let n = |o: &str| rs.iter().filter(|r| r["outcome"] == o).count();
        (k, json!({"runs": rs.len(), "succeeded": n("succeeded"), "failed": n("failed"), "abandoned": n("abandoned"),
            "success_rate": rate(n("succeeded"), rs.len()), "seconds": spread(rs.iter().filter_map(|r| r["seconds"].as_f64()).collect()),
            "gate_wait_s": spread(rs.iter().filter_map(|r| r["gate_wait_s"].as_f64()).filter(|s| *s > 0.0).collect())}))
    }).collect::<Map<_, _>>())
}

fn build_kind(command: &str) -> String {
    let words: Vec<&str> = command.split_whitespace().take(2).collect();
    match words.as_slice() {
        ["docker", second] => format!("docker {second}"),
        ["xcodebuild", ..] => "xcodebuild".into(),
        [first, ..] => first.rsplit('/').next().unwrap_or(first).to_string(),
        [] => "?".into(),
    }
}

fn builds(events: &[Value]) -> Value {
    let mut kinds: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for e in events { kinds.entry(build_kind(e["command"].as_str().unwrap_or(""))).or_default().push(e); }
    let secs = |es: &[&Value], k: &str| spread(es.iter().filter_map(|e| e[k].as_f64()).map(|v| v / 1000.0).collect());
    json!(kinds.into_iter().map(|(k, es)| (k, json!({"n": es.len(), "waited_s": secs(&es, "waitedMs"),
        "cpu_wait_s": secs(&es, "cpuWaitMs"), "slot_wait_s": secs(&es, "slotWaitMs"),
        "waited_over_60s": es.iter().filter(|e| e["waitedMs"].as_f64().is_some_and(|w| w >= 60_000.0)).count()}))).collect::<Map<_, _>>())
}

/// Judge ids per run: every release-board id a judge's signatures name.
fn judged(judges: &[Value]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for j in judges.iter().filter(|j| j["started"] == true) {
        for s in j["signatures"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            for id in RUN_ID.find_iter(s) { out.entry(id.as_str().into()).or_default().push(s.split(':').next().unwrap_or("?").into()); }
        }
    }
    out
}

pub(crate) fn summarize(rows: &[Value], ev: &Evidence, from_ms: f64, to_ms: f64) -> Value {
    let judged = judged(&ev.judges);
    let judged_runs = rows.iter().filter(|r| r["id"].as_str().is_some_and(|id| judged.contains_key(id))).count();
    let mut codes: BTreeMap<String, usize> = BTreeMap::new();
    for j in ev.judges.iter().filter(|j| j["started"] == true) {
        let code = j["signatures"][0].as_str().and_then(|s| s.split(':').next()).unwrap_or("?");
        *codes.entry(code.to_string()).or_default() += 1;
    }
    let mut failures: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for r in rows.iter().filter(|r| r["outcome"] == "failed") {
        failures.entry(r["failure"].as_str().unwrap_or("other").into()).or_default().push(r["id"].clone());
    }
    let signals: Map<String, Value> = SIGNALS.iter().map(|(k, _)| (k.to_string(), json!({
        "signals": rows.iter().map(|r| r["signals"][k].as_u64().unwrap_or(0)).sum::<u64>(),
        "runs": rows.iter().filter(|r| r["signals"][k].as_u64().unwrap_or(0) > 0).count()}))).collect();
    let meta = |k: &str| rate(rows.iter().filter(|r| r["meta"][k] == true).count(), rows.len());
    let on_board = ev.testflight.iter().filter(|p| p["startedAt"].as_str().is_some_and(|s| ev.board_ios_starts.iter().any(|b| b == s))).count();
    let mut waits_by_env: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for w in &ev.waits { waits_by_env.entry(w["env"].as_str().unwrap_or("?").into()).or_default().push(w["waited_s"].as_f64().unwrap_or(0.0)); }
    json!({
        "window": {"runs": rows.len(), "from_ms": from_ms, "to_ms": to_ms, "hours": ((to_ms - from_ms) / 360_000.0).round() / 10.0},
        "by_target": group(rows, "target"), "by_queue": group(rows, "queue"),
        "waits": {
            "gate": {"runs_waited": rows.iter().filter(|r| r["gate_waits"].as_u64().unwrap_or(0) > 0).count(),
                "per_run_s": spread(rows.iter().filter_map(|r| r["gate_wait_s"].as_f64()).filter(|s| *s > 0.0).collect()),
                "slot_s": rows.iter().filter_map(|r| r["gate_slot_s"].as_f64()).sum::<f64>(), "cpu_s": rows.iter().filter_map(|r| r["gate_cpu_s"].as_f64()).sum::<f64>()},
            "untimed_signals": signals,
            "measured_queue": {"all": spread(ev.waits.iter().filter_map(|w| w["waited_s"].as_f64()).collect()),
                "by_env": waits_by_env.into_iter().map(|(k, v)| (k, spread(v))).collect::<Map<_, _>>()},
            "testflight_pre_script_s": spread(ev.testflight.iter().filter_map(|p| p["preScriptWaitMs"].as_f64()).map(|v| v / 1000.0).collect()),
        },
        "failure_groups": failures.iter().fold(BTreeMap::<&str, usize>::new(), |mut m, (k, ids)| { *m.entry(group_of(k)).or_default() += ids.len(); m }),
        "failures": failures.into_iter().map(|(k, ids)| { let group = group_of(&k); (k, json!({"n": ids.len(), "group": group, "runs": ids})) }).collect::<Map<_, _>>(),
        "scripted": {"runs": rows.len(), "judged_runs": judged_runs, "script_only_rate": rate(rows.len() - judged_runs, rows.len()),
            "judges": ev.judges.iter().filter(|j| j["started"] == true).count(), "judges_by_code": codes},
        "coverage": {
            "metadata_rate": {"purpose": meta("purpose"), "task": meta("task"), "source": meta("source"), "tab": meta("tab")},
            "triggers": {"scripted": SCRIPTED_TRIGGERS, "manual": MANUAL_TRIGGERS, "rate": rate(SCRIPTED_TRIGGERS.len(), SCRIPTED_TRIGGERS.len() + MANUAL_TRIGGERS.len())},
            "testflight": {"publishes": ev.testflight.len(), "on_board": on_board, "off_board": ev.testflight.len() - on_board,
                "rate": rate(on_board, ev.testflight.len()),
                "outcomes": ev.testflight.iter().fold(BTreeMap::<String, usize>::new(), |mut m, p| { *m.entry(p["outcome"].as_str().unwrap_or("?").into()).or_default() += 1; m })},
        },
        "builds": {"gated": ev.slot_events.len(), "by_kind": builds(&ev.slot_events)},
        "docker_restarts": ev.docker_actions_ms.len(),
    })
}
