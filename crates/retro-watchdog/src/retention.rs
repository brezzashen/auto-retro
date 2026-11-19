//! Retention only touches this collector's named archives; live cursors and indexes survive.
use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::{Duration, SystemTime},
};

const DAY: u64 = 86_400;
const RAW_DAYS: u64 = 7;
const RECORD_DAYS: u64 = 30;
const LOG_BYTES: u64 = 10 * 1024 * 1024;
const LOG_BACKUPS: usize = 5;
const ARCHIVES: &[(&str, u64)] = &[
    ("cpu-windows", RAW_DAYS),
    ("resource-samples", RAW_DAYS),
    ("guest-samples", RAW_DAYS),
    ("probe-errors", RAW_DAYS),
    // The user keeps WindowServer stall and port logs for a month (2026-10-21).
    ("render-probe", RECORD_DAYS),
    ("port-census", RECORD_DAYS),
    ("mic-sessions", RECORD_DAYS),
    ("controlcenter-ports", RECORD_DAYS),
    ("controlcenter-restarts", RECORD_DAYS),
    ("daemon-restarts", RECORD_DAYS),
    ("stall-records", RECORD_DAYS),
    ("headless-heals", RECORD_DAYS),
    ("storybook-heals", RECORD_DAYS),
    ("docker-heals", RECORD_DAYS),
    ("win-guard", RECORD_DAYS),
    ("stream-watch", RECORD_DAYS),
    ("disk-guard", RECORD_DAYS),
    ("memory-guard", RECORD_DAYS),
    ("quota-economics", RECORD_DAYS),
    ("prompt-review", RECORD_DAYS),
    ("hook-watch", RECORD_DAYS),
    ("cache-watch", RECORD_DAYS),
    ("crm-turn-retro", RECORD_DAYS),
    ("crm-costly-turn", RECORD_DAYS),
    ("risk-retro", RECORD_DAYS),
    ("search-retro", RECORD_DAYS),
    ("recall-retro", RECORD_DAYS),
    ("memory-dream", RECORD_DAYS),
    ("task-records", RECORD_DAYS),
    ("program-records", RECORD_DAYS),
    ("container-records", RECORD_DAYS),
    ("xcode-records", RECORD_DAYS),
    ("xcode-error-records", RECORD_DAYS),
    ("incidents", RECORD_DAYS),
    ("analysis", RECORD_DAYS),
    ("periodic", RECORD_DAYS),
];

pub fn maintain(home: &Path, now: SystemTime, dry_run: bool) -> Result<Value> {
    let mut expired = Vec::new();
    for &(name, days) in ARCHIVES {
        let directory = root(home).join(name);
        if !directory.try_exists()? {
            continue;
        }
        if fs::symlink_metadata(&directory)?.file_type().is_symlink() {
            continue;
        }
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            let Some(modified) = newest(&path)? else { continue };
            if now.duration_since(modified).unwrap_or_default() < Duration::from_secs(days * DAY) {
                continue;
            }
            expired.push(path.clone());
            if !dry_run {
                if fs::symlink_metadata(&path)?.is_dir() {
                    fs::remove_dir_all(&path)?;
                } else {
                    fs::remove_file(&path)?;
                }
            }
        }
    }
    let report = json!({
        "at_ms": now.duration_since(SystemTime::UNIX_EPOCH)?.as_millis(),
        "raw_days": RAW_DAYS, "record_days": RECORD_DAYS,
        "log_max_bytes": LOG_BYTES, "log_backups": LOG_BACKUPS,
        "cleanup_interval_seconds": 3600, "dry_run": dry_run,
        "expired_entries": expired, "error": null,
    });
    if !dry_run {
        write_json(&root(home).join("retention.json"), &report)?;
    }
    Ok(report)
}

// A symlink protects its containing entry from recursive expiry; never traverse its target.
fn newest(path: &Path) -> Result<Option<SystemTime>> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(None);
    }
    let mut latest = meta.modified()?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            let Some(modified) = newest(&entry?.path())? else { return Ok(None) };
            latest = latest.max(modified);
        }
    }
    Ok(Some(latest))
}

