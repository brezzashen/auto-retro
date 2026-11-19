//! Canary retro analysis: pure functions over one window of finished Canary board runs and the
//! release receipt each run wrote (`~/.local/state/mars-crm/canary-releases/*.json`; the run log
//! prints its path as `receipt <path>`).
//!
//! Founder targets (2026-10-31, replacing 300 s / 900 s of 2026-10-28): a Canary release takes about
//! 5-10 min end to end (TARGET_S is the 10 min top of that band) and never more than HARD_S
//! (20 min); every run past HARD_S names the critical-path segment that cost the time. Three
//! checks answer the founder's anomaly questions: errors, a cache not deployed correctly (Rust
//! build cache missed, restore or save failed, an unchanged image rebuilt or rolled again), and
//! queue problems (board start, Cloud Build queue, rollout queue, build gate). A fourth check, `ci` (`ci.rs`),
//! names what a check ahead of the release could have caught or taken off the release path.
use super::super::release_retro::stats::{self, rate, spread};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, path::Path, sync::LazyLock};

pub(crate) const TARGET_S: f64 = 600.0;
pub(crate) const HARD_S: f64 = 1200.0;
/// rust-build with its cache restored took 12-19 s; a miss compiled for 285-1186 s (receipts 2026-10-28). Since
/// 2026-10-31 it compiles on the builder VM, whose SSD keeps the target, and has no restore or save step.
pub(crate) const RUST_CACHE_HIT_MAX_S: f64 = 120.0;
/// Cloud Build normally queues 47-50 s; longer means the prepare waited on the pool.
pub(crate) const BUILD_QUEUE_MAX_S: f64 = 120.0;
/// Board start to receipt start (candidate check, tooling build, CPU gate) normally takes about 15 s
/// (7-56 s over the live receipts of 2026-10-29..11-01). No release queue runs in it: longer is a
/// queue only when the CPU gate waited, else a stall (20261101T073812Z-canary-57304: 1158 s, its
/// candidate check's `git fetch` blocked on a wedged fsmonitor daemon).
pub(crate) const START_MAX_S: f64 = 120.0;
/// A live run's prepare beyond the longer of preflight and Cloud Build (archive, upload, slot
/// waits) took 34-72 s over the 47 live receipts of 2026-10-29..11-01; 57304 had 951 s (mars-ops
/// `git fetch` blocked on the fsmonitor daemon before its preflight started).
pub(crate) const PREPARE_SLACK_MAX_S: f64 = 120.0;
/// Prepared to rollout start: time spent behind another leader or the deploy lease.
pub(crate) const ROLLOUT_WAIT_MAX_S: f64 = 120.0;
/// A rolling update of the changed images takes about 84 s.
pub(crate) const ROLLOUT_MAX_S: f64 = 180.0;
/// `gcloud builds submit` upload to build creation: p99 270 s over 150 canary builds (2026-11-10/11); the
/// slow GCS path of 2026-11-11 took 336-787 s (run 20261111T062302Z-canary-3019: 787 s of its 1020 s prepare).
pub(crate) const UPLOAD_MAX_S: f64 = 300.0;
const SEGMENTS: [&str; 5] = ["start", "prepare", "rollout_wait", "rollout", "finish"];
const FAILED_STEP: [&str; 4] = ["FAILURE", "TIMEOUT", "INTERNAL_ERROR", "EXPIRED"];

static RECEIPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"receipt (/[^\s\x1b]+\.json)").unwrap());
static NULL: Value = Value::Null;

/// The receipt the run wrote last (a retried run prints one per attempt).
pub(crate) fn receipt_path(log: &str) -> Option<String> {
    RECEIPT.captures_iter(log).last().map(|c| c[1].to_string())
}

