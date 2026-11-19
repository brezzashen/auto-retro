//! Unacknowledged retro incidents. Each one holds its scope closed to new retros, so the CLI shows
//! its age, queue state and why `monitor-ack` refuses it, and the resident loop hands an orphan
//! (unacknowledged for a day and absent from the analysis queue) to one takeover session.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::path::Path;

/// Retro triggers and their routes.
pub(crate) const RETROS: [(&str, &str); 15] = [
    ("conversation_count_reached", "conversation-retro"), ("model_runtime_reached", "runtime-retro"),
    ("search_calls_reached", "search-retro"), ("release_count_reached", "release-retro"),
    ("canary_release_count_reached", "canary-retro"), ("agent_wait_reached", "wait-retro"), ("recall_calls_reached", "recall-retro"),
    ("memory_changes_reached", "memory-dream"), ("memory_dream_due", "memory-dream"), ("prompt_review_due", "prompt-review"), ("hook_defects_found", "hook-retro"), ("vector_db_growth_reached", "vector-db-retro"),
    ("crm_turn_count_reached", "crm-turn-retro"), ("risk_warning_found", "risk-retro"), ("crm_costly_turn_found", "crm-costly-turn")];
const OVERDUE_MS: f64 = 24.0 * 3_600_000.0;
const CHECK_EVERY_MS: f64 = 3_600_000.0;

/// Every unacknowledged retro incident, oldest first.
pub(crate) fn census(home: &Path, now: f64) -> Result<Vec<Value>> {
    let queue = std::fs::read(root(home).join("analysis-queue.json")).ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok()).unwrap_or(Value::Null);
    let job = |id: &str| queue["jobs"].as_array().and_then(|jobs| jobs.iter().find(|j| j["incident"] == id)).cloned();
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(root(home).join("incidents"))? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") || path.with_extension("ack").exists() { continue; }
        let Ok(incident) = serde_json::from_slice::<Value>(&std::fs::read(&path)?) else { continue };
        let Some((trigger, route)) = RETROS.iter().find(|(t, _)| incident["trigger"] == *t) else { continue };
        let id = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let detected = incident["detected_at_ms"].as_f64().unwrap_or(now);
        let job = job(&id);
        rows.push(json!({"incident": id, "trigger": trigger, "route": route, "detected_at_ms": detected,
            "age_h": ((now - detected) / 360_000.0).round() / 10.0,
            "queue": job.as_ref().map_or(json!("absent"), |j| j["state"].clone()),
            "takeovers": job.as_ref().map_or(json!(0), |j| j["takeovers"].clone()),
            "has_report": root(home).join("analysis").join(&id).join("report.json").exists(),
            "ack_refusal": crate::monitor::validate(home, &id, &path).err().map(|e| format!("{e:#}"))}));
    }
    rows.sort_by(|a, b| a["detected_at_ms"].as_f64().unwrap_or(0.0).total_cmp(&b["detected_at_ms"].as_f64().unwrap_or(0.0)));
    Ok(rows)
}

pub fn run(home: &Path) -> Result<()> {
    let now = crate::unix_now() * 1000.0;
    let state = read_state(home);
    println!("{}", serde_json::to_string_pretty(&json!({"at_ms": now, "overdue_after_h": OVERDUE_MS / 3_600_000.0,
        "incidents": census(home, now)?, "escalated": state["escalated"]}))?);
    Ok(())
}

fn read_state(home: &Path) -> Value {
    std::fs::read(root(home).join("retro-overdue.json")).ok()
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({"escalated": {}}))
}

/// Hourly: enqueue one takeover session per orphan; never twice for one incident.
pub(crate) fn escalate(home: &Path, now: f64) -> Result<()> {
    let mut state = read_state(home);
    if now - state["checked_ms"].as_f64().unwrap_or(0.0) < CHECK_EVERY_MS { return Ok(()); }
    let mut escalated = state["escalated"].as_object().cloned().unwrap_or_default();
    for row in orphans(&census(home, now)?, &escalated, now) {
        let id = row["incident"].as_str().unwrap_or_default();
        let session = crate::dispatch::session_opened_for(home, id).ok().flatten();
        crate::dispatch::start_session(home, id, &format!("看门狗 {} 补确认", row["route"].as_str().unwrap_or("retro")), &prompt(&row, session.as_ref()))?;
        escalated.insert(id.to_string(), json!({"at_ms": now, "ack_refusal": row["ack_refusal"]}));
        if !cfg!(test) { crate::dispatch::notify("看门狗：复盘超期未确认", &format!("{} {} 已 {} 小时未确认，已排队接管会话", row["route"].as_str().unwrap_or(""), id, row["age_h"])); }
    }
    state["checked_ms"] = json!(now);
    state["escalated"] = Value::Object(escalated);
    write_json(&root(home).join("retro-overdue.json"), &state)
}

fn orphans<'a>(rows: &'a [Value], escalated: &Map<String, Value>, now: f64) -> Vec<&'a Value> {
    rows.iter().filter(|r| r["queue"] == "absent" && now - r["detected_at_ms"].as_f64().unwrap_or(now) > OVERDUE_MS
        && !escalated.contains_key(r["incident"].as_str().unwrap_or_default())).collect()
}

fn prompt(row: &Value, session: Option<&Value>) -> String {
    let id = row["incident"].as_str().unwrap_or_default();
    let route = row["route"].as_str().unwrap_or_default();
    let previous = session.map_or("No earlier session for it was found.".to_string(),
        |s| format!("Its earlier session is {}; read that transcript's tail first.", s["path"].as_str().unwrap_or("unknown")));
    format!("cpu-watchdog incident {id} ({trigger}) has stayed unacknowledged for {age} h and keeps route {route} closed: no new {route} opens until it is acknowledged. \
        monitor-ack currently refuses it with: {refusal}. {previous} Read ~/.local/state/cpu-watchdog/incidents/{id}.json and analysis/{id}/, complete only what the refusal names under the route's requirements \
        (rerun the receipts it asks for; republish the report when its content changes), then run cpu-watchdog monitor-ack {id}. {header}",
        trigger = row["trigger"].as_str().unwrap_or_default(), age = row["age_h"],
        refusal = row["ack_refusal"].as_str().unwrap_or("nothing (run monitor-ack)"), header = crate::dispatch::route_header(route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_day_old_unqueued_unescalated_retros_are_orphans() {
        let day = OVERDUE_MS;
        let row = |id: &str, queue: &str, at: f64| json!({"incident": id, "queue": queue, "detected_at_ms": at, "route": "search-retro", "trigger": "search_calls_reached", "age_h": 30, "ack_refusal": "no recall receipt"});
        let rows = vec![row("1", "absent", 0.0), row("2", "queued", 0.0), row("3", "absent", 1.5 * day), row("4", "absent", 0.0)];
        let mut escalated = Map::new();
        escalated.insert("4".into(), json!({}));
        let picked: Vec<&str> = orphans(&rows, &escalated, 2.0 * day).iter().map(|r| r["incident"].as_str().unwrap()).collect();
        assert_eq!(picked, ["1"]);
        let text = prompt(&rows[0], Some(&json!({"path": "/t.jsonl"})));
        assert!(text.starts_with("cpu-watchdog incident 1 (search_calls_reached)") && text.contains("no recall receipt") && text.contains("/t.jsonl"));
        assert!(text.contains("references/search-retro.md"));
    }
}
