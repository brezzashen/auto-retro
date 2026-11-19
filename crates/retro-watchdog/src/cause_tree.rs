//! Recursive diagnosis (founder, 2026-10-25): twelve port-leak sessions in four days re-derived
//! the same microphone cause, and the deeper AudioVideoModule finding never reached the route
//! reference. The cause tree (`~/.codex/skills/watchdog/references/cause-tree.json`) holds every
//! answered question with its probe. Before a port-leak session opens, the probes of closed or
//! external nodes run on the incident; a node that explains at least its `close_share` of the
//! growth closes the incident by script (report, ack, ledger row) once the ControlCenter restart
//! succeeded. Otherwise the session opens with the attribution, to work only on the residual.
//! A model session on a tree trigger is acknowledged only when its report.json `cause_tree`
//! names the explaining node and the nodes it added (or why none), so findings become nodes.
use crate::monitor::{root, write_json};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

pub(crate) mod attribute;
pub(crate) mod capture;
pub(crate) mod gui;
pub(crate) mod ledger;
pub(crate) mod memory;
pub(crate) mod perf;
pub(crate) mod sharing;
pub(crate) mod queued;
pub(crate) mod universal;

/// The restarted instance must start below this, as port-leak.md step 5 requires.
const RECOVERED_PORTS: u64 = 2_000;

pub(crate) fn load(home: &Path) -> Result<Value> {
    let path = home.join(".codex/skills/watchdog/references/cause-tree.json");
    serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("read {}", path.display()))?)
        .context("cause-tree.json is not JSON")
}

pub(crate) fn nodes(tree: &Value) -> &[Value] {
    tree["nodes"].as_array().map_or(&[], Vec::as_slice)
}

pub(crate) fn node<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    nodes(tree).iter().find(|n| n["id"] == id)
}

pub(crate) fn answered(node: &Value) -> bool {
    node["status"] == "closed" || node["status"] == "external"
}

/// Whether `id` sits below a node that lists `trigger`.
pub(crate) fn under(tree: &Value, id: &str, trigger: &str) -> bool {
    let mut current = node(tree, id);
    while let Some(n) = current {
        if n["triggers"].as_array().is_some_and(|t| t.iter().any(|x| x == trigger)) { return true; }
        current = n["parent"].as_str().and_then(|p| node(tree, p));
    }
    false
}

/// The tree trigger of an incident: `trigger:kind` when the tree lists it (forced_recovery:docker),
/// else the bare trigger when the tree lists that.
pub(crate) fn tree_trigger(tree: &Value, event: &Value) -> Option<String> {
    let map = trigger_nodes(tree);
    let trigger = event["trigger"].as_str()?;
    let qualified = event["kind"].as_str().map(|kind| format!("{trigger}:{kind}"));
    qualified.filter(|q| map.contains_key(q)).or_else(|| map.contains_key(trigger).then(|| trigger.to_string()))
}

/// Each tree trigger and its first answered child, the node a baseline report is counted under.
pub(crate) fn trigger_nodes(tree: &Value) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for parent in nodes(tree) {
        let child = nodes(tree).iter().find(|n| n["parent"] == parent["id"] && answered(n));
        for trigger in parent["triggers"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            // The first parent listing a trigger keeps it: the merged performance branch N8 lists every
            // performance trigger after the specific branches (N3, N4) that answered them first.
            if let Some(id) = child.and_then(|c| c["id"].as_str()) { map.entry(trigger.to_string()).or_insert_with(|| id.to_string()); }
        }
    }
    map
}

/// The answered node whose probe explains enough of `attribution` for `trigger`.
pub(crate) fn explaining<'a>(tree: &'a Value, trigger: &str, attribution: &Value) -> Option<&'a Value> {
    nodes(tree).iter().filter(|n| answered(n) && under(tree, n["id"].as_str().unwrap_or_default(), trigger))
        .find(|n| {
            let probe = &attribution["probes"][n["probe"].as_str().unwrap_or_default()];
            n["close_share"].as_f64().zip(probe["share"].as_f64()).is_some_and(|(need, got)| got >= need)
                && attribution["growth"].as_i64().is_some_and(|g| g > 0)
        })
}

fn attribution(home: &Path, id: &str, now: f64) -> Result<(Value, Value)> {
    let incident = crate::telemetry::read_state(&root(home).join(format!("incidents/{id}.json"))).context("incident not found")?;
    ensure!(incident["trigger"] == "controlcenter_port_leak", "attribution covers controlcenter_port_leak only");
    let pid = incident["pid"].as_i64().context("incident without pid")?;
    let start = incident["process_start"].as_f64().context("incident without process_start")?;
    let mut value = attribute::controlcenter(home, pid, start, now * 1000.0)?;
    value["incident"] = json!(id);
    value["pid"] = json!(pid);
    Ok((incident, value))
}

