use anyhow::{Context, Result};
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

pub fn capture(home: &Path, program: &str, args: &[&str], seconds: u64) -> Result<String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(crate::monitor::root(home))?;
    let key = format!(
        "{:.0}-{}-{}",
        crate::unix_now() * 1000.0,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let path = crate::monitor::root(home).join(format!("probe-{key}.tmp"));
    let stderr_path = path.with_extension("stderr.tmp");
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&stderr_path)?;
    let result = (|| {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(stderr)
            .spawn()
            .context("start evidence probe")?;
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::ensure!(status.success(), "evidence probe exited {status}");
                break;
            }
            let oversized = [&path, &stderr_path].iter().any(|path| {
                std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > 4 * 1024 * 1024)
            });
            if start.elapsed() >= Duration::from_secs(seconds) || oversized {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::ensure!(!oversized, "evidence probe output exceeded 4 MiB");
                anyhow::bail!("evidence probe timed out after {seconds}s");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let file = std::fs::File::open(&path)?;
        anyhow::ensure!(
            file.metadata()?.len() <= 4 * 1024 * 1024,
            "evidence probe output exceeded 4 MiB"
        );
        let mut text = String::new();
        file.take(4 * 1024 * 1024).read_to_string(&mut text)?;
        Ok(text)
    })();
    match result {
        Ok(text) => {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(stderr_path);
            Ok(text)
        }
        Err(error) => {
            let directory = crate::monitor::root(home).join("probe-errors").join(key);
            crate::monitor::write_json(
                &directory.join("failure.json"),
                &serde_json::json!({
                "at_ms":crate::unix_now()*1000.0,"program":Path::new(program).file_name().map(|name|name.to_string_lossy()),
                "error":format!("{error:#}"),"stdout":"stdout.log","stderr":"stderr.log"}),
            )?;
            std::fs::rename(path, directory.join("stdout.log"))?;
            std::fs::rename(stderr_path, directory.join("stderr.log"))?;
            Err(error.context(format!("raw probe failure: {}", directory.display())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failing_and_timed_out_probes_leave_no_temporary_files() {
        let home = super::super::test_home("command");
        let state = crate::monitor::root(&home);
        std::fs::create_dir_all(&state).unwrap();
        assert_eq!(
            capture(&home, "/usr/bin/printf", &["hello"], 1).unwrap(),
            "hello"
        );
        assert!(capture(&home, "/usr/bin/false", &[], 1).is_err());
        assert!(format!(
            "{:#}",
            capture(&home, "/bin/sleep", &["10"], 0).unwrap_err()
        )
        .contains("timed out"));
        assert_eq!(std::fs::read_dir(&state).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_dir(state.join("probe-errors"))
                .unwrap()
                .count(),
            2
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
