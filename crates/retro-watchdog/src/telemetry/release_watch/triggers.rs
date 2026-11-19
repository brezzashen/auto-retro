//! Release-queue triggers: the scripted signals the normal path cannot settle by itself. Each one
//! names its row in `.claude/skills/release-queue/references/escalation.md`; a judging agent
//! decides it. Pure functions over the queue snapshot, the board and Docker's recovery times.
use super::snapshot::Git;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(crate) const WAIT_Q1_S: f64 = 900.0;
pub(crate) const WAIT_Q2_S: f64 = 1800.0;
pub(crate) const STALL_S: f64 = 1200.0;
/// An abandoned run is reported while its last progress is this recent. Shorter than the 6 h repeat
/// window, so one dead run opens at most one judge however long its board record stays `running`.
pub(crate) const ABANDONED_FRESH_S: f64 = 3_600.0;
const FAIL_WINDOW_MS: f64 = 7_200_000.0;
const FAIL_REPEATS: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Trigger { pub code: &'static str, pub signature: String, pub summary: String, pub affected: Vec<String> }

fn releases(snapshot: &Value) -> Vec<(&str, &Value)> {
    snapshot["environments"].as_object().into_iter().flatten()
        .flat_map(|(env, e)| e["releases"].as_array().into_iter().flatten().map(move |r| (env.as_str(), r))).collect()
}

fn id(r: &Value) -> String { r["id"].as_str().unwrap_or_default().to_string() }

fn frozen(r: &Value) -> bool { r["freeze"]["frozen"] == true }

/// The one release a freeze admits: no judge ever acts on it while it lives.
fn allowed(r: &Value) -> bool { frozen(r) && r["freeze"]["allowed"] == true }

/// T10: a live run in a frozen environment whose source the freeze does not allow.
fn into_frozen(snapshot: &Value, out: &mut Vec<Trigger>) {
    for (env, r) in releases(snapshot).into_iter().filter(|(_, r)| frozen(r) && !allowed(r) && r["state"] == "running") {
        out.push(Trigger { code: "T10", signature: format!("T10:{}", id(r)),
            summary: format!("{env} is frozen ({}); {} ships {} instead of {}", r["freeze"]["reason"].as_str().unwrap_or(""), id(r),
                r["source"].as_str().map_or("an unresolved source".into(), |s| s.chars().take(10).collect::<String>()),
                r["freeze"]["only"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")),
            affected: vec![id(r)] });
    }
}

/// T9: two sessions' runs in one environment that ship at least one common path (escalation.md T9:
/// "the same path"). Divergent sources with no common path merge in one ship-web round, so they
/// are no collision: release retro 811587194803931207163's only T9 judge (82334183045490983207)
/// found 16 and 5 changed files, an empty intersection and both runs riding one round.
/// A frozen environment's collisions are T10's: only its allowed release may proceed.
/// Two beta-web runs whose sources are both on `origin/mastra` are no collision either: the gateway
/// ship-web builds only the pushed tip and refuses any copy that differs from it
/// (`Mars_CRM/rust/crates/mars-ops/src/release/ship/gateway.rs` `ensure_pushed`), so neither can roll
/// the other back. Judges 82096973662365895020 and 81661975683923041583 each found that shape settled.
/// A beta-web source that is a pre-rebase local copy counts as pushed when its named inputs already
/// have the same bytes on `origin/mastra`, and shared paths that no image in a mapped environment
/// deploys collide in nothing (judges 86365156581115136928 and 81992542804222953903). Rollouts
/// count only the images both runs deploy (`deploys.rs`: beta plan services, beta-web api, beta-crm
/// crm), so an agent-only beta run never collides on web paths (judge 87201392560521810218).
fn conflicts(snapshot: &Value, git: &dyn Git, out: &mut Vec<Trigger>) {
    let live = releases(snapshot);
    // A dead run ships nothing, so it collides with nobody.
    let dead: Vec<String> = live.iter().filter(|(_, r)| r["state"] == "abandoned").map(|(_, r)| id(r)).collect();
    let inputs = |r: &Value| r["inputs"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
    let pushed_web = |r: &Value| r["target"] == "beta-web"
        && r["source"].as_str().is_some_and(|s| git.pushed(s) || git.carried(s, &inputs(r)));
    for (env, r) in live.iter().copied().filter(|(_, r)| !frozen(r) && r["state"] != "abandoned") {
        for o in r["overlaps"].as_array().into_iter().flatten() {
            let other = o["with"].as_str().unwrap_or_default();
            if o["session"] == r["creator"]["session"] || id(r).as_str() > other || dead.iter().any(|d| d == other) { continue; }
            if o["shared"].as_u64() == Some(0) { continue; }
            if o["mapped"] == true && o["rollouts"].as_array().is_some_and(Vec::is_empty) { continue; }
            let partner = live.iter().find(|(_, p)| p["id"] == other).map(|(_, p)| *p);
            if pushed_web(r) && partner.is_some_and(pushed_web) { continue; }
            out.push(Trigger { code: "T9", signature: format!("T9:{}:{other}", id(r)),
                summary: format!("{env}: {} and {other} collide ({}, {} shared path(s))", id(r), o["relation"].as_str().unwrap_or("?"), o["shared"]),
                affected: vec![id(r), other.to_string()] });
        }
    }
}

/// T4 (abandoned or silent) and T11 (waiting past the queue's limit).
fn stuck(snapshot: &Value, out: &mut Vec<Trigger>) {
    for (env, r) in releases(snapshot) {
        if r["state"] == "abandoned" {
            // Abandoned is final: no silence or wait trigger, and the one T4 only while it is fresh.
            if r["idle_s"].as_f64().is_some_and(|s| s < ABANDONED_FRESH_S) {
                out.push(Trigger { code: "T4", signature: format!("T4:abandoned:{}", id(r)),
                    summary: format!("{env}: {} is running on the board but its deliver process is gone", id(r)), affected: vec![id(r)] });
            }
            continue;
        } else if allowed(r) {
            continue;
        } else if r["phase"] == "running" && r["idle_s"].as_f64().is_some_and(|s| s >= STALL_S) {
            out.push(Trigger { code: "T4", signature: format!("T4:silent:{}", id(r)),
                summary: format!("{env}: {} printed no progress for {} min in {}", id(r), (r["idle_s"].as_f64().unwrap_or(0.0) / 60.0).round(), r["step"].as_str().unwrap_or("?")), affected: vec![id(r)] });
        }
        let limit = if r["queue"] == "Q1" { WAIT_Q1_S } else { WAIT_Q2_S };
        if r["phase"] == "waiting" && r["waiting_s"].as_f64().is_some_and(|s| s >= limit) {
            out.push(Trigger { code: "T11", signature: format!("T11:{}", id(r)),
                summary: format!("{env}: {} {} has waited {} min ({})", r["queue"].as_str().unwrap_or("?"), id(r), (r["waiting_s"].as_f64().unwrap_or(0.0) / 60.0).round(), r["progress"].as_str().unwrap_or("")),
                affected: vec![id(r)] });
        }
    }
}

/// T8: a Q1 run waits on a resource held by a Q2 run of the same environment that started after it,
/// and that Q2 took the resource while the Q1 already waited, i.e. the queue admitted Q2 ahead of a
/// Q1 already queued (escalation.md T8). The Q1's `blocker` (`blocker.rs`: the holder pid its
/// waiting line names, resolved to a board run) must be that Q2; judge 81710544233103865991's Q1
/// waited on the deploy lease of an older Q2, not the flagged one. The holder process must also have
/// started after the Q1's wait began (`at_ms - waiting_s`; the first snapshot that saw it waiting, so
/// never earlier than the real start): a Q1 that reaches a resource a Q2 already holds waits for that
/// atomic step by design (release retro 84705495676337590512; judge 87261054405244924163's Q1 was
/// still building when the younger Q2 took the free lease). An unresolved blocker or unknown holder
/// start raises no T8: the waits without a holder pid are the host build gate, a ship-web round and
/// Cloud Build status, none a Q2 admitted ahead, and a holder gone or off the board is no Q2 either;
/// a Q1 kept waiting still reaches T11.
fn inversions(snapshot: &Value, out: &mut Vec<Trigger>) {
    let age = |r: &Value| r["age_s"].as_f64().unwrap_or(0.0);
    let at_ms = snapshot["at_ms"].as_f64().unwrap_or(0.0);
    // Holder start has 1 s resolution and waiting_s is rounded: demand a clear second after.
    let took_while_waiting = |q1: &Value| q1["blocker"]["started_ms"].as_f64()
        .is_some_and(|started| started > at_ms - q1["waiting_s"].as_f64().unwrap_or(0.0) * 1000.0 + 1000.0);
    for (env, e) in snapshot["environments"].as_object().into_iter().flatten() {
        let list = e["releases"].as_array().map_or(&[][..], Vec::as_slice);
        if list.iter().any(frozen) { continue; }
        let waiting_q1: Vec<&Value> = list.iter().filter(|r| r["queue"] == "Q1" && r["phase"] == "waiting").collect();
        for q2 in list.iter().filter(|r| r["queue"] == "Q2" && r["state"] == "running") {
            let Some(q1) = waiting_q1.iter().find(|q1| q1["blocker"]["run"] == q2["id"] && age(q1) > age(q2) && took_while_waiting(q1)) else { continue };
            out.push(Trigger { code: "T8", signature: format!("T8:{}:{}", id(q2), id(q1)),
                summary: format!("{env}: Q1 {} waits on pid {} of Q2 {}, which started after it and took it while the Q1 waited", id(q1), q1["blocker"]["pid"], id(q2)),
                affected: vec![id(q2), id(q1)] });
        }
    }
}

/// T5: the same environment and queue item (`task`, else `purpose`) failed repeatedly within two
/// hours since its last successful run. A later success supersedes earlier failures of the same
/// item, of the same source SHA, or of a source SHA its own source carries, in that environment (a
/// retry often rewords its purpose or rides in another item's commit), and the signature names the
/// streak's first failure, so it stays put while the window slides. A live retry supersedes too:
/// a run still in flight in that environment, started after the failure, whose source is or carries
/// the failed source (judge 816077057436881794164: the item's rerun on the fix commit was already
/// preparing when T5 opened). If that retry fails as well, the streak returns with it.
fn failures(board: &[Value], git: &dyn Git, now_ms: f64, out: &mut Vec<Trigger>) {
    let at = |e: &Value| e["finishedAt"].as_str().and_then(crate::cause_tree::attribute::parse_time).unwrap_or(0.0);
    let target = |e: &Value| e["target"].as_str().unwrap_or("?").to_string();
    let item = |e: &Value| (target(e),
        e["task"].as_str().filter(|t| !t.is_empty()).or(e["purpose"].as_str()).unwrap_or("").to_string());
    let source = |e: &Value| e["source"].as_str().filter(|s| !s.is_empty()).map(|s| (target(e), s.to_string()));
    let mut succeeded: BTreeMap<(String, String), f64> = BTreeMap::new();
    let mut source_succeeded: BTreeMap<(String, String), f64> = BTreeMap::new();
    let latest = |map: &mut BTreeMap<(String, String), f64>, key, t: f64| { let last = map.entry(key).or_insert(0.0); *last = last.max(t); };
    for e in board.iter().filter(|e| matches!(e["state"].as_str(), Some("succeeded" | "finished"))) {
        latest(&mut succeeded, item(e), at(e));
        if let Some(key) = source(e) { latest(&mut source_succeeded, key, at(e)); }
    }
    // Only failures inside the window ask git, so one board pass stays a handful of merge-base calls.
    let carried = |e: &Value| now_ms - at(e) <= FAIL_WINDOW_MS && source(e).is_some_and(|(env, failed)| source_succeeded.iter()
        .any(|((target, ok), t)| *target == env && *t >= at(e) && git.relation(ok, &failed).0 == "contains"));
    let retrying = in_flight(board);
    let retried = |e: &Value| now_ms - at(e) <= FAIL_WINDOW_MS && source(e).is_some_and(|(env, failed)| retrying.iter()
        .any(|(target, live, started)| *target == env && *started >= at(e) && (*live == failed || git.relation(live, &failed).0 == "contains")));
    let superseded = |e: &Value| at(e) <= succeeded.get(&item(e)).copied().unwrap_or(0.0)
        || source(e).is_some_and(|key| at(e) <= source_succeeded.get(&key).copied().unwrap_or(0.0))
        || carried(e) || retried(e);
    // Failures after the item's last success, oldest first.
    let mut streaks: BTreeMap<(String, String), Vec<(f64, String)>> = BTreeMap::new();
    // A candidate refused for contamination is T6's, not a failed preflight.
    for e in board.iter().filter(|e| e["state"] == "failed" && !refused(e) && !superseded(e)) {
        streaks.entry(item(e)).or_default().push((at(e), id(e)));
    }
    for ((target, label), mut streak) in streaks {
        streak.sort_by(|a, b| a.0.total_cmp(&b.0));
        let recent: Vec<String> = streak.iter().filter(|(t, _)| now_ms - t <= FAIL_WINDOW_MS).map(|(_, i)| i.clone()).collect();
        if recent.len() < FAIL_REPEATS { continue; }
        out.push(Trigger { code: "T5", signature: format!("T5:{target}:{}", streak[0].1),
            summary: format!("{target} \"{}\" failed {} times in 2 h", label.chars().take(80).collect::<String>(), recent.len()), affected: recent });
    }
}

/// Runs still in flight (live creator process): environment, source and start time.
fn in_flight(board: &[Value]) -> Vec<(String, String, f64)> {
    board.iter().filter(|e| super::snapshot::live_state(e) == "running").filter_map(|e| Some((
        e["target"].as_str().unwrap_or("?").to_string(),
        e["source"].as_str().filter(|s| !s.is_empty())?.to_string(),
        e["startedAt"].as_str().and_then(crate::cause_tree::attribute::parse_time)?,
    ))).collect()
}

/// `deliver canary` refused this run before prepare: `deliver candidate-check` found evidence.
fn refused(e: &Value) -> bool { e["candidateCheck"]["contaminated"] == true }

fn short(sha: &str) -> String { sha.chars().take(10).collect() }

/// T6: a contaminated canary candidate. Either `deliver canary` refused it (judged while the refusal
/// is under ABANDONED_FRESH_S old), or a running canary run's source carries a commit whose owner
/// recorded a failed acceptance (`deliver accept`) that no later passed verdict on a commit it also
/// carries covers. Cherry-picked twins are matched only by `deliver candidate-check` itself.
fn contaminated(snapshot: &Value, board: &[Value], git: &dyn Git, now_ms: f64, out: &mut Vec<Trigger>) {
    let finished = |e: &Value| e["finishedAt"].as_str().and_then(crate::cause_tree::attribute::parse_time).unwrap_or(0.0);
    for e in board.iter().filter(|e| refused(e) && now_ms - finished(e) < ABANDONED_FRESH_S * 1000.0) {
        let t6 = e["candidateCheck"]["t6"].as_str().or(e["failure"][0].as_str()).unwrap_or("T6 contaminated canary candidate");
        out.push(Trigger { code: "T6", signature: format!("T6:{}", id(e)), summary: format!("{}: {}", id(e), t6.chars().take(300).collect::<String>()), affected: vec![id(e)] });
    }
    let verdict = |e: &Value, v: &str| e["acceptance"]["verdict"] == v && e["source"].as_str().is_some_and(|s| s.len() == 40);
    let at = |e: &Value| e["acceptance"]["at"].as_str().unwrap_or_default().to_string();
    let carries = |a: &str, b: &str| a == b || git.relation(a, b).0 == "contains";
    for (env, r) in releases(snapshot).into_iter().filter(|(env, r)| *env == "canary" && r["state"] == "running" && !allowed(r)) {
        let Some(candidate) = r["source"].as_str() else { continue };
        let unfixed: Vec<&Value> = board.iter().filter(|e| verdict(e, "failed")).filter(|failed| {
            let bad = failed["source"].as_str().unwrap_or_default();
            carries(candidate, bad) && !board.iter().filter(|p| verdict(p, "passed") && at(p) > at(failed))
                .filter_map(|p| p["source"].as_str()).any(|fix| carries(candidate, fix) && carries(fix, bad))
        }).collect();
        if unfixed.is_empty() { continue; }
        // One signature per run, shared with the refusal branch above, so one run opens one judge.
        let carried = unfixed.iter().map(|failed| format!("{} (release {}, session {})", short(failed["source"].as_str().unwrap_or_default()),
            id(failed), failed["creator"]["session"].as_str().map(short).unwrap_or_else(|| "?".into()))).collect::<Vec<_>>().join(", ");
        let mut affected = vec![id(r)];
        affected.extend(unfixed.iter().map(|failed| id(failed)));
        out.push(Trigger { code: "T6", signature: format!("T6:{}", id(r)),
            summary: format!("{env}: {} ships {} carrying {carried} whose acceptance failed", id(r), short(candidate)), affected });
    }
}

/// T1: the watchdog restarted Docker while beta runs were active.
fn docker(snapshot: &Value, docker_actions_ms: &[f64], seen_ms: f64, out: &mut Vec<Trigger>) {
    let Some(at) = docker_actions_ms.iter().copied().filter(|a| *a > seen_ms).reduce(f64::max) else { return };
    let beta: Vec<String> = snapshot["environments"]["beta"]["releases"].as_array().into_iter().flatten().map(id).collect();
    if !beta.is_empty() {
        out.push(Trigger { code: "T1", signature: format!("T1:{at:.0}"),
            summary: format!("Docker was restarted by the watchdog while {} beta run(s) were active", beta.len()), affected: beta });
    }
}

pub(crate) fn detect(snapshot: &Value, board: &[Value], git: &dyn Git, docker_actions_ms: &[f64], docker_seen_ms: f64, now_ms: f64) -> Vec<Trigger> {
    let mut out = Vec::new();
    docker(snapshot, docker_actions_ms, docker_seen_ms, &mut out);
    into_frozen(snapshot, &mut out);
    contaminated(snapshot, board, git, now_ms, &mut out);
    conflicts(snapshot, git, &mut out);
    inversions(snapshot, &mut out);
    stuck(snapshot, &mut out);
    failures(board, git, now_ms, &mut out);
    out
}

/// The affected runs as the judging agent needs them: purpose, queue, creator and Orca tab.
pub(crate) fn affected(snapshot: &Value, board: &[Value], ids: &[String]) -> Vec<Value> {
    let live = releases(snapshot);
    ids.iter().map(|wanted| live.iter().find(|(_, r)| r["id"] == wanted.as_str()).map(|(_, r)| (*r).clone())
        .or_else(|| board.iter().find(|e| e["id"] == wanted.as_str()).map(|e| json!({"id": e["id"], "target": e["target"],
            "queue": e["queue"], "purpose": e["purpose"], "state": e["state"], "failure": e["failure"], "creator": e["creator"], "log": e["log"]})))
        .unwrap_or_else(|| json!({"id": wanted}))).collect()
}
