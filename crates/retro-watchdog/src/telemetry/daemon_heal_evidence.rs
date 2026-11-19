//! Evidence kept before restarting a daemon whose cost is memory, not only CPU (founder decision
//! 2026-11-15, report perf-memory-1794682770151 item 2, cause-tree N8.4.1.1). fseventsd saturates
//! after about two billion events of its life and grows to 38-44 GiB of footprint; the restart is
//! the only cure, so the stacks and the footprint must be on disk before SIGTERM removes them.
//! One 5 s `sample` into `daemon-restarts/evidence/` and the `top` footprint of the old and the
//! respawned instance, recorded on the restart record (`footprint`, `new_footprint`, `sample`).
use crate::telemetry::memory_holders::{parse, Holder};
use serde_json::{json, Value};
use std::{
    io::Read,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Daemons restarted only after their sample and footprint were saved.
pub(super) const EVIDENCE_FIRST: &[&str] = &["fseventsd"];
const SAMPLE_SECS: &str = "5";
/// `sample` of fseventsd at 43 GiB on the swapping host of 2026-11-15 finished well inside this.
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(120);
const TOP_TIMEOUT_SECS: u64 = 20;
const GIB: f64 = 1_073_741_824.0;

fn gib(holder: &Holder) -> Value {
    let round = |b: f64| (b / GIB * 100.0).round() / 100.0;
    json!({"pid": holder.pid, "footprint_gib": round(holder.footprint), "compressed_gib": round(holder.compressed)})
}

/// Physical footprint of one pid from `top`; an error is recorded, never a guessed number.
pub(super) fn footprint(home: &Path, pid: i32) -> Value {
    let pid = pid.to_string();
    let args = ["-l", "1", "-pid", &pid, "-stats", "pid,mem,cmprs,command"];
    match crate::telemetry::command::capture(home, "/usr/bin/top", &args, TOP_TIMEOUT_SECS) {
        Ok(text) => parse(&text).first().map_or_else(|| json!({"error": "no top row for pid"}), gib),
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

/// A 5 s root `sample` of `pid` written to `daemon-restarts/evidence/<name>-<ms>-sample.txt`.
pub(super) fn sample(home: &Path, now: f64, name: &str, pid: i32) -> Value {
    let directory = crate::monitor::root(home).join("daemon-restarts").join("evidence");
    let path = directory.join(format!("{name}-{:.0}-sample.txt", now * 1000.0));
    let run = || -> anyhow::Result<()> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
        let file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        let mut child = Command::new("/usr/bin/sudo")
            .args(["-A", "/usr/bin/sample", &pid.to_string(), SAMPLE_SECS, "-file", "/dev/stdout"])
            .env("SUDO_ASKPASS", format!("{}/.local/bin/sudo-askpass", std::env::var("HOME")?))
            .stdin(Stdio::null()).stdout(file).stderr(Stdio::piped()).spawn()?;
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? { break status; }
            if started.elapsed() >= SAMPLE_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("sample timed out after {} s", SAMPLE_TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() { let _ = pipe.read_to_string(&mut stderr); }
        anyhow::ensure!(status.success(), "sample {status}: {}", stderr.lines().last().unwrap_or("").trim());
        Ok(())
    };
    match run() {
        Ok(()) => json!(path.display().to_string()),
        Err(error) => json!({"error": format!("{error:#}"), "path": path.display().to_string()}),
    }
}

/// FSEvents clients named in the pre-restart sample's delivery threads
/// (`com.apple.fseventsd.client.<pid>.<name>`). A 5 s sample shows the clients that had a delivery
/// thread then; an idle client can be missing, so the list is a floor, not a census.
pub(super) fn clients(sample_text: &str) -> Vec<(i32, String)> {
    const MARK: &str = "com.apple.fseventsd.client.";
    let mut found: Vec<(i32, String)> = sample_text.lines().filter_map(|line| {
        let rest = &line[line.find(MARK)? + MARK.len()..];
        let (pid, name) = rest.split_once('.')?;
        Some((pid.parse().ok()?, name.split_whitespace().next()?.to_string()))
    }).collect();
    found.sort();
    found.dedup();
    found
}

/// After an fseventsd restart, streams opened before it re-register but receive no events
/// (perf session 1794671950116, 2026-11-15: a node probe got 0 events in 6 min; Storybook stopped
/// hot-reloading until restarted; Spotlight and launchd WatchPaths were unaffected). Restart the CRM
/// Storybook catalog, and name every other client from the sample still running from before the
/// restart, so the founder knows which watchers to restart; agents and apps are never killed here.
pub(super) fn rewatch(restarted_at: f64, sample: &Value) -> Value {
    let listed = sample.as_str().and_then(|path| std::fs::read_to_string(path).ok()).map(|text| clients(&text));
    let storybook = crate::telemetry::storybook_watch::restart_if_older(restarted_at);
    let alive = crate::procs::snapshot(crate::unix_now).map(|(_, procs)| procs).unwrap_or_default();
    let stale: Vec<Value> = listed.iter().flatten()
        .filter(|(pid, _)| alive.iter().any(|p| p.pid == *pid && (p.start as f64) < restarted_at))
        .map(|(pid, name)| json!({"pid": pid, "name": name})).collect();
    json!({"storybook": storybook, "clients_seen": listed.map(|l| l.len()), "stale_watchers": stale})
}

/// One line for the notification and the report: "Storybook 已重启；仍需重启：claude 37307, …".
pub(super) fn summary(rewatch: &Value) -> String {
    let storybook = match rewatch["storybook"]["outcome"].as_str() {
        Some("restarted") => "Storybook 已重启".to_string(),
        Some("failed") => "Storybook 重启失败".to_string(),
        _ => format!("Storybook 未重启（{}）", rewatch["storybook"]["reason"].as_str().unwrap_or("?")),
    };
    let stale: Vec<String> = rewatch["stale_watchers"].as_array().into_iter().flatten()
        .map(|w| format!("{} {}", w["name"].as_str().unwrap_or("?"), w["pid"])).collect();
    if stale.is_empty() { storybook } else { format!("{storybook}；重启前就在运行、收不到文件事件的进程：{}", stale.join("、")) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn clients_come_from_delivery_thread_names() {
        let text = "    2729 Thread_227821967: com.apple.fseventsd.client.58874.node\n\
            +     2729 Thread_1: com.apple.fseventsd.client.27919.com.apple.dt.SKA\n\
            2729 Thread_2: com.apple.fseventsd.client.30728.2.1.325\n\
            2729 Thread_3: com.apple.fseventsd.client.58874.node\n\
            2729 Thread_5414: com.apple.fseventsd.disklogger.109.fseventsd\n";
        assert_eq!(super::clients(text), vec![(27919, "com.apple.dt.SKA".to_string()), (30728, "2.1.325".to_string()), (58874, "node".to_string())]);
    }

    #[test]
    fn summary_names_storybook_and_the_stale_watchers() {
        let rewatch = serde_json::json!({"storybook": {"outcome": "restarted"}, "stale_watchers": [{"pid": 37307, "name": "2.1.325"}]});
        assert_eq!(super::summary(&rewatch), "Storybook 已重启；重启前就在运行、收不到文件事件的进程：2.1.325 37307");
        let skipped = serde_json::json!({"storybook": {"outcome": "skipped", "reason": "no crm.pid"}, "stale_watchers": []});
        assert_eq!(super::summary(&skipped), "Storybook 未重启（no crm.pid）");
    }
}
