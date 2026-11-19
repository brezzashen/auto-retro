//! Script closure of a forced recovery whose cause the tree already answers (founder, 2026-10-25
//! and 2026-10-26): Docker VM-OOM / Redis AOF recoveries, plus effective PerfPowerServices daemon
//! restarts (node N7.1, when the respawned instance settles below its 0.5-core floor) and, since
//! founder decision A on 2026-11-04, ineffective ones (node N7.1.3: daemon_heal holds it 24 h).
//! Since 2026-11-15 (report perf-memory-1794682770151 item 2) an effective fseventsd restart closes
//! on N8.4.1.1 the same way; an ineffective one names no node and opens a session.
//! A node closes by script only when cause-tree.json marks it `script_close: true` and gives its
//! `answer`, and the recovery succeeded; a node with `max_script_closes_24h` opens a model session
//! once its 24 h recurrence count exceeds that cap.
use crate::cause_tree::ledger;
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

const DAY_MS: f64 = 86_400_000.0;

/// The cause-tree node a recovery record names: the restart attribution for an engine or
/// Kubernetes recovery, the AOF node for a Redis replica repair, or the EFFECTIVE node of a
/// daemon restart whose successor settled below its CPU floor.
fn named_node(kind: &str, event: &Value) -> Option<String> {
    match kind {
        "docker" if event["outcome"] == "recovered" => event["attribution"]["node"].as_str().map(str::to_string),
        "docker-redis" if event["outcome"] == "repaired" => Some("N4.7".to_string()),
        // Founder decision A (2026-11-04): an ineffective PerfPowerServices restart is recorded,
        // held 24 h by daemon_heal, and closed by script instead of opening a session.
        "daemon" => recovered_node(event).or_else(|| perf_power_ineffective(event).then(|| INEFFECTIVE_NODE.to_string())),
        _ => None,
    }
}

const INEFFECTIVE_NODE: &str = "N7.1.3";
/// Daemon and the node that answers its effective restart.
const EFFECTIVE: &[(&str, &str)] = &[("PerfPowerServices", "N7.1"), ("fseventsd", "N8.4.1.1")];

fn perf_power_ineffective(event: &Value) -> bool {
    event["name"] == "PerfPowerServices" && event["new_pid"].as_i64().is_some()
        && crate::telemetry::daemon_heal::ineffective_restart(event)
}

fn recovered_node(event: &Value) -> Option<String> {
    let floor = event["floor"].as_f64().unwrap_or(0.5);
    let (_, node) = EFFECTIVE.iter().find(|(name, _)| event["name"] == *name)?;
    (event["outcome"] == "restarted" && event["new_pid"].as_i64().is_some()
        && event["new_usage_one_core"].as_f64().is_some_and(|u| u < floor)).then(|| node.to_string())
}

fn closable<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    tree["nodes"].as_array()?.iter().find(|n| n["id"] == id)
        .filter(|n| n["script_close"] == true && n["answer"].as_str().is_some_and(|a| !a.trim().is_empty()))
}

/// Restarts of `name` in the last 24 h; for N7.1.3 only the ineffective ones, since an effective
/// restart earlier that day is not a recurrence of the ineffective case.
fn daemon_restarts_24h(home: &Path, name: &str, now_ms: f64, ineffective_only: bool) -> usize {
    let Ok(entries) = std::fs::read_dir(root(home).join("daemon-restarts")) else { return 0 };
    entries.flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .flat_map(|s| s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect::<Vec<_>>())
        .filter(|r| r["name"] == name && r["at_ms"].as_f64().is_some_and(|at| now_ms >= at && now_ms - at < DAY_MS))
        .filter(|r| !ineffective_only || crate::telemetry::daemon_heal::ineffective_restart(r))
        .count()
}

