//! Desktop responsiveness from the resident process snapshots, with no extra command: sustained
//! WindowServer and Virtualization (Docker) VM CPU, and idle Automator runners. Each Service run
//! leaves one runner registered as a LaunchServices app; 106 had piled up on 2026-10-20 while Dock
//! switching stalled for seconds. A hot episode or a runner pile-up creates one incident.
//! The render-server fence latency comes from `render_probe.rs`, drained on the same tick.
//! WindowServer and Docker VM hot windows here, and render stalls there, are conditions of the one performance trigger (`wake.rs`).
use super::diagnostics::incident;
use crate::{
    monitor::{root, write_json},
    procs::Proc,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

struct Rule {
    key: &'static str,
    name: &'static str,
    min_cpu_percent_one_core: f64,
    min_secs: f64,
}

const RULES: [Rule; 2] = [
    Rule { key: "windowserver", name: "WindowServer", min_cpu_percent_one_core: 80.0, min_secs: 120.0 },
    // Docker Desktop's Linux VM; any other Virtualization.framework VM is summed in as well.
    Rule { key: "docker_vm", name: "com.apple.Virtualization.VirtualMachine", min_cpu_percent_one_core: 800.0, min_secs: 300.0 },
];
const RUNNER: &str = "com.apple.automator.runner";
const RUNNER_IDLE_MIN_SECS: i64 = 60;
const RUNNER_LEAK_MIN_IDLE: usize = 20;
/// A longer gap between snapshots cannot prove continuity, so it resets every hot episode.
const MAX_GAP_SECS: f64 = 60.0;

#[derive(Default)]
pub struct Desktop {
    previous: Option<(f64, HashMap<(i32, i64), f64>)>,
    hot_since: HashMap<&'static str, f64>,
    leak_since: Option<f64>,
    render: super::render_probe::RenderProbe,
}

impl Desktop {
    pub fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        let elapsed = self.previous.as_ref().map(|(at, _)| now - at).filter(|e| *e > 0.0 && *e <= MAX_GAP_SECS);
        let mut state = serde_json::Map::new();
        state.insert("at_ms".into(), json!(now * 1000.0));
        state.insert("error".into(), Value::Null);
        state.insert("interval_s".into(), json!(elapsed));
        for rule in &RULES {
            let members: Vec<&Proc> = processes.iter().filter(|p| p.name == rule.name).collect();
            // Unknown when any member is new or the interval is unusable: never guess a rate.
            let cpu = match (&self.previous, elapsed) {
                (Some((_, old)), Some(elapsed)) if !members.is_empty() => members
                    .iter()
                    .map(|p| old.get(&(p.pid, p.start)).map(|before| (p.cpu_secs - before).max(0.0) / elapsed * 100.0))
                    .sum::<Option<f64>>(),
                _ => None,
            };
            let since = match cpu {
                Some(cpu) if cpu >= rule.min_cpu_percent_one_core => {
                    Some(*self.hot_since.entry(rule.key).or_insert(now - elapsed.unwrap_or(0.0)))
                }
                _ => {
                    self.hot_since.remove(rule.key);
                    None
                }
            };
            let rss_kib: u64 = members.iter().map(|p| p.rss_kib).sum();
            if let Some(since) = since.filter(|since| now - since >= rule.min_secs) {
                let id = incident(home, &format!("{}_hot:{}", rule.key, since as i64), json!({
                    "trigger": format!("{}_hot", rule.key), "process": rule.name,
                    "hot_since_ms": since * 1000.0, "duration_s": now - since,
                    "cpu_percent_one_core": cpu, "rss_kib": rss_kib,
                    "threshold": {"cpu_percent_one_core": rule.min_cpu_percent_one_core, "min_secs": rule.min_secs},
                }))?;
                let condition = match rule.key {
                    "windowserver" => Some(crate::wake::Condition::WindowServerHot),
                    "docker_vm" => Some(crate::wake::Condition::DockerVmHot),
                    _ => None,
                };
                if let Some(condition) = condition {
                    wake(home, now, condition, &id, &format!("{} used {:.0}% of one core for {:.0} s", rule.name, cpu.unwrap_or(0.0), now - since));
                }
            }
            state.insert(rule.key.into(), json!({
                "processes": members.len(), "cpu_percent_one_core": cpu, "rss_kib": rss_kib,
                "hot_since_ms": since.map(|since| since * 1000.0),
                "threshold": {"cpu_percent_one_core": rule.min_cpu_percent_one_core, "min_secs": rule.min_secs},
            }));
        }
        state.insert("automator_runners".into(), self.runners(home, now, processes)?);
        // A failed render probe marks the desktop source unhealthy without hiding the CPU rules.
        let render = self.render.drain(home, now).unwrap_or_else(|error| json!({"error": format!("{error:#}")}));
        let render_error = render["error"].as_str().map(|error| format!("render_server: {error}"));
        state.insert("probe_errors".into(), json!(render_error.into_iter().collect::<Vec<_>>()));
        state.insert("render_server".into(), render);
        write_json(&root(home).join("desktop.json"), &Value::Object(state))?;
        self.previous = Some((now, processes.iter().map(|p| ((p.pid, p.start), p.cpu_secs)).collect()));
        Ok(())
    }

    fn runners(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<Value> {
        let parents: HashSet<i32> = processes.iter().map(|p| p.ppid).collect();
        let runners: Vec<&Proc> = processes.iter().filter(|p| p.name == RUNNER).collect();
        let idle: Vec<&Proc> = runners
            .iter()
            .copied()
            .filter(|p| !parents.contains(&p.pid) && now as i64 - p.start >= RUNNER_IDLE_MIN_SECS)
            .collect();
        let since = if idle.len() >= RUNNER_LEAK_MIN_IDLE {
            Some(*self.leak_since.get_or_insert(now))
        } else {
            self.leak_since = None;
            None
        };
        let oldest_idle_s = idle.iter().map(|p| now as i64 - p.start).max();
        if let Some(since) = since {
            incident(home, &format!("automator_runner_leak:{}", since as i64), json!({
                "trigger": "automator_runner_leak", "idle": idle.len(), "total": runners.len(),
                "oldest_idle_s": oldest_idle_s, "rss_kib": idle.iter().map(|p| p.rss_kib).sum::<u64>(),
                "threshold": {"min_idle": RUNNER_LEAK_MIN_IDLE, "idle_min_secs": RUNNER_IDLE_MIN_SECS},
            }))?;
        }
        Ok(json!({"total": runners.len(), "idle": idle.len(), "oldest_idle_s": oldest_idle_s,
            "leak_since_ms": since.map(|since| since * 1000.0),
            "threshold": {"min_idle": RUNNER_LEAK_MIN_IDLE, "idle_min_secs": RUNNER_IDLE_MIN_SECS}}))
    }
}

/// Feeds a desktop incident to the one performance trigger; joined or deferred episodes are acknowledged.
pub(super) fn wake(home: &Path, now: f64, condition: crate::wake::Condition, id: &str, detail: &str) {
    if let Err(error) = crate::wake::trigger(home, now * 1000.0, condition, id, detail, true, json!({})) {
        crate::log(&format!("performance wake {id}: {error:#}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: i32, ppid: i32, start: i64, cpu_secs: f64, name: &str) -> Proc {
        Proc { pid, ppid, uid: 501, start, cpu_secs, rss_kib: 1024, name: name.into(), args: String::new() }
    }

    fn incidents(home: &Path) -> Vec<Value> {
        let directory = root(home).join("incidents");
        std::fs::read_dir(directory).map(|entries| entries
            .map(|entry| serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap())
            .collect()).unwrap_or_default()
    }

    #[test]
    fn sustained_windowserver_cpu_creates_one_incident_and_a_dip_resets_it() {
        let home = super::super::test_home("desktop-hot");
        let mut desktop = Desktop::default();
        // 90% of one core: 13.5 CPU seconds per 15 s interval.
        for tick in 0..=8 {
            let now = 1000.0 + tick as f64 * 15.0;
            desktop.observe(&home, now, &[proc(174, 1, 1, tick as f64 * 13.5, "WindowServer")]).unwrap();
        }
        let found = incidents(&home);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["trigger"], "windowserver_hot");
        assert!(found[0]["duration_s"].as_f64().unwrap() >= 120.0);
        // A quiet interval ends the episode; the state reports no hot window.
        desktop.observe(&home, 1150.0, &[proc(174, 1, 1, 108.5, "WindowServer")]).unwrap();
        let state: Value = serde_json::from_slice(&std::fs::read(root(&home).join("desktop.json")).unwrap()).unwrap();
        assert!(state["windowserver"]["hot_since_ms"].is_null());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_long_gap_or_a_new_process_never_counts_as_continuous_heat() {
        let home = super::super::test_home("desktop-gap");
        let mut desktop = Desktop::default();
        desktop.observe(&home, 1000.0, &[proc(41, 1, 1, 0.0, RULES[1].name)]).unwrap();
        // 1000% for 400 s, but sampled once: the gap exceeds MAX_GAP_SECS.
        desktop.observe(&home, 1400.0, &[proc(41, 1, 1, 4000.0, RULES[1].name)]).unwrap();
        // A restarted VM (new start time) has no baseline.
        desktop.observe(&home, 1415.0, &[proc(42, 1, 1400, 150.0, RULES[1].name)]).unwrap();
        assert!(incidents(&home).is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn idle_runner_pile_up_alerts_once_and_busy_or_young_runners_do_not_count() {
        let home = super::super::test_home("desktop-runners");
        let mut desktop = Desktop::default();
        let now = 100_000.0;
        let mut table: Vec<Proc> = (0..20).map(|i| proc(1000 + i, 1, 0, 0.0, RUNNER)).collect();
        table.push(proc(2000, 1, 0, 0.0, RUNNER));
        table.push(proc(2001, 2000, 0, 0.0, "zsh"));
        table.push(proc(2002, 1, now as i64 - 5, 0.0, RUNNER));
        desktop.observe(&home, now, &table).unwrap();
        desktop.observe(&home, now + 15.0, &table).unwrap();
        let found = incidents(&home);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["trigger"], "automator_runner_leak");
        assert_eq!(found[0]["idle"], 20);
        assert_eq!(found[0]["total"], 22);
        // Fewer idle runners than the threshold clears the episode.
        desktop.observe(&home, now + 30.0, &table[19..]).unwrap();
        let state: Value = serde_json::from_slice(&std::fs::read(root(&home).join("desktop.json")).unwrap()).unwrap();
        assert!(state["automator_runners"]["leak_since_ms"].is_null());
        std::fs::remove_dir_all(home).unwrap();
    }
}
