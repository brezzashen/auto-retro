//! Retro accounting. Each acknowledged retro adds a `period: retro` row with its session cost to
//! the diagnosis ledger, and `cpu-watchdog retro-scorecard [--backfill]` rolls up the flywheel:
//! analysis-queue health, overdue retros, per-route ack latency, findings and cost, and each
//! probe's verdict streak across retros (the input of probe graduation and rework).
use crate::cause_tree::ledger;
use crate::monitor::root;
use crate::retro_overdue::RETROS;
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::{collections::{BTreeMap, HashSet}, path::Path};

/// Consecutive same verdicts that make a probe streak worth acting on.
pub(crate) const STREAK: usize = 3;

fn read(path: &Path) -> Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

fn route_of(trigger: &str) -> Option<&'static str> {
    RETROS.iter().find(|(t, _)| *t == trigger).map(|(_, r)| *r)
}

/// Ledger row for an acknowledged retro; other triggers are ignored.
pub(crate) fn record_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event = read(incident);
    let Some(route) = event["trigger"].as_str().and_then(route_of) else { return Ok(()) };
    let report = read(&root(home).join("analysis").join(id).join("report.json"));
    let since = std::fs::metadata(incident)?.modified()?;
    ledger::append(home, &json!({"incident": id, "trigger": event["trigger"], "route": route, "period": "retro",
        "closed_by": "model", "findings": report["findings"].as_array().map_or(0, |f| f.len()),
        "session": ledger::session_cost(home, id, since), "at_ms": crate::unix_now() * 1000.0}))?;
    crate::retro_consolidate::apply(home, crate::unix_now() * 1000.0).map(|_| ())
}

/// Acknowledged retros the ledger does not hold yet; returns how many were added.
fn backfill(home: &Path) -> Result<usize> {
    let known: HashSet<String> = ledger::rows(home).iter().filter_map(|r| r["incident"].as_str().map(String::from)).collect();
    let mut added = 0;
    for (id, path, _) in retros(home)? {
        if known.contains(&id) || !path.with_extension("ack").exists() { continue; }
        let report = read(&root(home).join("analysis").join(&id).join("report.json"));
        let event = read(&path);
        let at = read(&path.with_extension("ack"))["acknowledged_at_ms"].as_f64().unwrap_or(0.0);
        ledger::append(home, &json!({"incident": id, "trigger": event["trigger"], "route": event["trigger"].as_str().and_then(route_of),
            "period": "retro", "closed_by": "model", "findings": report["findings"].as_array().map_or(0, |f| f.len()),
            "session": ledger::session_cost(home, &id, std::fs::metadata(&path)?.modified()?), "at_ms": at, "backfilled": true}))?;
        added += 1;
    }
    Ok(added)
}

/// Every retro incident as (id, path, detected_at_ms), oldest first.
fn retros(home: &Path) -> Result<Vec<(String, std::path::PathBuf, f64)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root(home).join("incidents"))?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") { continue; }
        let event = read(&path);
        if event["trigger"].as_str().and_then(route_of).is_none() { continue; }
        let id = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        found.push((id, path, event["detected_at_ms"].as_f64().unwrap_or(0.0)));
    }
    found.sort_by(|a, b| a.2.total_cmp(&b.2));
    Ok(found)
}

fn median(mut values: Vec<f64>) -> Value {
    if values.is_empty() { return Value::Null; }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    let m = if values.len() % 2 == 1 { values[mid] } else { (values[mid - 1] + values[mid]) / 2.0 };
    json!((m * 10.0).round() / 10.0)
}

fn routes(home: &Path, list: &[(String, std::path::PathBuf, f64)]) -> Value {
    let costs: BTreeMap<String, Value> = ledger::rows(home).into_iter().filter(|r| r["period"] == "retro")
        .filter_map(|r| Some((r["incident"].as_str()?.to_string(), r["session"].clone()))).collect();
    let mut out = Map::new();
    for (_, route) in RETROS {
        let mine: Vec<_> = list.iter().filter(|(_, p, _)| read(p)["trigger"].as_str().and_then(route_of) == Some(route)).collect();
        let acked: Vec<_> = mine.iter().filter(|(_, p, _)| p.with_extension("ack").exists()).collect();
        let latency = acked.iter().filter_map(|(_, p, at)| read(&p.with_extension("ack"))["acknowledged_at_ms"].as_f64().map(|a| (a - at) / 3_600_000.0)).collect();
        let findings = acked.iter().filter_map(|(id, _, _)| read(&root(home).join("analysis").join(id).join("report.json"))["findings"].as_array().map(|f| f.len() as f64)).collect();
        let sessions: Vec<&Value> = acked.iter().filter_map(|(id, _, _)| costs.get(id)).filter(|s| s.is_object()).collect();
        let field = |f: &str| median(sessions.iter().filter_map(|s| s[f].as_f64()).collect());
        out.insert(route.to_string(), json!({"incidents": mine.len(), "acked": acked.len(), "open": mine.len() - acked.len(),
            "ack_latency_h_median": median(latency), "findings_median": median(findings),
            "cost": {"measured": sessions.len(), "active_minutes_median": field("active_minutes"),
                "output_tokens_median": field("output_tokens"), "turns_median": field("turns")}}));
    }
    Value::Object(out)
}

