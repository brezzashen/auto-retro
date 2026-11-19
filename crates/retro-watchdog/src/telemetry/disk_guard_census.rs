//! Low-point census (founder, 2026-11-12, disk-low 88688732850875351539): when an S0 run starts with the
//! host below MIN_FREE_GIB, list the largest directories while S0 runs. A writer that frees its own output
//! before the disk-low session starts (133.7 GiB at 16:24, 238.8 GiB by 17:16, S0 removed nothing) can then
//! be named from `disk-guard/<ms>-census.json`. Read-only; `du` runs niced, bounded by BUDGET, one root at
//! a time, its output in a file (a pipe would fill and block on a large tree).
use super::{lowest, round, GIB, MIN_FREE_GIB};
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const BUDGET: Duration = Duration::from_secs(900);
const TOP: usize = 40;

/// The census thread of one S0 run and the file it writes.
pub(super) struct Census(Option<(PathBuf, JoinHandle<()>)>);

/// Starts the census when `before` (the S0 run's volumes) is below the floor; a no-op otherwise.
pub(super) fn start(home: &Path, before: &Value) -> Census {
    if cfg!(test) || !lowest(before).is_some_and(|free| free < MIN_FREE_GIB) {
        return Census(None);
    }
    let out = root(home).join("disk-guard").join(format!("{}-census.json", (crate::unix_now() * 1000.0) as u64));
    let (home, target) = (home.to_path_buf(), out.clone());
    let handle = std::thread::Builder::new().name("watchdog-disk-census".into())
        .spawn(move || if let Err(error) = run(&home, &target) { crate::log(&format!("disk guard census: {error:#}")) })
        .ok();
    Census(handle.map(|handle| (out, handle)))
}

impl Census {
    /// Waits for the census (bounded by BUDGET) and returns its path, or null when none was written.
    pub(super) fn finish(self) -> Value {
        let Some((out, handle)) = self.0 else { return Value::Null };
        let _ = handle.join();
        if out.exists() { json!(out) } else { Value::Null }
    }
}

fn run(home: &Path, out: &Path) -> Result<()> {
    let started = Instant::now();
    let mut roots = vec![(home.to_path_buf(), 3), (PathBuf::from("/private/tmp"), 2)];
    // The per-user `/var/folders/<xx>/<id>` folder holds T (TMPDIR) and C (app caches).
    if let Some(user) = std::env::var_os("TMPDIR").map(PathBuf::from).and_then(|t| t.parent().map(Path::to_path_buf)) {
        roots.push((user, 2));
    }
    let (mut rows, mut done) = (Vec::new(), Vec::new());
    for (index, (dir, depth)) in roots.iter().enumerate() {
        let raw = out.with_extension(format!("du{index}"));
        let status = du(dir, *depth, &raw, BUDGET.saturating_sub(started.elapsed()));
        let text = std::fs::read_to_string(&raw).unwrap_or_default();
        let _ = std::fs::remove_file(&raw);
        done.push(json!({"root": dir, "depth": depth, "status": status}));
        rows.extend(parse(&text, dir));
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows.truncate(TOP);
    let top: Vec<Value> = rows.iter().map(|(path, kb)| json!({"path": path, "gib": round(*kb as f64 * 1024.0 / GIB)})).collect();
    write_json(out, &json!({"at_ms": crate::unix_now() * 1000.0, "seconds": started.elapsed().as_secs(), "roots": done, "top": top}))
}

/// Runs `du -xk -d <depth>` into `raw`; returns "ok", "exit N" (du exits 1 on unreadable entries and
/// still prints the rest), "timeout" or the spawn error.
fn du(dir: &Path, depth: u32, raw: &Path, left: Duration) -> String {
    let Ok(file) = std::fs::File::create(raw) else { return "error: output file".into() };
    let spawned = Command::new("/usr/bin/nice").args(["-n", "10", "/usr/bin/du", "-xk", "-d", &depth.to_string()])
        .arg(dir).stdin(Stdio::null()).stdout(file).stderr(Stdio::null()).spawn();
    let mut child = match spawned { Ok(child) => child, Err(error) => return format!("error: {error}") };
    let deadline = Instant::now() + left;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return "ok".into(),
            Ok(Some(status)) => return format!("exit {}", status.code().unwrap_or(-1)),
            Ok(None) if Instant::now() >= deadline => { let _ = child.kill(); let _ = child.wait(); return "timeout".into() }
            Ok(None) => std::thread::sleep(Duration::from_secs(1)),
            Err(error) => return format!("error: {error}"),
        }
    }
}

/// `du -k` lines (`<KiB>\t<path>`) except the root itself.
fn parse(text: &str, root_dir: &Path) -> Vec<(String, u64)> {
    text.lines().filter_map(|line| line.split_once('\t'))
        .filter(|(_, path)| Path::new(path) != root_dir)
        .filter_map(|(kb, path)| Some((path.to_string(), kb.trim().parse().ok()?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_the_root_and_bad_lines() {
        let text = "100\t/h/a\n300\t/h/b/c\nx\t/h/bad\n500\t/h\n";
        assert_eq!(parse(text, Path::new("/h")), vec![("/h/a".to_string(), 100), ("/h/b/c".to_string(), 300)]);
    }

    #[test]
    fn du_lists_subdirectories_through_a_file() {
        let dir = std::env::temp_dir().join(format!("census-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("big")).unwrap();
        std::fs::write(dir.join("big/blob"), vec![7u8; 2 << 20]).unwrap();
        let raw = dir.with_extension("du");
        assert_eq!(du(&dir, 1, &raw, Duration::from_secs(30)), "ok");
        let rows = parse(&std::fs::read_to_string(&raw).unwrap(), &dir);
        let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_file(&raw));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].0.ends_with("/big") && rows[0].1 >= 2048, "{rows:?}");
    }

    #[test]
    fn start_is_a_no_op_above_the_floor_and_in_tests() {
        let before = json!([{"path": "/System/Volumes/Data", "free_gib": MIN_FREE_GIB + 1.0}]);
        assert!(start(Path::new("/nonexistent"), &before).finish().is_null());
        let low = json!([{"path": "/System/Volumes/Data", "free_gib": 1.0}]);
        assert!(start(Path::new("/nonexistent"), &low).finish().is_null());
    }
}