/// Closes a ControlCenter port-leak incident by script when a known node explains it; the
/// attribution is written either way for the session that follows.
pub(crate) fn close_port_leak(home: &Path, now: f64, id: &str) -> Result<Option<Value>> {
    let (incident, mut found) = attribution(home, id, now)?;
    let tree = load(home)?;
    let restart = crate::telemetry::read_state(&root(home).join("controlcenter-restart.json")).unwrap_or(Value::Null);
    let recovered = restart["incident"] == id && restart["outcome"] == "restarted"
        && restart["new_ports"].as_u64().is_some_and(|p| p < RECOVERED_PORTS);
    let node = explaining(&tree, "controlcenter_port_leak", &found).cloned();
    found["explained_by"] = node.as_ref().map_or(Value::Null, |n| n["id"].clone());
    found["restart_recovered"] = json!(recovered);
    let directory = root(home).join("analysis").join(id);
    write_json(&directory.join("attribution.json"), &found)?;
    let Some(node) = node.filter(|_| recovered) else { return Ok(None) };
    close(home, now, id, &incident, &found, &restart, &node, "live").map(Some)
}

/// Writes the script closure of a port-leak incident: report.json, report.md, ack, ledger row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn close(home: &Path, now: f64, id: &str, incident: &Value, found: &Value, restart: &Value, node: &Value,
    at: &str) -> Result<Value> {
    let directory = root(home).join("analysis").join(id);
    let share = found["probes"][node["probe"].as_str().unwrap_or_default()]["share"].as_f64().unwrap_or(0.0);
    let residual = &found["probes"]["cc_residual"];
    write_json(&directory.join("report.json"), &json!({
        "incident": id, "trigger": incident["trigger"], "route": "port-leak", "status": "closed_by_script",
        "closed_by": "cause-tree", "verdict": "known_cause", "cause": node["answer"],
        "cause_tree": {"node": node["id"], "new_nodes": [], "no_new_node_reason": "script closure on a known node"},
        "repair": restart, "verification": {"passed": true, "new_pid": restart["new_pid"], "new_ports": restart["new_ports"]},
        "growth": found, "open_items": [format!("residual {} ports outside known windows (node N2.2)", residual["growth"])],
    }))?;
    std::fs::write(directory.join("report.md"), report_md(id, incident, node, found, restart, share))?;
    write_json(&root(home).join(format!("incidents/{id}.ack")),
        &json!({"acknowledged_at_ms": now * 1000.0, "closed_by": "cause-tree", "node": node["id"]}))?;
    ledger::append(home, &json!({"incident": id, "trigger": incident["trigger"], "period": "live", "closed_by": "script",
        "node": node["id"], "share": share, "growth": found["growth"], "residual": residual["growth"],
        "session": null, "closed_at": at, "at_ms": now * 1000.0}))?;
    Ok(json!({"node": node["id"], "share": share, "growth": found["growth"]}))
}