/// Verdicts in retro order as runs, `ineffective×35, effective, regressed×3`: the one-per-line
/// array made the default output 819 lines (2026-11-09), so agents cut it with `| head`/`| tail` and
/// re-ran it with jq slices (conversation retro 818271371277672689612: 544970ac:82-101).
fn run_length<'a>(verdicts: impl Iterator<Item = &'a str>) -> String {
    let mut runs: Vec<(&str, usize)> = Vec::new();
    for v in verdicts {
        match runs.last_mut() {
            Some((last, n)) if *last == v => *n += 1,
            _ => runs.push((v, 1)),
        }
    }
    runs.iter().map(|(v, n)| if *n == 1 { v.to_string() } else { format!("{v}×{n}") }).collect::<Vec<_>>().join(", ")
}

/// Each probe's verdicts in retro order, its current streak, and the action the streak calls for.
pub(crate) fn probe_streaks(home: &Path) -> Result<Vec<Value>> {
    let mut history: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut notes: BTreeMap<String, Value> = BTreeMap::new();
    for (id, _, _) in retros(home)? {
        let report = read(&root(home).join("analysis").join(&id).join("report.json"));
        for f in report["followups"].as_array().into_iter().flatten() {
            if let (Some(p), Some(v)) = (f["probe"].as_str(), f["verdict"].as_str()) {
                history.entry(p.to_string()).or_default().push((id.clone(), v.to_string()));
                notes.insert(p.to_string(), json!({"incident": id, "next": f["next"], "analysis": f["analysis"]}));
            }
        }
    }
    Ok(history.into_iter().map(|(probe, verdicts)| {
        let note = notes.remove(&probe).unwrap_or(Value::Null);
        let last = verdicts.last().map(|v| v.1.clone()).unwrap_or_default();
        let streak = verdicts.iter().rev().take_while(|v| v.1 == last).count();
        let action = match (last.as_str(), streak >= STREAK) {
            ("effective", true) => "graduate",
            ("insufficient_data", true) => "redefine",
            ("ineffective" | "regressed", true) => "escalate",
            _ => "keep",
        };
        json!({"probe": probe, "judged": verdicts.len(), "last": last, "streak": streak, "action": action,
            "history": run_length(verdicts.iter().map(|v| v.1.as_str())), "latest": note})
    }).collect())
}

fn queue(home: &Path, now: f64) -> Value {
    let state = read(&root(home).join("analysis-queue.json"));
    let jobs = state["jobs"].as_array().cloned().unwrap_or_default();
    let hours = |at: &Value| at.as_f64().map(|a| ((now - a) / 360_000.0).round() / 10.0);
    let active: Vec<Value> = jobs.iter().filter(|j| matches!(j["state"].as_str(), Some("launching" | "launched" | "uncertain")))
        .map(|j| json!({"incident": j["incident"], "state": j["state"], "age_h": hours(&j["started_at_ms"]), "takeovers": j["takeovers"]})).collect();
    let waits: Vec<f64> = jobs.iter().filter(|j| j["state"] == "queued").filter_map(|j| hours(&j["queued_at_ms"])).collect();
    json!({"active": active, "max_active": state["max_active"],
        "queued": waits.len(), "oldest_wait_h": waits.iter().cloned().fold(None, |m: Option<f64>, w| Some(m.map_or(w, |m| m.max(w)))),
        "abandoned": jobs.iter().filter(|j| j["state"] == "abandoned").map(|j| j["incident"].clone()).collect::<Vec<_>>(),
        "error": state["error"]})
}

/// Streaks the default output prints whole: only the probes a retro must act on. Graduate and keep
/// rows (66 of 79 on 2026-11-08) pushed escalate's details past `| head -80`, so 61 sessions re-ran
/// the scorecard with jq and opened the previous report.json for its `next` (conversation retro
/// 81936231737310136716: 144 calls, 11 sessions stuck, e.g. 17d45799:111-121, 2ba88acd:110-120).
fn needs_action(streak: &Value) -> bool {
    matches!(streak["action"].as_str(), Some("escalate" | "redefine"))
}