/// Closes incident `id` by script when its node allows it; returns what the notification says.
pub(super) fn close(home: &Path, id: &str, kind: &str, event: &Value, now_ms: f64) -> Result<Option<Value>> {
    let Some(node_id) = named_node(kind, event) else { return Ok(None) };
    let tree = crate::cause_tree::load(home)?;
    let Some(node) = closable(&tree, &node_id) else { return Ok(None) };
    let ledger_repeats = 1 + ledger::rows(home).iter().filter(|r| r["node"] == node_id.as_str()
        && r["at_ms"].as_f64().is_some_and(|at| now_ms - at < DAY_MS)).count();
    let repeats = if kind == "daemon" {
        ledger_repeats.max(daemon_restarts_24h(home, event["name"].as_str().unwrap_or_default(), now_ms, node_id == INEFFECTIVE_NODE))
    } else {
        ledger_repeats
    };
    if node["max_script_closes_24h"].as_u64().is_some_and(|max| repeats > max as usize) {
        return Ok(None);
    }
    let decision = node["decision"].as_str().filter(|_| repeats >= 2);
    let directory = root(home).join("analysis").join(id);
    std::fs::create_dir_all(&directory)?;
    write_json(&directory.join("report.json"), &json!({
        "incident": id, "trigger": "forced_recovery", "kind": kind, "route": "recovery-retro",
        "status": "closed_by_script", "closed_by": "cause-tree", "verdict": "known_cause", "cause": node["answer"],
        "cause_tree": {"node": node_id, "new_nodes": [], "no_new_node_reason": "script closure on a known node"},
        "recurrence_24h": repeats, "decision_needed": decision, "recovery": event,
        "verification": {"passed": true, "outcome": event["outcome"]},
    }))?;
    std::fs::write(directory.join("report.md"), report_md(id, kind, &node_id, node, event, repeats, decision))?;
    write_json(&root(home).join(format!("incidents/{id}.ack")),
        &json!({"acknowledged_at_ms": now_ms, "closed_by": "cause-tree", "node": node_id}))?;
    ledger::append(home, &json!({"incident": id, "trigger": format!("forced_recovery:{kind}"), "period": "live",
        "closed_by": "script", "node": node_id, "recurrence_24h": repeats, "session": null, "at_ms": now_ms}))?;
    Ok(Some(json!({"node": node_id, "recurrence_24h": repeats, "decision": decision})))
}

/// "；Storybook 已重启；重启前就在运行、收不到文件事件的进程：…" for an fseventsd restart record.
fn rewatch_line(rewatch: &Value) -> String {
    if rewatch.is_null() { return String::new(); }
    let storybook = match rewatch["storybook"]["outcome"].as_str() {
        Some("restarted") => "Storybook 已重启（文件监听恢复）".to_string(),
        Some("failed") => "Storybook 重启失败，热更新可能失效".to_string(),
        _ => format!("Storybook 未重启（{}）", rewatch["storybook"]["reason"].as_str().unwrap_or("?")),
    };
    let stale: Vec<String> = rewatch["stale_watchers"].as_array().into_iter().flatten()
        .map(|w| format!("{} {}", w["name"].as_str().unwrap_or("?"), w["pid"])).collect();
    if stale.is_empty() { format!("；{storybook}") }
    else { format!("；{storybook}；重启前就在运行、收不到文件事件的进程（重启各自进程后恢复）：{}", stale.join("、")) }
}

