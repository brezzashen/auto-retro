//! Error evidence for a surge: the user's launchd jobs (`launchctl list`), their plist schedule
//! and log paths, the tail of a failing job's stderr, and crash reports written in the last
//! CRASH_WINDOW_SECS. Reads are bounded; nothing here runs or signals a job.
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{io::{Read, Seek, SeekFrom}, path::{Path, PathBuf}, process::Command, time::SystemTime};

const TAIL_BYTES: u64 = 4096;
const TAIL_LINES: usize = 20;
const CRASH_WINDOW_SECS: u64 = 900;
const MAX_CRASHES: usize = 20;
/// Exits by a crash signal (SIGILL, SIGTRAP, SIGABRT, SIGFPE, SIGBUS, SIGSEGV); -9/-15 are
/// ordinary stops by launchd or a restart.
const CRASH_SIGNALS: &[i64] = &[-4, -5, -6, -8, -10, -11];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Job {
    pub label: String,
    pub pid: Option<i32>,
    pub status: i64,
    pub keepalive: bool,
    pub stderr: Option<PathBuf>,
}

impl Job {
    pub fn failing(&self) -> bool {
        self.status > 0 || CRASH_SIGNALS.contains(&self.status)
    }
}

/// Every job in the user's launchd domain; own jobs also get their plist's KeepAlive and stderr.
pub fn list(home: &Path) -> Result<Vec<Job>> {
    let output = Command::new("/bin/launchctl").arg("list").output().context("run launchctl list")?;
    anyhow::ensure!(output.status.success(), "launchctl list exited with {}", output.status);
    let mut jobs = parse(&String::from_utf8_lossy(&output.stdout));
    for job in jobs.iter_mut().filter(|job| super::rules::own_job(&job.label) || job.failing()) {
        let plist = home.join("Library/LaunchAgents").join(format!("{}.plist", job.label));
        if let Some(value) = plist_json(&plist) {
            job.keepalive = value["KeepAlive"].as_bool().unwrap_or(false) || value["KeepAlive"].is_object();
            job.stderr = value["StandardErrorPath"].as_str().map(PathBuf::from);
        }
    }
    Ok(jobs)
}

pub fn parse(text: &str) -> Vec<Job> {
    text.lines().skip(1).filter_map(|line| {
        let mut columns = line.split('\t');
        let pid = columns.next()?.trim();
        let status = columns.next()?.trim().parse().ok()?;
        let label = columns.next()?.trim().to_string();
        Some(Job { label, pid: pid.parse().ok(), status, ..Job::default() })
    }).collect()
}

fn plist_json(path: &Path) -> Option<Value> {
    let output = Command::new("/usr/bin/plutil").args(["-convert", "json", "-o", "-"]).arg(path).output().ok()?;
    output.status.success().then(|| serde_json::from_slice(&output.stdout).ok()).flatten()
}

/// The last TAIL_LINES lines of at most the final TAIL_BYTES of `path`.
pub fn tail(path: &Path) -> Option<Vec<String>> {
    let lines = tail_lines(path, TAIL_BYTES)?;
    Some(lines[lines.len().saturating_sub(TAIL_LINES)..].to_vec())
}

/// Whole lines within the final `limit` bytes of `path`; a line cut by the limit is dropped.
pub fn tail_lines(path: &Path, limit: u64) -> Option<Vec<String>> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(limit))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(text.lines().skip(usize::from(length > limit)).map(String::from).collect())
}

/// Crash report file names (user and system DiagnosticReports) modified in the recent window.
pub fn recent_crashes(home: &Path) -> Vec<Value> {
    let mut found = Vec::new();
    for directory in [home.join("Library/Logs/DiagnosticReports"), PathBuf::from("/Library/Logs/DiagnosticReports")] {
        let Ok(entries) = std::fs::read_dir(&directory) else { continue };
        for entry in entries.flatten().take(4096) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !(name.ends_with(".ips") || name.ends_with(".crash") || name.ends_with(".spin") || name.ends_with(".hang")) {
                continue;
            }
            let age = entry.metadata().ok().and_then(|m| m.modified().ok())
                .and_then(|modified| SystemTime::now().duration_since(modified).ok());
            if let Some(age) = age.filter(|age| age.as_secs() <= CRASH_WINDOW_SECS) {
                found.push((age, serde_json::json!({"file": entry.path(), "age_s": age.as_secs()})));
            }
        }
    }
    found.sort_by_key(|(age, _)| *age);
    found.into_iter().take(MAX_CRASHES).map(|(_, value)| value).collect()
}