/// Live receipts as (live-at ms, tags), oldest first: what was live when a later run started.
pub(crate) fn live_tags(dir: &Path) -> Vec<(f64, Value)> {
    let mut out: Vec<(f64, Value)> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| serde_json::from_slice::<Value>(&std::fs::read(e.path()).ok()?).ok())
        .filter(|r| r["outcome"] == "live")
        .filter_map(|r| Some((stats::ms(r["rollout"]["liveAt"].as_str()).or_else(|| stats::ms(r["startedAt"].as_str()))?, r["tags"].clone())))
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

fn secs(a: Option<f64>, b: Option<f64>) -> Option<f64> { Some(((b? - a?) / 1000.0).round()) }

fn step<'a>(receipt: &'a Value, id: &str) -> &'a Value {
    receipt["build"]["steps"].as_array().and_then(|s| s.iter().find(|s| s["id"] == id)).unwrap_or(&NULL)
}

/// Preflight seconds spent waiting rather than running steps: the install runs first, then the other chains
/// at once. With `cpuWaitSeconds` per step (mars-ops since 2026-11-04) it is the measured CPU-window wait
/// (`preflight_cpu_wait`); an older receipt leaves the wall time beyond the install plus the longest other
/// chain (`preflight_wait`). 9e818e29b's prepare (2026-11-04): 1697 s wall, 35 + 258 s of steps, the rest
/// queued behind Blender holding the host at >= 95 % CPU.
fn preflight_wait(receipt: &Value) -> Option<(&'static str, f64)> {
    let steps = receipt["preflight"]["steps"].as_array().filter(|s| !s.is_empty())?;
    // mars-ops `PREPARES_NEXT`: these steps run in one chain with the step after them.
    const PREPARES_NEXT: [&str; 2] = ["build agent CRM fixture", "prepare script test sandbox"];
    let along = |key: &str| {
        let (mut longest, mut chain, mut joins) = (0.0_f64, 0.0, false);
        for s in &steps[1..] {
            chain = if joins { chain } else { 0.0 } + s[key].as_f64().unwrap_or(0.0);
            joins = s["label"].as_str().is_some_and(|l| PREPARES_NEXT.contains(&l));
            longest = longest.max(chain);
        }
        steps[0][key].as_f64().unwrap_or(0.0) + longest
    };
    if steps.iter().any(|s| s["cpuWaitSeconds"].is_number()) {
        return Some(("preflight_cpu_wait", along("cpuWaitSeconds")));
    }
    Some(("preflight_wait", (receipt["preflight"]["seconds"].as_f64()? - along("seconds")).max(0.0)))
}

/// Images of `list` whose tag equals the one already live: a rebuild or roll the cache should have skipped.
fn unchanged(receipt: &Value, live_before: &Value, list: &str) -> Vec<Value> {
    receipt[list].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|img| {
        receipt["tags"][img].as_str().is_some_and(|t| !t.is_empty() && live_before[img].as_str() == Some(t))
    }).map(|img| json!(img)).collect()
}

