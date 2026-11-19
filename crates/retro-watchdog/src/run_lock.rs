//! Single-instance guard for the resident loop. A second `cpu-watchdog run` beside the
//! LaunchAgent would repeat every recovery action and session launch, so it refuses to start.
use anyhow::{bail, Context, Result};
use std::{fs::{File, OpenOptions}, os::{fd::AsRawFd, unix::fs::OpenOptionsExt}, path::Path};

/// Holds `run.lock` until the process exits. std opens it close-on-exec, so launched
/// sessions and recovery commands never inherit the lock.
#[derive(Debug)]
pub struct RunLock(#[allow(dead_code)] File);

pub fn acquire(state_dir: &Path) -> Result<RunLock> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join("run.lock");
    let file = OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    // SAFETY: flock on a valid descriptor owned by `file`.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("another cpu-watchdog run holds {}; the resident loop is LaunchAgent local.autoretro.cpu-watchdog", path.display());
    }
    Ok(RunLock(file))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_second_run_is_refused_until_the_first_exits() {
        let dir = std::env::temp_dir().join(format!("cpu-watchdog-run-lock-{}", std::process::id()));
        let first = super::acquire(&dir).unwrap();
        assert!(super::acquire(&dir).unwrap_err().to_string().contains("another cpu-watchdog run"));
        drop(first);
        assert!(super::acquire(&dir).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