pub fn rotate(home: &Path) -> Result<()> {
    rotate_file(&home.join("Library/Logs/cpu-watchdog.log"), LOG_BYTES, LOG_BACKUPS)
}

fn rotate_file(path: &Path, limit: u64, backups: usize) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_file() || meta.len() < limit {
        return Ok(());
    }
    let archive = |n| path.with_file_name(format!("cpu-watchdog.log.{n}"));
    // Refuse symlinks rather than overwriting paths outside the collector's log directory.
    for n in 1..=backups {
        if let Ok(meta) = fs::symlink_metadata(archive(n)) {
            anyhow::ensure!(meta.is_file() && !meta.file_type().is_symlink(), "unsafe log archive");
        }
    }
    for n in (1..backups).rev() {
        if archive(n).try_exists()? {
            fs::rename(archive(n), archive(n + 1))?;
        }
    }
    let mut source = OpenOptions::new().read(true).write(true).open(path)?;
    source.seek(SeekFrom::End(-(meta.len().min(limit) as i64)))?;
    let mut target = OpenOptions::new().create(true).truncate(true).write(true).mode(0o600)
        .open(archive(1)).context("open rotated watchdog log")?;
    std::io::copy(&mut (&source).take(limit), &mut target)?;
    target.flush()?;
    // Preserve the inode held by launchd's append-mode stdout/stderr descriptors.
    source.set_len(0)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::FileTimes;
    use std::os::unix::fs::{symlink, MetadataExt};

    #[test]
    fn retention_expires_only_owned_archives_and_preserves_recent_children_and_symlinks() {
        let home = crate::telemetry::test_home("retention");
        let base = root(&home);
        let now = SystemTime::now();
        let old = now - Duration::from_secs(40 * DAY);
        let write = |name: &str, time| {
            let path = base.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"{}").unwrap();
            fs::File::open(&path).unwrap().set_times(FileTimes::new().set_modified(time)).unwrap();
            path
        };
        let expired = write("cpu-windows/old.json", old);
        let seven_days = write("task-records/week.json", now - Duration::from_secs(8 * DAY));
        let cursor = write("oom-cursor.json", old);
        let unknown = write("backups/keep.json", old);
        let recent = write("analysis/recent/new.json", now);
        let old_analysis = write("analysis/old/report.json", old);
        fs::File::open(old_analysis.parent().unwrap()).unwrap()
            .set_times(FileTimes::new().set_modified(old)).unwrap();
        let outside = write("outside.json", old);
        symlink(&outside, base.join("cpu-windows/link.json")).unwrap();
        assert_eq!(maintain(&home, now, true).unwrap()["expired_entries"].as_array().unwrap().len(), 2);
        assert!(expired.exists());
        maintain(&home, now, false).unwrap();
        assert!(!expired.exists());
        assert!(!old_analysis.exists());
        for path in [seven_days, cursor, unknown, recent, outside] { assert!(path.exists()); }
        assert!(fs::symlink_metadata(base.join("cpu-windows/link.json")).unwrap().file_type().is_symlink());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn rotation_bounds_archives_and_preserves_open_append_descriptor() {
        let home = crate::telemetry::test_home("rotation");
        let path = home.join("cpu-watchdog.log");
        let mut writer = OpenOptions::new().create(true).append(true).open(&path).unwrap();
        let inode = writer.metadata().unwrap().ino();
        for n in 0..8 {
            writer.write_all(&[b'a' + n; 24]).unwrap();
            rotate_file(&path, 16, 5).unwrap();
        }
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(fs::read(home.join("cpu-watchdog.log.1")).unwrap(), vec![b'h'; 16]);
        assert_eq!(fs::metadata(home.join("cpu-watchdog.log.5")).unwrap().len(), 16);
        assert!(!home.join("cpu-watchdog.log.6").exists());
        writer.write_all(b"still logging").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"still logging");
        fs::remove_dir_all(home).unwrap();
    }
}
