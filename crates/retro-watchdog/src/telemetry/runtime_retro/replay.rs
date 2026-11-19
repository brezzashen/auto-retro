//! `cpu-watchdog runtime-breakdown <incident-id> | --from-ms <ms> --to-ms <ms> [--top <n>]`: replays
//! every transcript over a past window with today's tool labels and prints the same breakdown an
//! incident carries, so a label or bucket change is measured on the window it was found in
//! (runtime retro 811163801920064345384).
use super::super::conversation_retro::{Layout, SOURCES};
use super::parse::{read_lines, timestamp_ms, walk, Cursor, Tally};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;

/// Window totals over `[from_ms, to_ms]`: every transcript written since `from_ms`, from its start,
/// counting only lines stamped inside the window.
pub(in crate::telemetry) fn replay(home: &Path, from_ms: f64, to_ms: f64) -> Tally {
    let mut tally = Tally::default();
    for (alias, dir, layout) in SOURCES {
        walk(&home.join(dir), 3, &mut |path, _created, modified, len| {
            if modified < from_ms { return; }
            let mut cursor = Cursor::default();
            cursor.alias = alias.to_string();
            cursor.sub = path.components().any(|c| c.as_os_str() == "subagents");
            cursor.codex = layout == Layout::Codex;
            let mut offset = 0;
            while offset < len {
                let before = offset;
                read_lines(path, len, &mut offset, &mut |line| {
                    let at = line["timestamp"].as_str().and_then(timestamp_ms).unwrap_or(f64::NAN);
                    if at >= from_ms && at <= to_ms { cursor.feed(line, &mut tally); }
                });
                if offset == before { break; }
            }
        }, &|name| match layout {
            Layout::Codex => name.starts_with("rollout-") && name.ends_with(".jsonl"),
            _ => name.ends_with(".jsonl") && !name.starts_with("ag-boot"),
        });
    }
    tally
}

pub(in crate::telemetry) fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str);
    let number = |name: &str| flag(name).map(|v| v.parse::<f64>().with_context(|| format!("{name} needs a number"))).transpose();
    let (from, to) = match (number("--from-ms")?, number("--to-ms")?, args.first().filter(|a| !a.starts_with('-'))) {
        (Some(from), Some(to), _) => (from, to),
        (_, _, Some(id)) => {
            let path = crate::monitor::root(home).join("incidents").join(format!("{id}.json"));
            let incident: Value = serde_json::from_str(&std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?)?;
            match (incident["window"]["from_ms"].as_f64(), incident["window"]["to_ms"].as_f64()) {
                (Some(from), Some(to)) => (from, to),
                _ => bail!("{} has no window.from_ms/to_ms", path.display()),
            }
        }
        _ => bail!("usage: cpu-watchdog runtime-breakdown <incident-id> | --from-ms <ms> --to-ms <ms> [--top <n>]"),
    };
    let top = number("--top")?.map_or(30, |n| n as usize);
    let tally = replay(home, from, to);
    let mut out = super::breakdown_of(&tally, top);
    out["window"] = serde_json::json!({"from_ms": from, "to_ms": to});
    out["model_calls"] = serde_json::json!(tally.model_calls());
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