fn report_md(id: &str, incident: &Value, node: &Value, found: &Value, restart: &Value, share: f64) -> String {
    let probe = |name: &str, field: &str| found["probes"][name][field].to_string();
    format!("# ControlCenter Mach 端口泄漏（事件 {id}）· 脚本结案\n\n\
        结论：已知原因 {node_id} 解释了 {pct:.0}% 的增长，按原因树脚本结案，没有开模型会话。\n\n\
        ## 触发\n\n- pid {pid}：{ports} 个端口，运行 {uptime} s。\n\n\
        ## 处置\n\n- 看门狗已重启 ControlCenter：pid {old} → {new}，新实例 {new_ports} 个端口。\n\n\
        ## 增长速率与归因\n\n| 探针 | 增长 | 占比 |\n|---|---|---|\n\
        | AudioVideoModule 创建（{av_n} 次） | {av_g} | {av_s} |\n| 麦克风会话（{mic_n} 次） | {mic_g} | {mic_s} |\n\
        | 其他模块创建（{mod_n} 次） | {mod_g} | {mod_s} |\n| 屏幕共享会话（{ss_n} 次） | {ss_g} | {ss_s} |\n\
        | 截屏流启动（{cap_n} 次） | {cap_g} | {cap_s} |\n| Universal Control 接入（{uc_n} 次） | {uc_g} | {uc_s} |\n\
        | 菜单栏重建合计 | {mb_g} | {mb_s} |\n| 全部已知窗口（含麦克风台账） | {kw_g} | {kw_s} |\n| 残差（以上窗口之外） | {res_g} | {res_s} |\n\n- 总增长 {growth}，历时 {hours} h。\n\n\
        ## 待确认\n\n- 残差归 N2.2；原因树：~/.codex/skills/watchdog/references/cause-tree.json。\n",
        node_id = node["id"].as_str().unwrap_or("?"), pct = share * 100.0, pid = incident["pid"], ports = incident["ports"],
        uptime = incident["uptime_s"], old = restart["pid"], new = restart["new_pid"], new_ports = restart["new_ports"],
        av_n = probe("cc_av_module", "events"), av_g = probe("cc_av_module", "growth"), av_s = probe("cc_av_module", "share"),
        mod_n = probe("cc_other_modules", "events"), mod_g = probe("cc_other_modules", "growth"), mod_s = probe("cc_other_modules", "share"),
        ss_n = probe("cc_screen_sharing", "events"), ss_g = probe("cc_screen_sharing", "growth"), ss_s = probe("cc_screen_sharing", "share"),
        cap_n = probe("cc_screen_capture", "events"), cap_g = probe("cc_screen_capture", "growth"), cap_s = probe("cc_screen_capture", "share"),
        uc_n = probe("cc_universal_control", "events"), uc_g = probe("cc_universal_control", "growth"), uc_s = probe("cc_universal_control", "share"),
        mb_g = probe("cc_menubar_rebuild", "growth"), mb_s = probe("cc_menubar_rebuild", "share"),
        kw_g = probe("cc_known_windows", "growth"), kw_s = probe("cc_known_windows", "share"),
        mic_n = probe("cc_mic_sessions", "sessions"), mic_g = probe("cc_mic_sessions", "growth"), mic_s = probe("cc_mic_sessions", "share"),
        res_g = probe("cc_residual", "growth"), res_s = probe("cc_residual", "share"), growth = found["growth"], hours = found["hours"])
}

/// monitor-ack gate for tree triggers: the report names its node and what it added to the tree.
pub(crate) fn validate_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    // Without an installed tree (test homes, a fresh host) there is nothing to extend.
    if !home.join(".codex/skills/watchdog/references/cause-tree.json").exists() { return Ok(()); }
    let tree = load(home)?;
    let Some(trigger) = tree_trigger(&tree, &event) else { return Ok(()) };
    let trigger = trigger.as_str();
    let report: Value = serde_json::from_slice(&std::fs::read(root(home).join("analysis").join(id).join("report.json"))
        .context("report.json missing")?)?;
    let entry = &report["cause_tree"];
    let named = entry["node"].as_str().context(
        "report.json needs cause_tree.node: the cause-tree.json node that explains this incident")?;
    ensure!(node(&tree, named).is_some_and(|_| under(&tree, named, trigger)), "cause_tree.node {named} is not under {trigger} in cause-tree.json");
    let added: Vec<&str> = entry["new_nodes"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    for new in &added { ensure!(node(&tree, new).is_some(), "cause_tree.new_nodes {new} is not in cause-tree.json; add the node first"); }
    ensure!(!added.is_empty() || entry["no_new_node_reason"].as_str().is_some_and(|r| !r.trim().is_empty()),
        "report.json needs cause_tree.new_nodes (ids added to cause-tree.json) or cause_tree.no_new_node_reason");
    Ok(())
}

/// Ledger row for a model session on a tree trigger, after its acknowledgement.
pub(crate) fn record_ack(home: &Path, id: &str, incident: &Path) -> Result<()> {
    let event: Value = serde_json::from_slice(&std::fs::read(incident)?)?;
    let Some(trigger) = load(home).ok().and_then(|tree| tree_trigger(&tree, &event)) else { return Ok(()) };
    let report = crate::telemetry::read_state(&root(home).join("analysis").join(id).join("report.json")).unwrap_or(Value::Null);
    let added = report["cause_tree"]["new_nodes"].as_array().cloned().unwrap_or_default();
    let since = std::fs::metadata(incident)?.modified()?;
    ledger::append(home, &json!({"incident": id, "trigger": trigger, "period": "live", "closed_by": "model",
        "node": report["cause_tree"]["node"], "new_nodes": added, "repeat": added.is_empty(),
        "session": ledger::session_cost(home, id, since), "at_ms": crate::unix_now() * 1000.0}))
}

