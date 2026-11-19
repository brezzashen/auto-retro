//! The Docker VM's own disk, read inside its first running container: a full disk cannot start a new one.
//! The real CLI, not `~/.local/bin/docker`, which wraps every call in the build gate.
use super::S0_PATH;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// A hung engine must not stall the guard's sample.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Free GiB of the container root, or None without an engine, a running container or a readable `df`.
pub(super) fn free_gib(home: &Path) -> Option<f64> {
    let docker = home.join(".docker/bin/docker");
    let id = run(&docker, &["ps", "-q"])?.lines().map(str::trim).find(|line| !line.is_empty())?.to_string();
    parse_df(&run(&docker, &["exec", &id, "df", "-Pk", "/"])?)
}

/// Available KiB (fourth column) of `df -Pk`'s data line, in GiB.
pub(super) fn parse_df(output: &str) -> Option<f64> {
    let available: f64 = output.lines().last()?.split_whitespace().nth(3)?.parse().ok()?;
    Some(available / 1024.0 / 1024.0)
}

/// stdout of a successful run within TIMEOUT; both commands print a few lines, so the pipe never fills.
fn run(program: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program).args(args).env("PATH", S0_PATH).stdin(Stdio::null())
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().ok()? {
            let mut text = String::new();
            child.stdout.take()?.read_to_string(&mut text).ok()?;
            return status.success().then_some(text);
        }
        if started.elapsed() >= TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