/// One run: its critical-path segments, prepare figures, verdict against the targets, the three checks and the cause.
pub(crate) fn run_row(run: &Value, outcome: &str, log: &str, receipt: &Value, live_before: &Value) -> Value {
    let base = stats::run_row(run, outcome, log);
    let t = |v: &Value| stats::ms(v.as_str());
    let (board_start, board_end) = (t(&run["startedAt"]), stats::end_ms(run));
    let (queued_at, live_at) = (t(&receipt["queuedAt"]), t(&receipt["rollout"]["liveAt"]));
    let rollout_s = receipt["rollout"]["runSeconds"].as_f64();
    // A run that failed before it was prepared spent its receipt's seconds preparing.
    let prepare_s = secs(t(&receipt["startedAt"]), queued_at).or_else(|| receipt["seconds"].as_f64().filter(|_| receipt["outcome"] != "live"));
    let segments = json!({"start": secs(board_start, t(&receipt["startedAt"])), "prepare": prepare_s,
        "rollout_wait": secs(queued_at, live_at).map(|s| s - rollout_s.unwrap_or(0.0)), "rollout": rollout_s, "finish": secs(live_at, board_end)});
    let rust_build = step(receipt, "rust-build")["seconds"].as_f64();
    let build_queue = receipt["build"]["queuedSeconds"].as_f64();
    let upload = receipt["build"]["uploadSeconds"].as_f64();
    // A rider's own upload before its queued build was cancelled (Mars_CRM `rode.uploadSeconds`); `build` is the cover's.
    let rode_upload = receipt["rode"]["uploadSeconds"].as_f64();
    let prepare = json!({"build_id": receipt["build"]["id"], "preflight_s": receipt["preflight"]["seconds"], "build_queue_s": build_queue, "build_run_s": receipt["build"]["runSeconds"], "upload_s": upload, "rode_upload_s": rode_upload,
        "rust_build_s": rust_build, "rust_restore_s": step(receipt, "rust-restore")["seconds"], "build_status": receipt["build"]["status"]});
    let live = receipt["outcome"] == "live";
    let has_build = receipt["build"]["steps"].as_array().is_some_and(|s| !s.is_empty());

    // A rider its creator stopped waited on its leader: a queue item and cause, not an error (retry.rs).
    let rider_stop = super::retry::stopped_rider(&base["failure"], log);
    let mut errors = Vec::new();
    if outcome != "succeeded" && !rider_stop {
        let tail: String = receipt["failure"][0].as_str().unwrap_or("").chars().take(300).collect();
        errors.push(json!({"kind": outcome, "class": base["failure"], "tail": tail}));
    }
    let failed_steps: Vec<Value> = receipt["build"]["steps"].as_array().into_iter().flatten()
        .filter(|s| s["status"].as_str().is_some_and(|st| FAILED_STEP.contains(&st))).map(|s| s["id"].clone()).collect();
    if !failed_steps.is_empty() { errors.push(json!({"kind": "build_step", "steps": failed_steps})); }
    if live && receipt["health"]["status"].as_str().is_some_and(|s| s != "ok") { errors.push(json!({"kind": "health", "health": receipt["health"]})); }
    let retried = super::retry::retried_attempt(log);
    if let Some(class) = retried { errors.push(json!({"kind": "retried_attempt", "class": class})); }

    let mut cache = Vec::new();
    let cache_step_failed = |id: &str| has_build && !step(receipt, id).is_null() && step(receipt, id)["status"] != "SUCCESS";
    // A restored cache (or the builder VM's kept target) leaves only the changed workspace crates to
    // compile: Cloud Build fe1c8b2c and ff1ec3a0 (2026-10-28) restored every dependency and still spent
    // 9-12 min recompiling mars-crm-server. That is build time, not a cache that failed to deploy.
    if let Some(s) = rust_build.filter(|s| *s > RUST_CACHE_HIT_MAX_S) {
        let kind = if cache_step_failed("rust-restore") { "rust_cache_miss" } else { "rust_rebuild" };
        cache.push(json!({"kind": kind, "rust_build_s": s, "rust_restore_s": prepare["rust_restore_s"]}));
    }
    if cache_step_failed("rust-restore") { cache.push(json!({"kind": "rust_restore", "status": step(receipt, "rust-restore")["status"]})); }
    if live && cache_step_failed("rust-save") { cache.push(json!({"kind": "rust_save", "status": step(receipt, "rust-save")["status"]})); }
    // A run that reused another prepare built nothing itself, and a receipt without a rollout build
    // rolled nothing: `built`/`rolled` are the prepare's plan (20261102T153557Z-canary-82834 re-released
    // the commit 50272 had just put live and was flagged for both).
    let own_build = receipt["preparedBy"].is_null();
    let rolled = !receipt["rollout"]["build"].is_null() || receipt["rollout"]["runSeconds"].is_number();
    for (list, kind, did) in [("built", "unchanged_rebuilt", own_build), ("rolled", "unchanged_rolled", rolled)] {
        let images = if did { unchanged(receipt, live_before, list) } else { Vec::new() };
        if !images.is_empty() { cache.push(json!({"kind": kind, "images": images})); }
    }

    let mut queue = Vec::new();
    if rider_stop { queue.push(json!({"kind": "stopped_rider", "s": base["seconds"]})); }
    let mut stall = Vec::new();
    let over = |v: Option<f64>, max: f64| v.filter(|s| *s > max);
    // A start held by the gate, or by a tool-prebuild the release's tooling waited for (canary
    // 20261105T170817Z-canary-58402: 979 s behind a prebuild held in render-queue's exclusive memory
    // window, incident 816879591079598131716), is a queue wait, not an unexplained stall.
    let prebuild_waited = base["signals"]["tool_prebuild"].as_u64().is_some_and(|n| n > 0);
    let gate_waited = over(base["gate_wait_s"].as_f64(), 60.0).is_some() || prebuild_waited;
    if let Some(s) = over(segments["start"].as_f64(), START_MAX_S) {
        if gate_waited { queue.push(json!({"kind": "start_wait", "s": s})); } else { stall.push(json!({"kind": "start", "s": s})); }
    }
    // Preflight runs beside the source upload and Cloud Build; the longer one is the prepare segment's critical path.
    let build_path = upload.unwrap_or(0.0) + build_queue.unwrap_or(0.0) + prepare["build_run_s"].as_f64().unwrap_or(0.0);
    let preflight_s = prepare["preflight_s"].as_f64();
    let critical = preflight_s.unwrap_or(0.0).max(build_path);
    // A run that shared another session's prepare waited for all of it, while its receipt carries only that
    // prepare's last pass: the remainder is a queue wait on the preparer, not an unexplained stall.
    let mut shared_wait = None;
    if live && preflight_s.is_some() {
        if let Some(s) = over(prepare_s.map(|p| p - critical), PREPARE_SLACK_MAX_S) {
            if own_build {
                stall.push(json!({"kind": "prepare", "s": s}));
            } else {
                queue.push(json!({"kind": "shared_prepare", "s": s}));
                shared_wait = Some(s);
            }
        }
    }
    if let Some(s) = over(build_queue, BUILD_QUEUE_MAX_S) { queue.push(json!({"kind": "build_queue", "s": s})); }
    if let Some(s) = over(segments["rollout_wait"].as_f64(), ROLLOUT_WAIT_MAX_S) { queue.push(json!({"kind": "rollout_wait", "s": s})); }
    if let Some(s) = over(base["gate_wait_s"].as_f64(), 60.0) { queue.push(json!({"kind": "build_gate", "s": s})); }
    let waited = preflight_wait(receipt).filter(|(_, s)| *s > 60.0);
    if let Some((kind, s)) = waited { queue.push(json!({"kind": kind, "s": s})); }
    let slow_rollout = over(rollout_s, ROLLOUT_MAX_S);

    let seconds = base["seconds"].as_f64();
    let verdict = match seconds { None => "unknown", Some(s) if s <= TARGET_S => "target", Some(s) if s <= HARD_S => "over_target", _ => "over_hard" };
    let dominant = SEGMENTS.iter().filter_map(|k| Some((*k, segments[*k].as_f64()?))).max_by(|a, b| a.1.total_cmp(&b.1)).map(|d| d.0);
    let preflight_led = preflight_s.is_some_and(|p| p >= build_path);
    // Stalled seconds outweighing every working segment (the critical prepare path, rollout wait,
    // rollout) set the release time themselves: neither a queue nor the preflight caused them.
    let stall_s: f64 = stall.iter().filter_map(|s| s["s"].as_f64()).sum();
    let working = [Some(critical), segments["rollout_wait"].as_f64(), rollout_s].into_iter().flatten().fold(0.0, f64::max);
    let cause = match dominant {
        _ if rider_stop => "queue",
        _ if outcome != "succeeded" => "error",
        // A rider whose own upload crawled before its ride lost those seconds whatever the cover then did
        // (20261112T033259Z-canary-64843: 631 s own upload, then 783 s behind the cover's build).
        Some("prepare") if over(rode_upload, UPLOAD_MAX_S).is_some_and(|u| u > shared_wait.unwrap_or(0.0)) => "upload",
        _ if shared_wait.is_some_and(|s| s > working) => "queue",
        // A failed attempt deliver executed again holds the stall (retry.rs).
        _ if stall_s > working && retried.is_some() => "error",
        _ if stall_s > working => "other",
        Some("start" | "rollout_wait") => "queue",
        Some("rollout") => "rollout",
        // A preflight that mostly waited for the host held the release in a queue, not in its checks.
        Some("prepare") if preflight_led && waited.is_some_and(|(_, w)| 2.0 * w > preflight_s.unwrap_or(0.0)) => "queue",
        Some("prepare") if preflight_led => "preflight",
        Some("prepare") if cache.iter().any(|c| c["kind"] == "rust_cache_miss") => "cache",
        Some("prepare") if over(upload, UPLOAD_MAX_S).is_some_and(|u| 2.0 * u > build_path) => "upload",
        Some("prepare") if over(build_queue, BUILD_QUEUE_MAX_S).is_some() => "queue",
        Some("prepare") => "build",
        _ => "other",
    };
    json!({"id": run["id"], "source": run["source"], "outcome": outcome, "seconds": seconds, "verdict": verdict, "cause": cause, "dominant": dominant,
        "receipt": receipt.is_object(), "segments": segments, "prepare": prepare, "rolled": receipt["rolled"], "rollout_build": receipt["rollout"]["build"], "slow_rollout_s": slow_rollout,
        "errors": errors, "cache": cache, "queue": queue, "ci": super::ci::signals(outcome, &base["failure"], seconds, live, preflight_s, build_path, prepare_s), "stall": stall, "stall_s": stall_s, "signals": base["signals"], "gate_wait_s": base["gate_wait_s"]})
}

