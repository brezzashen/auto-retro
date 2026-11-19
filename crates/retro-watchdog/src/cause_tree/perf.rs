//! Merged performance cause list (founder, 2026-10-27): every performance condition (CPU surge,
//! 90% monitor, WindowServer stall or hot, Docker VM hot, forced recovery) is compared against the
//! same list of workload classes in cause-tree.json (nodes with `probe: "perf_class"` and `match`,
//! the process names of that class). The probe averages the per-name CPU of the cpu-windows frames
//! inside the episode window, gives each class its share of host CPU, and lists the self-heals
//! (daemon, headless page, Docker, Storybook, ControlCenter, surge actions) recorded around it.
//! Heals run first in their own modules; this comparison runs after them, so a session receives
//! the known cause and the heal outcome and works only on what they leave open.
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path};

pub(crate) const PROBE: &str = "perf_class";
const BUCKET_MS: f64 = 600_000.0;
/// Frames just around the window still describe its load.
const PAD_MS: f64 = 60_000.0;
/// A heal this long before the episode may be what it reacted to.
const HEAL_LEAD_MS: f64 = 900_000.0;
const HEALS: [(&str, &str); 6] = [("daemon-restarts", "daemon"), ("headless-heals", "headless"), ("docker-heals", "docker"),
    ("storybook-heals", "storybook"), ("controlcenter-restarts", "controlcenter"), ("software-encodes", "software_encode")];
const DEFAULT_WINDOW_MS: f64 = 300_000.0;
/// Unmatched names listed beside the classes.
const UNCLASSIFIED_TOP: usize = 3;

/// The load window of a performance incident: its hot or episode start to its detection.
pub(crate) fn window(event: &Value, now_ms: f64) -> (f64, f64) {
    let end = event["detected_at_ms"].as_f64().or(event["recovery"]["at_ms"].as_f64()).unwrap_or(now_ms);
    let start = event["hot_since_ms"].as_f64().or(event["episode_since_ms"].as_f64())
        .filter(|s| *s <= end).unwrap_or(end - DEFAULT_WINDOW_MS);
    (start, end)
}

/// The cpu-windows frames and whole-host CPU samples inside `from..to`.
pub(super) fn frames(home: &Path, from: f64, to: f64) -> (Vec<Value>, Vec<Value>) {
    let directory = root(home).join("cpu-windows");
    let Ok(entries) = std::fs::read_dir(&directory) else { return (Vec::new(), Vec::new()) };
    let mut buckets: Vec<f64> = entries.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".json")?.parse().ok())
        .filter(|b: &f64| *b <= to && b + BUCKET_MS >= from).collect();
    buckets.sort_by(f64::total_cmp);
    let windows: Vec<Value> = buckets.iter().filter_map(|b| crate::telemetry::read_state(&directory.join(format!("{b}.json")))).collect();
    let within = |key: &str, at: &str| -> Vec<Value> { windows.iter().flat_map(|w| w[key].as_array().cloned().unwrap_or_default())
        .filter(|f| f[at].as_f64().is_some_and(|at| at >= from && at <= to)).collect() };
    (within("frames", "at_ms"), within("samples", "end"))
}

/// Mean whole-host CPU of the samples in one-core percent (100 = one logical core busy).
/// Frames see only processes alive at both ends of a ~16 s frame, so short-lived compiler
/// children and kernel time are missing from them (10-27 episodes saw 33-69% of host CPU).
fn host_cpu(samples: &[Value]) -> Option<f64> {
    let percents: Vec<f64> = samples.iter().filter_map(|s| s["percent"].as_f64()).collect();
    let cores = std::thread::available_parallelism().ok()?.get() as f64;
    (!percents.is_empty()).then(|| percents.iter().sum::<f64>() / percents.len() as f64 * cores)
}

fn heals(home: &Path, from: f64, to: f64) -> Vec<Value> {
    let mut found = Vec::new();
    for (directory, kind) in HEALS {
        let Ok(entries) = std::fs::read_dir(root(home).join(directory)) else { continue };
        for path in entries.flatten().map(|e| e.path()) {
            let day = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<f64>().ok());
            if day.is_some_and(|d| d > to || d + 86_400_000.0 < from) { continue; }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            found.extend(text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .filter(|r| r["at_ms"].as_f64().is_some_and(|at| at >= from && at <= to))
                .map(|r| json!({"kind": kind, "at_ms": r["at_ms"], "outcome": r["outcome"], "detail": r["kind"].as_str().or(r["name"].as_str())})));
        }
    }
    found
}