pub fn run(home: &Path, args: Vec<String>) -> Result<()> {
    if args.iter().any(|a| a == "--backfill") { eprintln!("backfilled {} retro(s)", backfill(home)?); }
    let now = crate::unix_now() * 1000.0;
    let overdue: Vec<Value> = crate::retro_overdue::census(home, now)?.into_iter()
        .map(|r| json!({"incident": r["incident"], "route": r["route"], "age_h": r["age_h"], "queue": r["queue"], "ack_refusal": r["ack_refusal"]})).collect();
    // `--incident <id>`: is this retro still open? Replaces `retro-scorecard | jq '.overdue'` after monitor-ack.
    if let Some(id) = args.iter().position(|a| a == "--incident").and_then(|i| args.get(i + 1)) {
        let acked = root(home).join("incidents").join(format!("{id}.ack")).exists();
        let row = overdue.iter().find(|r| r["incident"] == id.as_str()).cloned().unwrap_or(Value::Null);
        println!("{}", serde_json::to_string_pretty(&json!({"incident": id, "acked": acked, "overdue": row}))?);
        return Ok(());
    }
    let list = retros(home)?;
    let streaks = probe_streaks(home)?;
    let mut actions: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for s in &streaks { actions.entry(s["action"].as_str().unwrap_or("keep").to_string()).or_default().push(s["probe"].clone()); }
    let last: Vec<&str> = streaks.iter().filter_map(|s| s["last"].as_str()).collect();
    let share = |v: &str| if last.is_empty() { Value::Null } else { json!((last.iter().filter(|l| **l == v).count() as f64 / last.len() as f64 * 100.0).round()) };
    let full = args.iter().any(|a| a == "--full");
    let shown: Vec<&Value> = streaks.iter().filter(|s| full || needs_action(s)).collect();
    // Default: graduate and keep collapse to counts so the actionable lists fit in the first screen.
    let actions: Map<String, Value> = actions.into_iter().map(|(k, v)| match (full, k.as_str()) {
        (false, "graduate" | "keep") => (format!("{k}_count"), json!(v.len())),
        _ => (k, json!(v)),
    }).collect();
    println!("{}", serde_json::to_string_pretty(&json!({"at_ms": now, "overdue": overdue,
        "probes": {"count": streaks.len(), "last_effective_pct": share("effective"), "last_insufficient_pct": share("insufficient_data"),
            "last_ineffective_pct": share("ineffective"), "actions": actions,
            "streaks_shown": if full { "all (--full)" } else { "escalate and redefine only, each with the latest retro's next; --full prints every probe" },
            "streaks": shown},
        "queue": queue(home, now), "routes": routes(home, &list)}))?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_handles_odd_even_and_empty() {
        assert_eq!(median(vec![3.0, 1.0, 2.0]), json!(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), json!(2.5));
        assert_eq!(median(vec![]), Value::Null);
    }

    #[test]
    fn streaks_pick_graduate_redefine_escalate() {
        let home = crate::telemetry::test_home("retro-scorecard");
        let dir = root(&home);
        std::fs::create_dir_all(dir.join("incidents")).unwrap();
        for (n, verdicts) in [["effective", "insufficient_data", "ineffective"], ["effective", "insufficient_data", "ineffective"], ["effective", "insufficient_data", "effective"]].iter().enumerate() {
            let id = format!("{n}");
            crate::monitor::write_json(&dir.join(format!("incidents/{id}.json")), &json!({"trigger": "conversation_count_reached", "detected_at_ms": n as f64})).unwrap();
            std::fs::create_dir_all(dir.join("analysis").join(&id)).unwrap();
            let followups: Vec<Value> = verdicts.iter().enumerate().map(|(i, v)| json!({"probe": format!("R{i}"), "verdict": v})).collect();
            crate::monitor::write_json(&dir.join(format!("analysis/{id}/report.json")), &json!({"followups": followups})).unwrap();
        }
        let streaks = probe_streaks(&home).unwrap();
        let action = |p: &str| streaks.iter().find(|s| s["probe"] == p).unwrap()["action"].clone();
        assert_eq!(action("R0"), "graduate");
        assert_eq!(action("R1"), "redefine");
        assert_eq!(action("R2"), "keep");
        assert!(needs_action(&json!({"action": "redefine"})) && !needs_action(&json!({"action": "graduate"})));
        let r1 = streaks.iter().find(|s| s["probe"] == "R1").unwrap();
        assert_eq!(r1["latest"]["incident"], "2");
        assert_eq!(r1["history"], "insufficient_data×3");
        assert_eq!(run_length(["ineffective", "ineffective", "effective", "regressed"].into_iter()), "ineffective×2, effective, regressed");
        std::fs::remove_dir_all(home).unwrap();
    }
}
