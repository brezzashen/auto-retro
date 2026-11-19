//! Process table snapshots from `ps`, joined by pid: `comm` gives the executable path (which may
//! contain spaces), `args` the full command line used for build exemptions.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    /// Approximate start time in unix seconds (now − elapsed); used to tell a reused pid apart.
    pub start: i64,
    /// Cumulative CPU time in seconds across all threads.
    pub cpu_secs: f64,
    pub rss_kib: u64,
    /// Executable basename, e.g. `node` or `Google Chrome Helper (Renderer)`.
    pub name: String,
    pub args: String,
}

/// Returns the snapshot and the unix time its CPU times were read at; the clock is read right
/// after the `time` column so a slow `ps` over a large table does not dilute the usage.
pub fn snapshot(clock: impl Fn() -> f64) -> Result<(f64, Vec<Proc>)> {
    let table = ps(&["-Axo", "pid=,ppid=,uid=,etime=,time=,rss=,comm="])?;
    let now = clock();
    let args = parse_args(&ps(&["-Axo", "pid=,args="])?);
    Ok((now, parse_table(&table, &args, now as i64)))
}

/// Start time (unix seconds) of `pid` read now, or None when no such process exists. Called right
/// before a signal so a pid reused since the snapshot is never signalled.
pub fn start_now(pid: i32, clock: impl Fn() -> f64) -> Option<i64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "etime=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let elapsed = parse_duration(String::from_utf8_lossy(&output.stdout).trim())?;
    Some(clock() as i64 - elapsed as i64)
}

/// Whether `current` (from `start_now`) still names the process recorded with start `recorded`.
pub fn is_same_process(recorded: i64, current: Option<i64>) -> bool {
    current.is_some_and(|start| (start - recorded).abs() <= 2)
}

/// True only when `pid` still exists with the start time recorded in the snapshot.
pub fn still_same_process(pid: i32, recorded: i64, clock: impl Fn() -> f64) -> bool {
    is_same_process(recorded, start_now(pid, clock))
}

fn ps(arguments: &[&str]) -> Result<String> {
    let output = Command::new("/bin/ps")
        .args(arguments)
        .output()
        .context("run ps")?;
    if !output.status.success() {
        bail!("ps exited with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn parse_args(text: &str) -> HashMap<i32, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            Some((pid.parse().ok()?, rest.trim().to_string()))
        })
        .collect()
}

pub fn parse_table(text: &str, args: &HashMap<i32, String>, now: i64) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| parse_row(line, args, now))
        .collect()
}

fn parse_row(line: &str, args: &HashMap<i32, String>, now: i64) -> Option<Proc> {
    let mut rest = line.trim_start();
    let mut fields = [""; 6];
    for field in fields.iter_mut() {
        let (value, tail) = rest.split_once(char::is_whitespace)?;
        *field = value;
        rest = tail.trim_start();
    }
    let pid: i32 = fields[0].parse().ok()?;
    let comm = rest.trim_end();
    Some(Proc {
        pid,
        ppid: fields[1].parse().ok()?,
        uid: fields[2].parse().ok()?,
        start: now - parse_duration(fields[3])? as i64,
        cpu_secs: parse_duration(fields[4])?,
        rss_kib: fields[5].parse().ok()?,
        name: comm.rsplit('/').next().unwrap_or(comm).to_string(),
        args: args.get(&pid).cloned().unwrap_or_default(),
    })
}

/// Parses `ps` durations: `[dd-][hh:]mm:ss[.cc]`; macOS `time` also prints minutes past 59 (`401:21.49`).
pub fn parse_duration(text: &str) -> Option<f64> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<f64>().ok()?, clock),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in clock.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_cover_ps_formats() {
        assert_eq!(parse_duration("0:55.38"), Some(55.38));
        assert_eq!(parse_duration("401:21.49"), Some(401.0 * 60.0 + 21.49));
        assert_eq!(parse_duration("01:04:47"), Some(3887.0));
        assert_eq!(
            parse_duration("03-04:07:28"),
            Some(3.0 * 86_400.0 + 4.0 * 3600.0 + 7.0 * 60.0 + 28.0)
        );
        assert_eq!(parse_duration("bogus"), None);
    }

    #[test]
    fn a_pid_is_the_same_process_only_with_a_matching_start_time() {
        assert!(
            is_same_process(1_000, Some(1_001)),
            "within the one-second rounding of ps"
        );
        assert!(
            !is_same_process(1_000, Some(1_300)),
            "pid reused by a later process"
        );
        assert!(!is_same_process(1_000, None), "process gone");
    }

    #[test]
    fn rows_keep_names_with_spaces_and_join_args() {
        let args = parse_args("  812 /Applications/Google Chrome.app/Helper --type=renderer\n");
        let table = "  812   700   501    10:00   9:30.00 2048 /Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Helper (Renderer)\n\
                     garbage line\n";
        let procs = parse_table(table, &args, 10_000);
        assert_eq!(procs.len(), 1);
        let proc = &procs[0];
        assert_eq!(
            (proc.pid, proc.ppid, proc.uid, proc.start),
            (812, 700, 501, 9_400)
        );
        assert_eq!(proc.cpu_secs, 570.0);
        assert_eq!(proc.rss_kib, 2048);
        assert_eq!(proc.name, "Google Chrome Helper (Renderer)");
        assert!(proc.args.ends_with("--type=renderer"));
    }
}
