use crate::{
    monitor::{root, write_json},
    procs::Proc,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Clone, Serialize, Deserialize)]
struct Peak {
    pid: i32,
    start: i64,
    name: String,
    kind: String,
    first_seen_ms: f64,
    last_seen_ms: f64,
    samples: u64,
    peak_tree_cpu_percent_one_core: Option<f64>,
    peak_tree_rss_kib: u64,
    peak_cpu_at_ms: Option<f64>,
    peak_rss_at_ms: f64,
    largest_sample_gap_ms: f64,
    ended_observed_at_ms: Option<f64>,
    #[serde(default)]
    artifacts: Value,
    #[serde(default)]
    program: Value,
    #[serde(default)]
    peak_cpu_processes: Vec<Value>,
}

#[derive(Default)]
pub struct Tasks {
    peaks: Vec<Peak>,
    previous: Option<(f64, Vec<Proc>)>,
    loaded: bool,
    programs: crate::program::Programs,
}

fn kind(p: &Proc) -> Option<&'static str> {
    match p.name.as_str() {
        "xcodebuild" | "XCBBuildService" | "SWBBuildService" => Some("xcode"),
        "swift-frontend" | "swiftc" | "clang" | "clang++" | "ld" => Some("compiler"),
        "cargo" | "rustc" => Some("rust-build"),
        "bun" | "node" | "npm" | "pnpm" | "yarn" => {
            let words: Vec<_> = p.args.split_whitespace().collect();
            if words.iter().any(|s| ["install", "ci"].contains(s)) {
                Some("dependency-install")
            } else if words.iter().any(|s| *s == "test" || s.starts_with("test:")) {
                Some("test")
            } else if words
                .iter()
                .any(|s| *s == "build" || s.starts_with("build:"))
            {
                Some("build")
            } else {
                None
            }
        }
        _ => None,
    }
}

