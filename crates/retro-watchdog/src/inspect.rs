//! Read-only host diagnosis. CPU is a measured delta in percent of one core; arguments stay private.
use crate::{inventory, procs};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Clone, Serialize)]
pub struct Usage {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    pub cpu_percent: Option<f64>,
    pub rss_kib: Option<u64>,
}

pub fn capture(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn raw(program: &str, args: &[&str]) -> serde_json::Value {
    match capture(program, args) {
        Ok(text) => json!({"text": text}),
        Err(error) => json!({"error": error.to_string()}),
    }
}

fn seconds(args: &[String]) -> Result<u64> {
    let value = match args {
        [] => 10,
        [flag, value] if flag == "--seconds" => {
            value.parse().context("seconds must be an integer")?
        }
        _ => bail!("usage: cpu-watchdog inspect [--seconds 1..30]"),
    };
    if !(1..=30).contains(&value) {
        bail!("seconds must be in 1..30");
    }
    Ok(value)
}

fn measured(
    before: &[procs::Proc],
    after: &[procs::Proc],
    elapsed: f64,
    rss: &HashMap<i32, u64>,
) -> Vec<Usage> {
    let previous: HashMap<_, _> = before.iter().map(|p| (p.pid, p)).collect();
    after
        .iter()
        .map(|p| {
            let cpu = previous
                .get(&p.pid)
                .filter(|old| (old.start - p.start).abs() <= 2)
                .and_then(|old| {
                    let delta = p.cpu_secs - old.cpu_secs;
                    (elapsed > 0.0 && delta >= 0.0).then_some(delta / elapsed * 100.0)
                });
            Usage {
                pid: p.pid,
                ppid: p.ppid,
                name: p.name.clone(),
                cpu_percent: cpu,
                rss_kib: rss.get(&p.pid).copied(),
            }
        })
        .collect()
}

fn family(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("orca") {
        "Orca"
    } else if lower.contains("figma") {
        "Figma"
    } else if lower.contains("docker") || lower.contains("virtualmachine") {
        "Docker/VM"
    } else if lower.contains("sim") || lower == "marscrm" || lower == "previewshell" {
        "Simulator"
    } else if lower.contains("chrome") || lower.contains("chromium") {
        "Chrome"
    } else if lower.contains("watchdog") || lower == "tm-scheduler" {
        "Watchdogs"
    } else if lower.contains("claude") || lower.contains("codex") {
        "Agents"
    } else {
        "Other (includes system)"
    }
}

pub fn run(home: &Path, args: Vec<String>) -> Result<()> {
    let delay = seconds(&args)?;
    let start = Instant::now();
    let epoch = crate::unix_now();
    let clock = || epoch + start.elapsed().as_secs_f64();
    let (begin, before) = procs::snapshot(clock)?;
    std::thread::sleep(Duration::from_secs(delay));
    let (at, after) = procs::snapshot(clock)?;
    let elapsed = at - begin;
    let mut errors = Vec::new();
    let rss: HashMap<i32, u64> = match capture("/bin/ps", &["-Axo", "pid=,rss="]) {
        Ok(text) => text
            .lines()
            .filter_map(|line| {
                let mut cols = line.split_whitespace();
                Some((cols.next()?.parse().ok()?, cols.next()?.parse().ok()?))
            })
            .collect(),
        Err(error) => {
            errors.push(error.to_string());
            HashMap::new()
        }
    };
    let mut usage = measured(&before, &after, elapsed, &rss);
    let jobs = inventory::read(home, &usage, &mut errors);
    let mut groups = std::collections::BTreeMap::new();
    for p in &usage {
        let entry = groups
            .entry(family(&p.name))
            .or_insert((0usize, 0.0f64, 0u64, 0usize));
        entry.0 += 1;
        entry.1 += p.cpu_percent.unwrap_or(0.0);
        entry.2 += p.rss_kib.unwrap_or(0);
        entry.3 += usize::from(p.cpu_percent.is_none());
    }
    usage.sort_by(|a, b| {
        b.cpu_percent
            .unwrap_or(-1.0)
            .total_cmp(&a.cpu_percent.unwrap_or(-1.0))
    });
    let top_cpu: Vec<_> = usage.iter().take(25).cloned().collect();
    usage.sort_by_key(|p| std::cmp::Reverse(p.rss_kib.unwrap_or(0)));
    let top_memory: Vec<_> = usage.iter().take(15).cloned().collect();
    let status_path = home.join(".local/state/cpu-watchdog/status.json");
    let watchdog = match std::fs::read_to_string(&status_path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(mut state) => {
                if let Some(rows) = state["watching"].as_array_mut() {
                    for row in rows {
                        if let Some(obj) = row.as_object_mut() {
                            obj.remove("args");
                        }
                    }
                }
                state
            }
            Err(error) => json!({"error": error.to_string()}),
        },
        Err(error) => json!({"error": error.to_string()}),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "host-performance.v1", "at_unix": at, "sample_seconds": elapsed,
            "cpu_unit": "percent_of_one_core", "logical_cpus": raw("/usr/sbin/sysctl", &["-n", "hw.logicalcpu"]),
            "process_count": usage.len(), "top_cpu": top_cpu, "top_memory": top_memory,
            "groups_by_executable": groups.into_iter().map(|(name, (count, cpu, rss, missing))|
                json!({"name":name,"process_count":count,"cpu_percent":cpu,"rss_kib":rss,"unmeasured_processes":missing})).collect::<Vec<_>>(),
            "watchdogs": jobs, "cpu_watchdog_status": watchdog,
            "memory": raw("/usr/bin/vm_stat", &[]), "swap": raw("/usr/sbin/sysctl", &["vm.swapusage"]),
            "disk": raw("/bin/df", &["-k", "/System/Volumes/Data"]), "errors": errors,
            "limits": ["Short-lived processes can be missed; use spawn or coalition profiling for churn.",
                "RSS sums include shared pages and are not unique physical memory.",
                "Groups use executable names, not ownership; CPU is not a whole-host percentage.",
                "Loaded jobs are not necessarily healthy; absent PID does not imply failure for periodic jobs."]
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn proc(start: i64, cpu_secs: f64) -> procs::Proc {
        procs::Proc {
            pid: 1,
            ppid: 0,
            uid: 501,
            start,
            cpu_secs,
            rss_kib: 0,
            name: "node".into(),
            args: "secret".into(),
        }
    }
    #[test]
    fn deltas_reject_pid_reuse_and_keep_arguments_private() {
        let before = vec![proc(100, 4.0)];
        let rows = measured(&before, &[proc(100, 14.0)], 10.0, &HashMap::new());
        assert_eq!(rows[0].cpu_percent, Some(100.0));
        assert!(!serde_json::to_string(&rows).unwrap().contains("secret"));
        assert_eq!(
            measured(&before, &[proc(200, 14.0)], 10.0, &HashMap::new())[0].cpu_percent,
            None
        );
        assert_eq!(
            measured(&before, &[proc(100, 1.0)], 10.0, &HashMap::new())[0].cpu_percent,
            None
        );
    }
    #[test]
    fn sampling_is_bounded() {
        assert_eq!(seconds(&[]).unwrap(), 10);
        for value in ["0", "31", "NaN"] {
            assert!(seconds(&["--seconds".into(), value.into()]).is_err());
        }
    }
}