/// Runs that reused one prepare share its Cloud Build and preflight: its Rust build and preflight excess
/// count once, on the first run carrying it (50272 and 82834 both carried build e3871aad and counted its
/// 339 s rust-build twice; 26205 and 67188 both carried 3060191b and counted its 207 s preflight twice).
/// A run absorbed into another run's rollout build counts that rollout's wait once, on the first run.
fn once_per_build(rows: &[Value]) -> Vec<Value> {
    let (mut seen, mut rollouts) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    rows.iter().map(|r| {
        let mut r = r.clone();
        if let Some(rollout) = r["rollout_build"].as_str().map(str::to_owned) {
            if !rollouts.insert(rollout) {
                if let Some(q) = r["queue"].as_array_mut() { q.retain(|i| i["kind"] != "rollout_wait"); }
            }
        }
        let Some(id) = r["prepare"]["build_id"].as_str().map(str::to_owned) else { return r };
        if !seen.insert(id) {
            r["prepare"]["rust_build_s"] = Value::Null;
            if let Some(c) = r["cache"].as_array_mut() { c.retain(|i| !i["kind"].as_str().is_some_and(|k| k.starts_with("rust_"))); }
            if let Some(c) = r["ci"].as_array_mut() { c.retain(|i| i["kind"] != "preflight_on_path"); }
            if let Some(q) = r["queue"].as_array_mut() { q.retain(|i| !i["kind"].as_str().is_some_and(|k| k.starts_with("preflight_"))); }
        }
        r
    }).collect()
}