fn report_md(id: &str, kind: &str, node_id: &str, node: &Value, event: &Value, repeats: usize, decision: Option<&str>) -> String {
    let pending = decision.map_or("- 无：首次出现，按已知原因结案。\n".to_string(),
        |d| format!("- 24 小时内第 {repeats} 次，同一原因反复出现，需要你决定：{d}\n"));
    let (heading, detail) = if kind == "daemon" {
        ("守护进程", format!("守护进程：{}（pid {} → {}）；结果：{}；重启前均值 {:.3} 核，重启后 {:.4} 核",
            event["name"].as_str().unwrap_or("?"), event["pid"], event["new_pid"],
            event["outcome"].as_str().unwrap_or("?"),
            event["usage_one_core"].as_f64().unwrap_or(0.0),
            event["new_usage_one_core"].as_f64().unwrap_or(0.0))
            + &event["footprint"]["footprint_gib"].as_f64().map_or(String::new(), |before| format!("；物理占用 {before} GiB → {} GiB",
                event["new_footprint"]["footprint_gib"].as_f64().map_or("?".to_string(), |after| after.to_string())))
            + &rewatch_line(&event["rewatch"]))
    } else {
        ("Docker", format!("结果：{}；停机方：{}",
            event["outcome"].as_str().unwrap_or("?"),
            event["attribution"]["stopped_by"]["actor"].as_str().unwrap_or("?")))
    };
    format!("# {heading} 强制恢复（事件 {id}）· 脚本结案\n\n\
        结论：原因树节点 {node_id} 已解释这次恢复（{kind}），没有开模型会话。\n\n\
        ## 恢复\n\n- {detail}。\n\n\
        ## 已知原因\n\n- {question}\n- {answer}\n\n## 待决定\n\n{pending}\n\
        原因树：~/.codex/skills/watchdog/references/cause-tree.json；台账：diagnosis-ledger.ndjson。\n",
        question = node["question"].as_str().unwrap_or(""), answer = node["answer"].as_str().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_opted_in_answered_nodes_close_after_a_successful_recovery() {
        let tree = json!({"nodes": [
            {"id": "N4.9", "status": "open", "script_close": true, "answer": "concurrent builds exhaust VM memory"},
            {"id": "N4.6", "status": "open", "script_close": true, "answer": " "},
            {"id": "N4.4", "status": "closed", "answer": "no"},
            {"id": "N7.1", "status": "external", "script_close": true, "max_script_closes_24h": 1, "answer": "Apple powerlog spin"},
        ]});
        assert!(closable(&tree, "N4.9").is_some());
        assert!(closable(&tree, "N7.1").is_some());
        assert!(closable(&tree, "N4.6").is_none() && closable(&tree, "N4.4").is_none());
        let recovered = json!({"outcome": "recovered", "attribution": {"node": "N4.9"}});
        assert_eq!(named_node("docker", &recovered).as_deref(), Some("N4.9"));
        assert_eq!(named_node("docker", &json!({"outcome": "failed", "attribution": {"node": "N4.9"}})), None);
        assert_eq!(named_node("docker-redis", &json!({"outcome": "repaired"})).as_deref(), Some("N4.7"));
        assert_eq!(named_node("daemon", &recovered), None);
        let pps_ok = json!({"name": "PerfPowerServices", "outcome": "restarted", "new_pid": 40998, "floor": 0.5, "new_usage_one_core": 0.01});
        let pps_hot = json!({"name": "PerfPowerServices", "outcome": "restarted", "new_pid": 40998, "floor": 0.5, "new_usage_one_core": 0.62});
        assert_eq!(named_node("daemon", &pps_ok).as_deref(), Some("N7.1"));
        assert_eq!(named_node("daemon", &pps_hot).as_deref(), Some("N7.1.3"));
        let pps_failed = json!({"name": "PerfPowerServices", "outcome": "failed: sudo", "floor": 0.5});
        assert_eq!(named_node("daemon", &pps_failed), None);
        // fseventsd: an effective restart closes on N8.4.1.1; a hot successor names no node.
        let fse_ok = json!({"name": "fseventsd", "outcome": "restarted", "new_pid": 812, "floor": 0.8, "new_usage_one_core": 0.05});
        let fse_hot = json!({"name": "fseventsd", "outcome": "restarted", "new_pid": 812, "floor": 0.8, "new_usage_one_core": 0.95});
        assert_eq!(named_node("daemon", &fse_ok).as_deref(), Some("N8.4.1.1"));
        assert_eq!(named_node("daemon", &fse_hot), None);
        let line = rewatch_line(&json!({"storybook": {"outcome": "restarted"}, "stale_watchers": [{"pid": 37307, "name": "2.1.325"}]}));
        assert!(line.contains("Storybook 已重启") && line.contains("2.1.325 37307"), "{line}");
        assert_eq!(rewatch_line(&Value::Null), "");
        assert_eq!(named_node("daemon", &json!({"name": "syspolicyd", "outcome": "restarted", "new_pid": 9, "floor": 0.5, "new_usage_one_core": 0.01})), None);
    }

    #[test]
    fn an_ineffective_restart_closes_by_script_despite_an_effective_one_earlier_that_day() {
        let home = crate::telemetry::test_home("known-daemon-ineffective");
        let tree_dir = home.join(".codex/skills/watchdog/references");
        std::fs::create_dir_all(&tree_dir).unwrap();
        std::fs::write(tree_dir.join("cause-tree.json"), serde_json::to_vec(&json!({"nodes": [
            {"id": "N7.1.3", "status": "external", "script_close": true, "max_script_closes_24h": 1,
             "question": "Why does a successor stay hot?", "answer": "Restart does not help; held 24 h."}
        ]})).unwrap()).unwrap();
        // 11-04 shape: an effective restart at 14:10, then an ineffective one at 17:28.
        let log = root(&home).join("daemon-restarts");
        std::fs::create_dir_all(&log).unwrap();
        let effective = json!({"at_ms": 1_000_000.0, "name": "PerfPowerServices", "outcome": "restarted", "new_pid": 2, "floor": 0.5, "new_usage_one_core": 0.13});
        let hot = json!({"at_ms": 2_000_000.0, "name": "PerfPowerServices", "outcome": "restarted", "pid": 2, "new_pid": 3,
            "usage_one_core": 0.87, "floor": 0.5, "new_usage_one_core": 0.79});
        std::fs::write(log.join("0.ndjson"), format!("{effective}\n{hot}\n")).unwrap();
        let closed = close(&home, "inc-hot", "daemon", &hot, 2_000_000.0).unwrap().unwrap();
        assert_eq!(closed["node"], "N7.1.3");
        assert_eq!(closed["recurrence_24h"], 1);
        let report = std::fs::read_to_string(root(&home).join("analysis/inc-hot/report.md")).unwrap();
        assert!(report.contains("N7.1.3") && report.contains("0.7900"), "{report}");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn daemon_script_close_stops_after_first_occurrence_in_24h() {
        let home = crate::telemetry::test_home("known-daemon-cap");
        let tree_dir = home.join(".codex/skills/watchdog/references");
        std::fs::create_dir_all(&tree_dir).unwrap();
        std::fs::write(tree_dir.join("cause-tree.json"), serde_json::to_vec(&json!({"nodes": [
            {"id": "N7.1", "status": "external", "script_close": true, "max_script_closes_24h": 1,
             "question": "Why does PerfPowerServices spin?", "answer": "Inside Apple binary; SIGTERM cures it."}
        ]})).unwrap()).unwrap();
        let event = json!({"name": "PerfPowerServices", "outcome": "restarted", "pid": 161, "new_pid": 40998,
            "usage_one_core": 0.502, "floor": 0.5, "new_usage_one_core": 0.003});
        let first = close(&home, "inc-1", "daemon", &event, 1_000_000.0).unwrap();
        assert_eq!(first.unwrap()["node"], "N7.1");
        let second = close(&home, "inc-2", "daemon", &event, 2_000_000.0).unwrap();
        assert!(second.is_none(), "second restart within 24h must open a session");
        let after_24h = close(&home, "inc-3", "daemon", &event, 1_000_000.0 + DAY_MS + 1.0).unwrap();
        assert_eq!(after_24h.unwrap()["node"], "N7.1");
        std::fs::remove_dir_all(home).unwrap();
    }
}