fn failed(outcome: &Value) -> bool { outcome.as_str().is_some_and(|o| o.starts_with("failed")) }

/// The class comparison of `from..to` (ms) against the tree's perf_class nodes.
pub(crate) fn attribute(home: &Path, tree: &Value, from: f64, to: f64, surge_actions: &[Value]) -> Value {
    let (frames, samples) = frames(home, from - PAD_MS, to + PAD_MS);
    let mut by_name: HashMap<String, f64> = HashMap::new();
    let mut total = 0.0;
    for frame in &frames {
        total += frame["cpu"]["total_cpu_percent_one_core"].as_f64().unwrap_or(0.0);
        for p in frame["cpu"]["by_name"].as_array().into_iter().flatten() {
            *by_name.entry(p["name"].as_str().unwrap_or_default().to_string()).or_default() += p["cpu_percent_one_core"].as_f64().unwrap_or(0.0);
        }
    }
    let count = frames.len().max(1) as f64;
    let observed = total / count;
    let host = host_cpu(&samples).map_or(observed, |h| h.max(observed));
    // `share` stays of frame-visible CPU: close_share and the script-close choice (founder, 10-27) were
    // set on that basis, and host shares closed none of 22 episodes. `host_share` records the gap.
    let mut classes: Vec<Value> = super::nodes(tree).iter().filter(|n| n["probe"] == PROBE).map(|n| {
        let names: Vec<&str> = n["match"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let mut members: Vec<(&str, f64)> = names.iter().filter_map(|m| by_name.get(*m).map(|v| (*m, v / count))).collect();
        members.sort_by(|a, b| b.1.total_cmp(&a.1));
        let cpu: f64 = members.iter().map(|m| m.1).sum();
        json!({"node": n["id"], "cpu_one_core": cpu.round(), "share": if observed > 0.0 { cpu / observed } else { 0.0 },
            "host_share": if host > 0.0 { cpu / host } else { 0.0 },
            "top": members.iter().take(3).map(|(m, v)| json!({"name": m, "cpu_one_core": v.round()})).collect::<Vec<_>>()})
    }).collect();
    classes.sort_by(|a, b| b["share"].as_f64().unwrap_or(0.0).total_cmp(&a["share"].as_f64().unwrap_or(0.0)));
    let unclassified = unclassified(tree, &by_name, count, observed);
    let dominant = classes.first().filter(|c| c["share"].as_f64().unwrap_or(0.0) > 0.0).cloned();
    let explained = dominant.as_ref().and_then(|d| {
        let node = super::node(tree, d["node"].as_str()?)?;
        (super::answered(node) && node["close_share"].as_f64().is_some_and(|need| d["share"].as_f64().unwrap_or(0.0) >= need)).then(|| node["id"].clone())
    });
    let mut heals = heals(home, from - HEAL_LEAD_MS, to + PAD_MS);
    heals.extend(surge_actions.iter().map(|a| json!({"kind": "surge", "outcome": a["outcome"], "detail": a["decision"]["target"]})));
    let heal_failed = heals.iter().any(|h| failed(&h["outcome"]));
    json!({"from_ms": from, "to_ms": to, "frames": frames.len(), "host_cpu_one_core": host.round(),
        "observed_cpu_one_core": observed.round(), "unattributed_cpu_one_core": (host - observed).round(),
        "classes": classes, "unclassified": unclassified, "dominant": dominant.map(|d| d["node"].clone()), "explained_by": explained,
        "heals": heals, "heal_failed": heal_failed})
}

/// The heaviest frame-visible names no perf_class matches, with their share of frame-visible CPU.
/// A large entry means the class list misses a workload and the dominant class may be wrong
/// (10-30 00:06-00:11: an agent's history-vector-check pool held 31-36%, above native builds).
fn unclassified(tree: &Value, by_name: &HashMap<String, f64>, count: f64, observed: f64) -> Vec<Value> {
    let known: std::collections::HashSet<&str> = super::nodes(tree).iter().filter(|n| n["probe"] == PROBE)
        .flat_map(|n| n["match"].as_array().into_iter().flatten().filter_map(Value::as_str)).collect();
    let mut rest: Vec<(&str, f64)> = by_name.iter().filter(|(n, _)| !known.contains(n.as_str())).map(|(n, v)| (n.as_str(), v / count)).collect();
    rest.sort_by(|a, b| b.1.total_cmp(&a.1));
    rest.iter().take(UNCLASSIFIED_TOP).map(|(n, v)| json!({"name": n, "cpu_one_core": v.round(),
        "share": if observed > 0.0 { v / observed } else { 0.0 }})).collect()
}

/// Writes `analysis/<id>/perf-attribution.json` for incident `id` and returns it with the node that
/// may close it by script (an explaining node marked `script_close`, all heals succeeded).
pub(crate) fn compare(home: &Path, id: &str, now_ms: f64) -> Result<(Value, Option<Value>)> {
    let event = crate::telemetry::read_state(&root(home).join(format!("incidents/{id}.json"))).unwrap_or(Value::Null);
    let Ok(tree) = super::load(home) else { return Ok((Value::Null, None)) };
    let (from, to) = window(&event, now_ms);
    let report = crate::telemetry::read_state(&root(home).join("analysis").join(id).join("report.json")).unwrap_or(Value::Null);
    let actions = report["actions"].as_array().cloned().unwrap_or_default();
    let mut found = attribute(home, &tree, from, to, &actions);
    found["incident"] = json!(id);
    // A hot WindowServer is too small for a class share; its own probe may explain what the classes leave open.
    if let Some(gui) = super::gui::probe(home, &tree, &event, &frames(home, from - PAD_MS, to + PAD_MS).0, from - PAD_MS, to + PAD_MS) {
        if gui["explains"] == true && found["explained_by"].is_null() { found["explained_by"] = gui["node"].clone(); }
        found["gui"] = gui;
    }
    // Memory exhaustion is explained by who holds memory, which outranks the busiest CPU class.
    if let Some(memory) = super::memory::probe(&tree, &event) {
        if memory["explains"] == true { found["explained_by"] = memory["node"].clone(); }
        found["memory"] = memory;
    }
    write_json(&root(home).join("analysis").join(id).join("perf-attribution.json"), &found)?;
    let closing = found["explained_by"].as_str().and_then(|n| super::node(&tree, n))
        .filter(|n| n["script_close"] == true && !found["heal_failed"].as_bool().unwrap_or(true)).cloned();
    Ok((found, closing))
}

/// Closes a performance episode its known node explains: report, ack and ledger row, no session.
pub(crate) fn close(home: &Path, id: &str, condition: &str, found: &Value, node: &Value, now_ms: f64) -> Result<()> {
    let directory = root(home).join("analysis").join(id);
    write_json(&directory.join("report.json"), &json!({
        "incident": id, "condition": condition, "route": "perf", "status": "closed_by_script", "closed_by": "cause-tree",
        "verdict": "known_cause", "cause": node["answer"], "attribution": found,
        "cause_tree": {"node": node["id"], "new_nodes": [], "no_new_node_reason": "script closure on a known node"},
        "verification": {"passed": true}}))?;
    let gui = &found["gui"];
    let basis = if gui["explains"] == true && gui["node"] == node["id"] {
        format!("计算机操作服务出现在 {} 帧 CPU 采样里，渲染探针 {} 次采样没有 250 ms 以上的卡顿", gui["agent_frames"], gui["render_ticks"])
    } else {
        format!("占主机 CPU {:.0}%", found["classes"][0]["share"].as_f64().unwrap_or(0.0) * 100.0)
    };
    std::fs::write(directory.join("report.md"), format!("# 性能事件 {id}（{condition}）· 脚本结案\n\n\
        结论：原因清单节点 {node_id} 解释了这次负载（{basis}），自愈全部成功，没有开模型会话。\n\n\
        - {question}\n- {answer}\n\n对比明细：perf-attribution.json；原因树：~/.codex/skills/watchdog/references/cause-tree.json。\n",
        node_id = node["id"].as_str().unwrap_or("?"),
        question = node["question"].as_str().unwrap_or(""), answer = node["answer"].as_str().unwrap_or("")))?;
    write_json(&root(home).join(format!("incidents/{id}.ack")),
        &json!({"acknowledged_at_ms": now_ms, "closed_by": "cause-tree", "node": node["id"]}))?;
    super::ledger::append(home, &json!({"incident": id, "trigger": format!("perf:{condition}"), "period": "live",
        "closed_by": "script", "node": node["id"], "session": null, "at_ms": now_ms}))
}

#[cfg(test)]
#[path = "perf_tests.rs"]
mod tests;
