//! Discover watchdog jobs from installed plists; never run their potentially mutating commands.
use crate::inspect::{capture, Usage};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

fn relevant(label: &str) -> bool {
    [
        "watch",
        "cpu-build",
        "sim-lease-trim",
        "tm-scheduler",
        "storage-cap",
        "ports-recovery",
    ]
    .iter()
    .any(|word| label.contains(word))
}

fn descendants(pid: i32, usage: &[Usage]) -> Vec<&Usage> {
    let mut found = HashSet::from([pid]);
    loop {
        let old = found.len();
        for row in usage {
            if found.contains(&row.ppid) {
                found.insert(row.pid);
            }
        }
        if found.len() == old {
            break;
        }
    }
    usage
        .iter()
        .filter(|row| found.contains(&row.pid))
        .collect()
}

pub fn read(home: &Path, usage: &[Usage], errors: &mut Vec<String>) -> Vec<Value> {
    let listing = match capture("/bin/launchctl", &["list"]) {
        Ok(text) => text,
        Err(error) => {
            errors.push(error.to_string());
            String::new()
        }
    };
    let loaded: HashMap<_, _> = listing
        .lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<_> = line.split_whitespace().collect();
            (cols.len() == 3).then(|| {
                (
                    cols[2].to_string(),
                    (cols[0].parse::<i32>().ok(), cols[1].to_string()),
                )
            })
        })
        .collect();
    let mut jobs = Vec::new();
    for dir in [
        home.join("Library/LaunchAgents"),
        "/Library/LaunchAgents".into(),
        "/Library/LaunchDaemons".into(),
    ] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                errors.push(format!("{}: {error}", dir.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("plist") {
                continue;
            }
            let filename = path.file_stem().and_then(|x| x.to_str()).unwrap_or("");
            if !relevant(filename) {
                continue;
            }
            let parsed = capture(
                "/usr/bin/plutil",
                &["-convert", "json", "-o", "-", &path.to_string_lossy()],
            )
            .and_then(|text| Ok(serde_json::from_str::<Value>(&text)?));
            let data = match parsed {
                Ok(data) => data,
                Err(error) => {
                    errors.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            let label = data["Label"].as_str().unwrap_or(filename);
            // launchctl list covers this user's bootstrap domain only, not system daemons.
            let user_domain = dir == home.join("Library/LaunchAgents");
            let state = user_domain.then(|| loaded.get(label)).flatten();
            let pid = state.and_then(|row| row.0);
            let members = pid.map(|pid| descendants(pid, usage));
            let args = data["ProgramArguments"].as_array();
            let entrypoints: Vec<_> = args
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|arg| arg.starts_with('/') && Path::new(arg).is_file())
                .collect();
            jobs.push(json!({
                "label": label, "plist": path, "entrypoints": entrypoints,
                "loaded_in_user_domain": if user_domain { Some(state.is_some()) } else { None },
                "pid": pid, "last_exit": state.map(|s| &s.1),
                "start_interval_seconds": data.get("StartInterval"), "keep_alive": data.get("KeepAlive"),
                "stdout": data.get("StandardOutPath"), "stderr": data.get("StandardErrorPath"),
                "sampled_tree_cpu_percent": members.as_ref().map(|rows| rows.iter().filter_map(|p| p.cpu_percent).sum::<f64>()),
                "unmeasured_tree_processes": members.as_ref().map(|rows| rows.iter().filter(|p| p.cpu_percent.is_none()).count()),
                "sampled_tree_rss_kib": members.as_ref().map(|rows| rows.iter().filter_map(|p| p.rss_kib).sum::<u64>())
            }));
        }
    }
    for label in loaded.keys().filter(|label| relevant(label)) {
        if !jobs.iter().any(|job| job["label"] == label.as_str()) {
            jobs.push(json!({"label":label,"loaded_in_user_domain":true,"plist":null,"note":"No matching plist found in scanned directories"}));
        }
    }
    jobs.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
    jobs
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tree_totals_include_grandchildren_without_looping_on_cycles() {
        let row = |pid, ppid| Usage {
            pid,
            ppid,
            name: "test".into(),
            cpu_percent: Some(1.0),
            rss_kib: None,
        };
        let usage = [row(1, 3), row(2, 1), row(3, 2), row(4, 0)];
        assert_eq!(descendants(1, &usage).len(), 3);
        assert!(descendants(9, &usage).is_empty());
    }
}
