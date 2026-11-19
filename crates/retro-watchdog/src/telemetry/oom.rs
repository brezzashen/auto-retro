use crate::monitor::{root, write_json};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const BUDGET: u64 = 2 * 1024 * 1024;
#[derive(Default, Deserialize, Serialize)]
struct Cursor {
    inode: u64,
    offset: u64,
    seen: Vec<String>,
    gap: Option<String>,
    #[serde(default)]
    context: Vec<Value>,
}

fn victim(line: &Value) -> Option<(&str, &str)> {
    if line
        .get("component")
        .and_then(Value::as_str)
        .is_some_and(|c| c != "kmsg")
    {
        return None;
    }
    let message = line["msg"].as_str()?;
    if !message.contains("Out of memory: Killed process ")
        && !message.contains("Memory cgroup out of memory: Killed process ")
    {
        return None;
    }
    Some((line["time"].as_str()?, message))
}

fn event_id(time: &str, message: &str) -> String {
    // Duplicate kernel delivery can have different subsecond log timestamps.
    let key = format!("{}:{}", time.get(..19).unwrap_or(time), message);
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    });
    format!("9{hash}")
}

fn candidates(directory: &Path) -> Result<Vec<(PathBuf, std::fs::Metadata)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "init.log" || name.starts_with("init.log.") {
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                files.push((entry.path(), metadata));
            }
        }
    }
    files.sort_by_key(|(path, metadata)| {
        (
            path.file_name().is_some_and(|name| name == "init.log"),
            metadata.mtime(),
            metadata.mtime_nsec(),
        )
    });
    anyhow::ensure!(
        files.len() <= 256,
        "Docker log rotation inventory exceeds 256 files"
    );
    Ok(files)
}

pub fn observe(home: &Path, now: f64) -> Result<Value> {
    let path = home.join("Library/Containers/com.docker.docker/Data/log/vm/init.log");
    let metadata = std::fs::metadata(&path).context("Docker VM kernel log unavailable")?;
    let checkpoint = root(home).join("oom-cursor.json");
    let mut cursor: Cursor = match std::fs::read(&checkpoint) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("invalid OOM checkpoint")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let cursor = Cursor {
                inode: metadata.ino(),
                offset: metadata.len(),
                ..Cursor::default()
            };
            write_json(&checkpoint, &cursor)?;
            return Ok(json!({"at_ms":now*1000.0,"initialized_at_current_end":true,
                "inode":cursor.inode,"offset":cursor.offset,"error":null}));
        }
        Err(error) => return Err(error.into()),
    };
    let mut files = if cursor.inode == metadata.ino() {
        vec![(path.clone(), metadata)]
    } else {
        candidates(path.parent().unwrap())?
    };
    let index = files
        .iter()
        .position(|(_, metadata)| metadata.ino() == cursor.inode);
    if let Some(index) = index {
        files.drain(..index);
    } else {
        cursor.gap = Some("Previous inode no longer retained; rotated log coverage lost".into());
        files.retain(|(file, _)| file == &path);
        cursor.inode = 0;
        cursor.offset = 0;
    }
    let mut remaining = BUDGET;
    let mut emitted = 0;
    for (file, metadata) in files {
        if remaining == 0 {
            break;
        }
        if cursor.inode != metadata.ino() {
            cursor.inode = metadata.ino();
            cursor.offset = 0;
        }
        if cursor.offset > metadata.len() {
            cursor.gap = Some("Docker log truncated; prior coverage lost".into());
            cursor.offset = 0;
        }
        let mut input = File::open(&file)?;
        input.seek(SeekFrom::Start(cursor.offset))?;
        let mut reader = BufReader::new(input.take(remaining));
        loop {
            let mut bytes = Vec::new();
            let count = reader.read_until(b'\n', &mut bytes)?;
            if count == 0 {
                break;
            }
            remaining = remaining.saturating_sub(count as u64);
            // Do not commit a partial final line; retry from its start next tick.
            if bytes.last() != Some(&b'\n') {
                break;
            }
            if let Ok(line) = serde_json::from_slice::<Value>(&bytes) {
                if line["component"] == "kmsg" {
                    cursor
                        .context
                        .push(json!({"time":line["time"],"msg":line["msg"]}));
                    if cursor.context.len() > 128 {
                        cursor.context.remove(0);
                    }
                }
                if let Some((time, message)) = victim(&line) {
                    let id = event_id(time, message);
                    if !cursor.seen.contains(&id) {
                        let incident = root(home).join("incidents").join(format!("{id}.json"));
                        if !incident.exists() {
                            write_json(
                                &incident,
                                &json!({"id":id,"trigger":"guest_oom_kill",
                                "detected_at_ms":now*1000.0,"guest_log_time_utc":time,
                                "kernel_message":message,"kernel_context_before":cursor.context,
                                "oom_scope":if message.starts_with("Memory cgroup") {"memory_cgroup"} else {"guest_global"},
                                "source":file,"source_inode":cursor.inode,
                                "source_offset":cursor.offset,"coverage_gap":cursor.gap,
                                "semantics":"Killed process is the victim, not proof of the memory consumer or cause",
                                "telemetry":super::evidence(home)}),
                            )?;
                            emitted += 1;
                        }
                        cursor.seen.push(id);
                        if cursor.seen.len() > 128 {
                            cursor.seen.remove(0);
                        }
                    }
                }
            }
            cursor.offset += count as u64;
        }
        // A budget-limited or partial read must stay on this inode until drained.
        if cursor.offset < metadata.len() {
            break;
        }
    }
    write_json(&checkpoint, &cursor)?;
    Ok(
        json!({"at_ms":now*1000.0,"error":null,"new_incidents":emitted,
        "inode":cursor.inode,"offset":cursor.offset,"coverage_gap":cursor.gap,
        "read_budget_bytes":BUDGET,"budget_exhausted":remaining==0}),
    )
}

#[cfg(test)]
#[path = "oom_tests.rs"]
mod tests;

/// `guest_oom_kill` incidents carry their scope before the large telemetry block.
const SCOPE_PREFIX_BYTES: u64 = 256 * 1024;

/// Write times (ms) of the guest-wide OOM kill incidents written since `from_ms`, oldest first.
/// Docker restart attribution reads them (`docker_watch/restarts.rs`, node N4.9).
pub(super) fn guest_global_times(home: &Path, from_ms: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(root(home).join("incidents"));
    for entry in entries.into_iter().flatten().flatten() {
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        let at = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or_default();
        if at < from_ms || entry.path().extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(file) = File::open(entry.path()) else {
            continue;
        };
        let mut head = Vec::new();
        // The prefix may end inside a multibyte character; a lossy decode keeps the rest readable.
        if file.take(SCOPE_PREFIX_BYTES).read_to_end(&mut head).is_ok()
            && guest_global(&String::from_utf8_lossy(&head))
        {
            out.push(at);
        }
    }
    out.sort_by(f64::total_cmp);
    out
}

fn guest_global(head: &str) -> bool {
    let compact: String = head.split_whitespace().collect();
    compact.contains(r#""oom_scope":"guest_global""#)
}
