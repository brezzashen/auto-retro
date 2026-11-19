use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

fn memory_bytes(display: &str) -> Option<f64> {
    let value = display.split('/').next()?.trim();
    let split = value.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let number = value[..split].parse::<f64>().ok()?;
    let unit = value[split..].trim();
    let multiplier = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0f64.powi(2),
        "GiB" => 1024.0f64.powi(3),
        "TiB" => 1024.0f64.powi(4),
        "kB" => 1000.0,
        "MB" => 1e6,
        "GB" => 1e9,
        _ => return None,
    };
    (number.is_finite() && number >= 0.0).then_some(number * multiplier)
}

pub fn observe(home: &Path, now: f64, containers: &[Value]) -> Result<()> {
    let path = root(home).join("container-peaks.json");
    let old = super::read(&path).unwrap_or(json!({"containers":[]}));
    let mut peaks = old["containers"].as_array().cloned().unwrap_or_default();
    for row in containers {
        let Some(id) = row["id"].as_str() else {
            continue;
        };
        anyhow::ensure!(
            id.bytes().all(|b| b.is_ascii_hexdigit()) && !id.is_empty(),
            "invalid container id"
        );
        let index = peaks
            .iter()
            .position(|peak| peak["id"] == id && peak["ended_observed_at_ms"].is_null());
        let index = index.unwrap_or_else(|| {
            peaks.push(
                json!({"id":id,"name":row["name"],"first_seen_ms":now*1000.0,
                "last_seen_ms":now*1000.0,"samples":0,"peak_cpu_percent_one_core":null,
                "peak_memory_usage_bytes":null,"ended_observed_at_ms":null}),
            );
            peaks.len() - 1
        });
        let peak = &mut peaks[index];
        peak["last_seen_ms"] = json!(now * 1000.0);
        peak["samples"] = json!(peak["samples"].as_u64().unwrap_or(0) + 1);
        if let Some(cpu) = row["cpu_percent_one_core_display"]
            .as_str()
            .and_then(|s| s.trim_end_matches('%').parse::<f64>().ok())
        {
            if cpu.is_finite()
                && cpu >= 0.0
                && peak["peak_cpu_percent_one_core"]
                    .as_f64()
                    .is_none_or(|old| cpu > old)
            {
                peak["peak_cpu_percent_one_core"] = json!(cpu);
                peak["peak_cpu_at_ms"] = json!(now * 1000.0);
            }
        }
        if let Some(memory) = row["memory_usage_display"].as_str().and_then(memory_bytes) {
            if peak["peak_memory_usage_bytes"]
                .as_f64()
                .is_none_or(|old| memory > old)
            {
                peak["peak_memory_usage_bytes"] = json!(memory);
                peak["peak_memory_at_ms"] = json!(now * 1000.0);
            }
        }
    }
    for peak in &mut peaks {
        if peak["ended_observed_at_ms"].is_null()
            && !containers.iter().any(|row| row["id"] == peak["id"])
        {
            peak["ended_observed_at_ms"] = json!(now * 1000.0);
            let id = peak["id"].as_str().unwrap();
            let first = peak["first_seen_ms"].as_f64().unwrap_or(now * 1000.0);
            write_json(
                &root(home)
                    .join("container-records")
                    .join(format!("{id}-{first:.0}.json")),
                peak,
            )?;
        }
    }
    peaks.retain(|peak| {
        peak["ended_observed_at_ms"].is_null()
            || now * 1000.0 - peak["last_seen_ms"].as_f64().unwrap_or(0.0) < 3_600_000.0
    });
    write_json(
        &path,
        &json!({"at_ms":now*1000.0,"containers":peaks,
        "semantics":"Sampled about once a minute; Docker CLI memory usage is not RSS; short containers and between-sample peaks may be missed; end means absent from a successful stats snapshot"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn container_peaks_preserve_units_restart_and_completion() {
        let home = super::super::test_home("containers");
        let row = json!({"id":"ab1234","name":"test","cpu_percent_one_core_display":"200.50%","memory_usage_display":"1.5GiB / 12GiB"});
        observe(&home, 100.0, &[row]).unwrap();
        let state = super::super::read(&root(&home).join("container-peaks.json")).unwrap();
        assert_eq!(
            state["containers"][0]["peak_memory_usage_bytes"],
            1.5 * 1024.0f64.powi(3)
        );
        observe(&home, 160.0, &[]).unwrap();
        assert_eq!(
            std::fs::read_dir(root(&home).join("container-records"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(memory_bytes("12MB / 12GB"), Some(12e6));
        assert!(memory_bytes("bad").is_none());
        std::fs::remove_dir_all(home).unwrap();
    }
}