impl Tasks {
    pub fn observe(&mut self, home: &Path, now: f64, processes: &[Proc]) -> Result<()> {
        if !self.loaded {
            if let Some(saved) = super::read(&root(home).join("tasks.json")) {
                self.peaks = serde_json::from_value(saved["tasks"].clone()).unwrap_or_default();
            }
            self.loaded = true;
        }
        let table: HashMap<_, _> = processes.iter().map(|p| (p.pid, p)).collect();
        let selected: HashSet<_> = processes
            .iter()
            .filter(|p| kind(p).is_some())
            .map(|p| p.pid)
            .collect();
        let ancestor = |p: &Proc| {
            let mut pid = p.ppid;
            for _ in 0..64 {
                if selected.contains(&pid) {
                    return true;
                }
                match table.get(&pid) {
                    Some(parent) if parent.ppid != pid => pid = parent.ppid,
                    _ => break,
                }
            }
            false
        };
        let mut active = HashSet::new();
        for p in processes
            .iter()
            .filter(|p| selected.contains(&p.pid) && !ancestor(p))
        {
            let mut members = HashSet::from([p.pid]);
            for _ in 0..64 {
                let before = members.len();
                members.extend(
                    processes
                        .iter()
                        .filter(|child| members.contains(&child.ppid))
                        .map(|child| child.pid)
                        .collect::<Vec<_>>(),
                );
                if before == members.len() {
                    break;
                }
            }
            let group: Vec<_> = processes
                .iter()
                .filter(|child| members.contains(&child.pid))
                .collect();
            let rss = group.iter().map(|p| p.rss_kib).sum();
            let cpu = self.previous.as_ref().and_then(|(time, old)| {
                if now <= *time || now - time > 30.0 {
                    return None;
                }
                let old: HashMap<_, _> = old.iter().map(|p| (p.pid, p)).collect();
                let mut total = 0.0;
                for child in &group {
                    let before = old.get(&child.pid)?;
                    if (before.start - child.start).abs() > 2 || child.cpu_secs < before.cpu_secs {
                        return None;
                    }
                    total += (child.cpu_secs - before.cpu_secs) / (now - time) * 100.0;
                }
                Some(total)
            });
            let index = self.peaks.iter().position(|peak| {
                peak.pid == p.pid
                    && (peak.start - p.start).abs() <= 2
                    && peak.ended_observed_at_ms.is_none()
            });
            let index = index.unwrap_or_else(|| {
                self.peaks.push(Peak {
                    pid: p.pid,
                    start: p.start,
                    name: p.name.clone(),
                    kind: kind(p).unwrap().into(),
                    first_seen_ms: now * 1000.0,
                    last_seen_ms: now * 1000.0,
                    samples: 0,
                    peak_tree_cpu_percent_one_core: None,
                    peak_tree_rss_kib: 0,
                    peak_cpu_at_ms: None,
                    peak_rss_at_ms: now * 1000.0,
                    largest_sample_gap_ms: 0.0,
                    ended_observed_at_ms: None,
                    artifacts: if kind(p) == Some("xcode") {
                        super::xcode::artifacts(&p.args)
                    } else {
                        Value::Null
                    },
                    program: self.programs.capture(p, processes, now),
                    peak_cpu_processes: Vec::new(),
                });
                self.peaks.len() - 1
            });
            active.insert(index);
            let peak = &mut self.peaks[index];
            if peak.program.is_null() {
                peak.program = self.programs.capture(p, processes, now);
            }
            peak.largest_sample_gap_ms = peak
                .largest_sample_gap_ms
                .max(now * 1000.0 - peak.last_seen_ms);
            peak.last_seen_ms = now * 1000.0;
            peak.samples += 1;
            if rss >= peak.peak_tree_rss_kib {
                peak.peak_tree_rss_kib = rss;
                peak.peak_rss_at_ms = now * 1000.0;
            }
            if let Some(cpu) = cpu {
                if peak
                    .peak_tree_cpu_percent_one_core
                    .is_none_or(|value| cpu > value)
                {
                    peak.peak_tree_cpu_percent_one_core = Some(cpu);
                    peak.peak_cpu_at_ms = Some(now * 1000.0);
                    let old: HashMap<_, _> = self.previous.as_ref().unwrap().1.iter().map(|p|(p.pid,p)).collect();
                    let elapsed = now - self.previous.as_ref().unwrap().0;
                    let mut contributors: Vec<_> = group.iter().map(|p| (*p,
                        (p.cpu_secs - old[&p.pid].cpu_secs) / elapsed * 100.0)).collect();
                    contributors.sort_by(|a,b| b.1.total_cmp(&a.1));
                    peak.peak_cpu_processes = contributors.into_iter().take(8).map(|(p,cpu)|
                        json!({"pid":p.pid,"start":p.start,"name":p.name,"rss_kib":p.rss_kib,
                            "cpu_percent_one_core":cpu,"program":self.programs.capture(p, processes, now)})).collect();
                }
            }
        }
        for (index, peak) in self.peaks.iter_mut().enumerate() {
            if !active.contains(&index) && peak.ended_observed_at_ms.is_none() {
                peak.ended_observed_at_ms = Some(now * 1000.0);
                let id = format!("{}-{}", peak.pid, peak.start);
                write_json(
                    &root(home).join("task-records").join(format!("{id}.json")),
                    peak,
                )?;
            }
        }
        self.peaks.retain(|peak| {
            peak.ended_observed_at_ms.is_none() || now * 1000.0 - peak.last_seen_ms < 3_600_000.0
        });
        let value: Value = json!({"at_ms":now*1000.0,"nominal_interval_ms":15000,
            "semantics":"Sampled process-tree peaks; RSS sum includes shared pages; short tasks and between-sample spikes may be missed; CPU null when tree continuity is incomplete",
            "tasks":self.peaks});
        write_json(&root(home).join("tasks.json"), &value)?;
        self.previous = Some((now, processes.to_vec()));
        Ok(())
    }
}

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tests;