fn flagged(rows: &[Value], key: &str) -> Value {
    let items: Vec<Value> = rows.iter().flat_map(|r| r[key].as_array().into_iter().flatten().map(|i| { let mut i = i.clone(); i["run"] = r["id"].clone(); i })).collect();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for i in &items { *kinds.entry(i["kind"].as_str().unwrap_or("?").into()).or_default() += 1; }
    json!({"runs": rows.iter().filter(|r| r[key].as_array().is_some_and(|a| !a.is_empty())).count(), "by_kind": kinds, "items": items})
}

/// The window: outcomes and durations against the targets, each over-hard run with its cause, the three checks,
/// the release judges that named a window run and the queue waits release_watch measured for them.
pub(crate) fn summarize(rows: &[Value], judges: &[Value], waits: &[Value], from_ms: f64, to_ms: f64) -> Value {
    let rows = &once_per_build(rows);
    let count = |k: &str, v: &str| rows.iter().filter(|r| r[k] == v).count();
    let ok: Vec<&Value> = rows.iter().filter(|r| r["outcome"] == "succeeded").collect();
    let over_hard: Vec<Value> = rows.iter().filter(|r| r["verdict"] == "over_hard")
        .map(|r| json!({"id": r["id"], "seconds": r["seconds"], "cause": r["cause"], "dominant": r["dominant"], "segments": r["segments"]})).collect();
    let mut causes: BTreeMap<String, usize> = BTreeMap::new();
    for r in &over_hard { *causes.entry(r["cause"].as_str().unwrap_or("?").into()).or_default() += 1; }
    let segments: Map<String, Value> = SEGMENTS.iter().map(|k| (k.to_string(), spread(ok.iter().filter_map(|r| r["segments"][*k].as_f64()).collect()))).collect();
    let rust: Vec<f64> = rows.iter().filter_map(|r| r["prepare"]["rust_build_s"].as_f64()).collect();
    let hits = rust.iter().filter(|s| **s <= RUST_CACHE_HIT_MAX_S).count();
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["id"].as_str()).collect();
    let names = |s: &Value| s["signatures"].as_array().into_iter().flatten().filter_map(Value::as_str).any(|sig| ids.iter().any(|id| sig.contains(id)));
    let waits: Vec<&Value> = waits.iter().filter(|w| w["id"].as_str().is_some_and(|id| ids.contains(&id))).collect();
    json!({
        "window": {"runs": rows.len(), "from_ms": from_ms, "to_ms": to_ms},
        "targets": {"target_s": TARGET_S, "hard_s": HARD_S, "rust_cache_hit_max_s": RUST_CACHE_HIT_MAX_S, "build_queue_max_s": BUILD_QUEUE_MAX_S,
            "start_max_s": START_MAX_S, "prepare_slack_max_s": PREPARE_SLACK_MAX_S, "rollout_wait_max_s": ROLLOUT_WAIT_MAX_S, "rollout_max_s": ROLLOUT_MAX_S},
        "outcomes": {"succeeded": ok.len(), "failed": count("outcome", "failed"), "abandoned": count("outcome", "abandoned"), "success_rate": rate(ok.len(), rows.len())},
        "seconds": {"succeeded": spread(ok.iter().filter_map(|r| r["seconds"].as_f64()).collect()), "all": spread(rows.iter().filter_map(|r| r["seconds"].as_f64()).collect())},
        "verdicts": {"target": count("verdict", "target"), "over_target": count("verdict", "over_target"), "over_hard": over_hard.len(), "unknown": count("verdict", "unknown")},
        "over_hard": over_hard, "over_hard_causes": causes, "segments": segments,
        "checks": {"errors": flagged(rows, "errors"), "cache": flagged(rows, "cache"), "queue": flagged(rows, "queue"), "ci": flagged(rows, "ci")},
        "ci_cost": super::ci::cost(rows),
        "stalls": flagged(rows, "stall"),
        "cache_hit_rate": rate(hits, rust.len()),
        "slow_rollouts": rows.iter().filter(|r| r["slow_rollout_s"].is_number()).map(|r| json!({"run": r["id"], "s": r["slow_rollout_s"], "rolled": r["rolled"]})).collect::<Vec<_>>(),
        "no_receipt": rows.iter().filter(|r| r["receipt"] == false).map(|r| r["id"].clone()).collect::<Vec<_>>(),
        "judges": judges.iter().filter(|j| j["started"] == true && names(j)).cloned().collect::<Vec<_>>(),
        "measured_queue": {"all": spread(waits.iter().filter_map(|w| w["waited_s"].as_f64()).collect()), "runs": waits.len()},
    })
}