/// `cpu-watchdog attribute <id>`: the attribution and the node that would close it, without acting.
pub(crate) fn attribute_cli(home: &Path, id: Option<String>) -> Result<()> {
    let id = id.context("usage: cpu-watchdog attribute <incident-id>")?;
    let (_, mut found) = attribution(home, &id, crate::unix_now())?;
    found["explained_by"] = explaining(&load(home)?, "controlcenter_port_leak", &found).map_or(Value::Null, |n| n["id"].clone());
    println!("{}", serde_json::to_string_pretty(&found)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Value {
        json!({"nodes": [
            {"id": "root", "parent": null, "status": "open"},
            {"id": "N2", "parent": "root", "triggers": ["controlcenter_port_leak"], "status": "open"},
            {"id": "N2.1", "parent": "N2", "probe": "cc_av_module", "close_share": 0.85, "status": "external"},
            {"id": "N2.2", "parent": "N2", "probe": "cc_residual", "status": "open"},
            {"id": "N2.5", "parent": "N2", "probe": "cc_menubar_rebuild", "close_share": 0.85, "status": "closed"},
            {"id": "N2.13", "parent": "N2", "probe": "cc_known_windows", "close_share": 0.85, "status": "closed"},
            {"id": "N3", "parent": "root", "triggers": ["windowserver_render_stall"], "status": "open"},
            {"id": "N3.1", "parent": "N3", "status": "closed"},
            {"id": "N4", "parent": "root", "triggers": ["forced_recovery:docker"], "status": "open"},
            {"id": "N4.2", "parent": "N4", "status": "closed"},
        ]})
    }

    #[test]
    fn known_node_closes_only_above_its_share() {
        let tree = tree();
        let at = |share: f64| json!({"growth": 3000, "probes": {"cc_av_module": {"share": share}, "cc_residual": {"share": 0.5}}});
        assert_eq!(explaining(&tree, "controlcenter_port_leak", &at(0.89)).unwrap()["id"], "N2.1");
        assert!(explaining(&tree, "controlcenter_port_leak", &at(0.60)).is_none());
        // Incident 87046536238356200262: AudioVideoModule alone 0.76, all menu-bar rebuilds 1.0.
        let rebuilt = json!({"growth": 3272, "probes": {"cc_av_module": {"share": 0.76}, "cc_menubar_rebuild": {"share": 1.0}}});
        assert_eq!(explaining(&tree, "controlcenter_port_leak", &rebuilt).unwrap()["id"], "N2.5");
        // Incident 810984561562712258015 (11.4 h): module lines of the evening burst rotated out, so
        // AudioVideoModule 0.67, rebuilds 0.75, ledger 0.84; every known window together 0.96.
        let mixed = json!({"growth": 3248, "probes": {"cc_av_module": {"share": 0.67}, "cc_menubar_rebuild": {"share": 0.75},
            "cc_mic_sessions": {"share": 0.84}, "cc_known_windows": {"share": 0.96}}});
        assert_eq!(explaining(&tree, "controlcenter_port_leak", &mixed).unwrap()["id"], "N2.13");
        // Incident 811358242637012976142: every known window 0.80 left a real driver (N2.10) to find.
        let open = json!({"growth": 3264, "probes": {"cc_menubar_rebuild": {"share": 0.66}, "cc_known_windows": {"share": 0.80}}});
        assert!(explaining(&tree, "controlcenter_port_leak", &open).is_none());
        // An open node never closes, whatever its share.
        assert!(explaining(&tree, "windowserver_render_stall", &at(0.99)).is_none());
    }

    #[test]
    fn trigger_nodes_pick_the_first_answered_child() {
        let map = trigger_nodes(&tree());
        assert_eq!(map["controlcenter_port_leak"], "N2.1");
        assert_eq!(map["windowserver_render_stall"], "N3.1");
        assert!(under(&tree(), "N2.2", "controlcenter_port_leak") && !under(&tree(), "N3.1", "controlcenter_port_leak"));
        let docker = json!({"trigger": "forced_recovery", "kind": "docker"});
        assert_eq!(tree_trigger(&tree(), &docker).as_deref(), Some("forced_recovery:docker"));
        assert_eq!(tree_trigger(&tree(), &json!({"trigger": "forced_recovery", "kind": "daemon"})), None);
        assert_eq!(tree_trigger(&tree(), &json!({"trigger": "controlcenter_port_leak"})).as_deref(), Some("controlcenter_port_leak"));
    }
}
