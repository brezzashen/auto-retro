//! Who holds the memory when the memory guard fires (perf retro 1794671950116, 2026-11-15).
//! A `memory_exhausted` incident carried CPU frames and RSS only, so its cause comparison named the
//! busiest CPU class (rustc at 24%) while fseventsd held 38 GiB of physical footprint, 30 GiB of it
//! compressed: RSS showed 9.5 GiB and its CPU share was 5%. The same daemon had 36.4 GiB on
//! 2026-11-03 and that retro saw only its CPU. `top` reports the footprint (`ri_phys_footprint`)
//! and the compressed part for every process, root daemons included, without privileges.
use serde_json::{json, Value};
use std::path::Path;

const TOP: &str = "/usr/bin/top";
const PS: &str = "/bin/ps";
/// Rows kept in `holders`; every row still counts toward `groups` and `total`.
const KEEP: usize = 12;
/// `top -l 1` took 3.8 s on the exhausted host of 2026-11-15; the guard runs it once per relief.
const TIMEOUT_SECS: u64 = 20;
const GIB: f64 = 1_073_741_824.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Holder { pub pid: i64, pub name: String, pub footprint: f64, pub compressed: f64 }

/// A `top` size column: "39G", "4430M", "3264K", "0B", with "+" or "-" appended when it changed.
fn bytes(token: &str) -> Option<f64> {
    let token = token.trim_end_matches(['+', '-']);
    let (number, unit) = token.split_at(token.len().checked_sub(1)?);
    let scale = match unit { "B" => 1.0, "K" => 1024.0, "M" => 1_048_576.0, "G" => GIB, "T" => 1024.0 * GIB, _ => return None };
    Some(number.parse::<f64>().ok()? * scale)
}

/// The rows below the "PID MEM CMPRS COMMAND" header of `top -stats pid,mem,cmprs,command`.
pub fn parse(text: &str) -> Vec<Holder> {
    text.lines().skip_while(|line| !line.trim_start().starts_with("PID")).skip(1).filter_map(|line| {
        let mut parts = line.split_whitespace();
        let pid = parts.next()?.trim_end_matches('*').parse().ok()?;
        let (footprint, compressed) = (bytes(parts.next()?)?, bytes(parts.next()?)?);
        let name = parts.collect::<Vec<_>>().join(" ");
        (!name.is_empty()).then_some(Holder { pid, name, footprint, compressed })
    }).collect()
}

fn gib(bytes: f64) -> f64 { (bytes / GIB * 100.0).round() / 100.0 }

/// The largest footprints, largest first, with the process-tree sums of `memory_groups.rs` and the
/// host total. A failed probe is recorded, never an empty healthy list; without the process table
/// the groups are left out and the rows stand alone.
pub fn capture(home: &Path) -> Value {
    let stats = ["-l", "1", "-o", "mem", "-stats", "pid,mem,cmprs,command"];
    let text = match crate::telemetry::command::capture(home, TOP, &stats, TIMEOUT_SECS) {
        Ok(text) => text,
        Err(error) => return json!({"error": format!("{error:#}"), "holders": []}),
    };
    let rows = parse(&text);
    let mut captured = json!({"source": "top -o mem (physical footprint, compressed part)", "holders": rows.iter().take(KEEP)
        .map(|h| json!({"pid": h.pid, "name": h.name, "footprint_gib": gib(h.footprint), "compressed_gib": gib(h.compressed)})).collect::<Vec<_>>()});
    match crate::telemetry::command::capture(home, PS, &["-axo", "pid=,ppid=,comm="], TIMEOUT_SECS) {
        Ok(table) => {
            let (groups, total) = super::memory_groups::summarize(&rows, &super::memory_groups::parse_tree(&table));
            (captured["groups"], captured["total"]) = (json!(groups), total);
        }
        Err(error) => captured["groups_error"] = json!(format!("{error:#}")),
    }
    captured
}

/// The largest single footprint or, when larger, the largest process-tree group: (name, GiB, detail).
pub fn largest_holder(captured: &Value) -> Option<(String, f64, Option<String>)> {
    let row = captured["holders"].as_array().and_then(|rows| rows.first())
        .and_then(|first| Some((first["name"].as_str()?.to_owned(), first["footprint_gib"].as_f64()?, None)));
    let group = captured["groups"].as_array().and_then(|groups| groups.first()).and_then(|first| Some((first["name"].as_str()?.to_owned(),
        first["footprint_gib"].as_f64()?, Some(format!("{} trees, {} processes", first["trees"], first["processes"])))));
    match (row, group) {
        (Some(row), Some(group)) => Some(if group.1 > row.1 { group } else { row }),
        (row, group) => row.or(group),
    }
}

/// "fseventsd 38.3 GiB" or "simulators 58.2 GiB (6 trees, 1180 processes)" for the incident detail
/// line; None when the probe returned no row.
pub fn largest(captured: &Value) -> Option<String> {
    let (name, footprint, detail) = largest_holder(captured)?;
    Some(match detail {
        Some(detail) => format!("{name} {footprint:.1} GiB ({detail})"),
        None => format!("{name} {footprint:.1} GiB"),
    })
}

#[cfg(test)]
#[path = "memory_holders_tests.rs"]
mod tests;
