use super::command::capture;
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

fn parse(text: &str) -> Value {
    let mut result = serde_json::Map::new();
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("WATCHDOG_SECTION ") {
            section = name;
            continue;
        }
        match section {
            "boot_id" => {
                result.insert("boot_id".into(), json!(line.trim()));
            }
            "psi_memory" | "psi_cpu" | "psi_io" => {
                let mut fields = line.split_whitespace();
                if let Some(kind) = fields.next() {
                    let mut values = serde_json::Map::new();
                    for item in fields {
                        if let Some((key, value)) = item.split_once('=') {
                            if let Ok(number) = value.parse::<f64>() {
                                if number.is_finite() && number >= 0.0 {
                                    values.insert(key.into(), json!(number));
                                }
                            }
                        }
                    }
                    result.entry(section).or_insert(json!({}))[kind] = Value::Object(values);
                }
            }
            "meminfo" | "vmstat" => {
                let mut fields = line.split_whitespace();
                if let (Some(key), Some(value)) = (fields.next(), fields.next()) {
                    if section == "vmstat"
                        && !["oom_kill", "pswpin", "pswpout", "pgmajfault"].contains(&key)
                        && !key.starts_with("allocstall_")
                    {
                        continue;
                    }
                    if let Ok(number) = value.parse::<u64>() {
                        result.entry(section).or_insert(json!({}))[key.trim_end_matches(':')] =
                            json!(number);
                    }
                }
            }
            _ => {}
        }
    }
    Value::Object(result)
}

fn deltas(previous: &Value, current: &Value, elapsed: f64) -> Value {
    if elapsed <= 0.0
        || previous["boot_id"] != current["boot_id"]
        || !current["boot_id"].is_string()
    {
        return Value::Null;
    }
    let mut result = serde_json::Map::new();
    for key in [
        "oom_kill",
        "pswpin",
        "pswpout",
        "pgmajfault",
        "allocstall_dma",
        "allocstall_dma32",
        "allocstall_normal",
        "allocstall_movable",
    ] {
        if let (Some(old), Some(new)) = (
            previous["vmstat"][key].as_u64(),
            current["vmstat"][key].as_u64(),
        ) {
            if new >= old {
                result.insert(key.into(), json!(new - old));
            }
        }
    }
    for kind in ["some", "full"] {
        if let (Some(old), Some(new)) = (
            previous["psi_memory"][kind]["total"].as_f64(),
            current["psi_memory"][kind]["total"].as_f64(),
        ) {
            if new >= old {
                result.insert(
                    format!("memory_{kind}_stall_percent"),
                    json!((new - old) / elapsed / 10000.0),
                );
            }
        }
    }
    json!({"elapsed_seconds":elapsed,"values":result})
}

pub fn collect(home: &Path) -> Result<()> {
    let docker_path = home.join(".docker/bin/docker");
    let docker = docker_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Docker binary path is not UTF-8"))?;
    let started = crate::unix_now();
    // Read-only /proc views of the local guest through an existing running container.
    let script="set -e; echo WATCHDOG_SECTION boot_id; cat /proc/sys/kernel/random/boot_id; echo WATCHDOG_SECTION psi_memory; cat /proc/pressure/memory; echo WATCHDOG_SECTION psi_cpu; cat /proc/pressure/cpu; echo WATCHDOG_SECTION psi_io; cat /proc/pressure/io; echo WATCHDOG_SECTION meminfo; cat /proc/meminfo; echo WATCHDOG_SECTION vmstat; cat /proc/vmstat";
    let sampled = capture(
        home,
        docker,
        &[
            "--context",
            "desktop-linux",
            "exec",
            "mars-crm-postgres",
            "sh",
            "-c",
            script,
        ],
        5,
    );
    let finished = crate::unix_now();
    let path = root(home).join("guest.json");
    let previous = super::read(&path);
    let (sample, error) = match sampled {
        Ok(text) => {
            let sample = parse(&text);
            let valid = sample["boot_id"].as_str().is_some_and(|id| !id.is_empty())
                && sample["psi_memory"]["full"]["avg10"].is_number()
                && sample["vmstat"]["oom_kill"].is_number();
            (
                sample,
                (!valid).then(|| "Guest /proc probe returned incomplete metrics".into()),
            )
        }
        Err(error) => (Value::Null, Some(format!("{error:#}"))),
    };
    let delta = previous.as_ref().map(|old| {
        deltas(
            &old["sample"],
            &sample,
            finished
                - old["sample_finished_at_ms"]
                    .as_f64()
                    .unwrap_or(finished * 1000.0)
                    / 1000.0,
        )
    });
    let stats = capture(
        home,
        docker,
        &[
            "--context",
            "desktop-linux",
            "stats",
            "--no-stream",
            "--format",
            "{{json .}}",
        ],
        5,
    );
    let (containers,stats_error)=match stats {
        Ok(text)=>(text.lines().filter_map(|line|serde_json::from_str::<Value>(line).ok())
            .map(|row|json!({"id":row["ID"],"name":row["Name"],"cpu_percent_one_core_display":row["CPUPerc"],
                "memory_usage_display":row["MemUsage"],"pids_display":row["PIDs"]})).collect::<Vec<_>>(),None),
        Err(error)=>(Vec::new(),Some(format!("{error:#}"))),
    };
    if stats_error.is_none() {
        super::containers::observe(home, crate::unix_now(), &containers)?;
    }
    write_json(
        &path,
        &json!({"at_ms":crate::unix_now()*1000.0,"sample_started_at_ms":started*1000.0,
        "sample_finished_at_ms":finished*1000.0,"context":"desktop-linux","proc_container":"mars-crm-postgres",
        "error":error,"sample":sample,"counter_delta":delta,"containers":containers,"stats_error":stats_error,
        "semantics":"PSI avg10/60/300 are rolling percentages; total is cumulative microseconds; meminfo is KiB; swap counters are pages; Docker memory display is not RSS; no memory alert threshold configured"}),
    )?;
    let history = root(home)
        .join("guest-samples")
        .join(format!("{:.0}.json", finished * 1000.0));
    write_json(&history, &super::read(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn psi_units_counter_deltas_and_guest_restart() {
        let old=parse("WATCHDOG_SECTION boot_id\na\nWATCHDOG_SECTION psi_memory\nfull avg10=3.0 avg60=2 avg300=1 total=100\nWATCHDOG_SECTION vmstat\noom_kill 10\npswpout 20\n");
        let new=parse("WATCHDOG_SECTION boot_id\na\nWATCHDOG_SECTION psi_memory\nfull avg10=4 avg60=2 avg300=1 total=1000100\nWATCHDOG_SECTION vmstat\noom_kill 11\npswpout 30\n");
        assert_eq!(new["psi_memory"]["full"]["avg10"], 4.0);
        let delta = deltas(&old, &new, 10.0);
        assert_eq!(delta["values"]["memory_full_stall_percent"], 10.0);
        assert_eq!(delta["values"]["oom_kill"], 1);
        assert!(deltas(&old, &json!({"boot_id":"b"}), 10.0).is_null());
        assert!(deltas(&new, &old, 10.0)["values"].get("oom_kill").is_none());
    }
}
