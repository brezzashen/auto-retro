//! Process-tree groups behind a memory episode (perf retro 1794856006950, 2026-11-17).
//! The twelve largest single footprints named WindowServer at 4.1 GiB as the largest holder while
//! six booted simulators held about 58 GiB in 1,180 processes of under 1 GiB each and 38 agent
//! session trees held 18 GiB. A class made of many small processes never reaches the per-process
//! list, so every footprint is also summed under the nearest ancestor that roots a known tree.
use super::memory_holders::Holder;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Group name and the `comm` basename of the process that roots one tree of the group. An Android
/// emulator is one qemu process (4.8-8.4 GiB each; four held 30 GiB on 2026-11-18, perf retro
/// 1794985103952) that the twelve-row list shows as separate rows and no group counted.
const ROOTS: [(&str, &str); 3] =
    [("simulators", "launchd_sim"), ("agent sessions", "claude"), ("android emulators", "qemu-system-aarch64-headless")];
const MAX_DEPTH: usize = 64;
const GIB: f64 = 1_073_741_824.0;

/// `ps -axo pid=,ppid=,comm=` rows as pid -> (parent pid, basename of the executable).
pub fn parse_tree(text: &str) -> HashMap<i64, (i64, String)> {
    text.lines().filter_map(|line| {
        let mut parts = line.split_whitespace();
        let (pid, parent) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
        let command = parts.collect::<Vec<_>>().join(" ");
        Some((pid, (parent, command.rsplit('/').next().unwrap_or_default().to_owned())))
    }).collect()
}

/// Index into ROOTS and the root pid of the nearest ancestor (the process itself included).
fn root_of(pid: i64, tree: &HashMap<i64, (i64, String)>) -> Option<(usize, i64)> {
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        let (parent, name) = tree.get(&current)?;
        if let Some(group) = ROOTS.iter().position(|(_, root)| root == name) { return Some((group, current)); }
        if *parent <= 1 || *parent == current { return None; }
        current = *parent;
    }
    None
}

fn gib(bytes: f64) -> f64 { (bytes / GIB * 100.0).round() / 100.0 }

/// Every footprint summed per group, largest first, and the whole-host total.
pub fn summarize(holders: &[Holder], tree: &HashMap<i64, (i64, String)>) -> (Vec<Value>, Value) {
    let mut sums = vec![(0.0, 0.0, 0usize, HashSet::new()); ROOTS.len()];
    for holder in holders {
        if let Some((group, root)) = root_of(holder.pid, tree) {
            let sum = &mut sums[group];
            (sum.0, sum.1, sum.2) = (sum.0 + holder.footprint, sum.1 + holder.compressed, sum.2 + 1);
            sum.3.insert(root);
        }
    }
    let mut groups: Vec<_> = sums.iter().zip(ROOTS).filter(|(sum, _)| sum.2 > 0).map(|(sum, (name, _))| (sum.0, json!({"name": name,
        "trees": sum.3.len(), "processes": sum.2, "footprint_gib": gib(sum.0), "compressed_gib": gib(sum.1)}))).collect();
    groups.sort_by(|a, b| b.0.total_cmp(&a.0));
    let total = json!({"processes": holders.len(), "footprint_gib": gib(holders.iter().map(|h| h.footprint).sum()),
        "compressed_gib": gib(holders.iter().map(|h| h.compressed).sum())});
    (groups.into_iter().map(|(_, group)| group).collect(), total)
}

#[cfg(test)]
#[path = "memory_groups_tests.rs"]
mod tests;
