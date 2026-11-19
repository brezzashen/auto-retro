//! Failed background outputs kept past their files (release retro 86614182838730582299): a Claude
//! background run writes its output to `/private/tmp/claude-501/<project>/<session>/tasks/<id>.output`,
//! which is purged within days, while its `<task-notification>` reports the wrapper's exit code
//! (often `0`, since agents append `; echo "exit $?"`). Once the file is gone a recomputed window
//! loses every failure only that file showed (2026-10-27 window: 50 failed calls became 11). So the
//! head and tail of every output that reads as failed are kept here by call id, and the watch warms
//! this cache every WARM_SECS over the open window, before the retro session runs.
//!
//! State: `release-retro-outputs.json` (`{call: {at_ms, output}}`), private, pruned after KEEP_MS.
use crate::monitor::{root, write_json};
use serde_json::{json, Map, Value};
use std::{io::{Read, Seek, SeekFrom}, path::{Path, PathBuf}};

const HEAD: u64 = 4_096;
const TAIL: u64 = 16_384;
/// Characters kept per failed output: classification reads the head, failure markers the tail.
const KEEP_CHARS: usize = 4_096;
pub(super) const KEEP_MS: f64 = 14.0 * 86_400_000.0;
pub(super) const WARM_SECS: f64 = 3_600.0;

/// The head and tail of a background run's output file.
pub(super) fn output_file(path: &str) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut head = vec![0; len.min(HEAD) as usize];
    file.read_exact(&mut head).ok()?;
    let mut tail = Vec::new();
    if len > HEAD { file.seek(SeekFrom::Start(len.saturating_sub(TAIL).max(HEAD))).ok()?; file.read_to_end(&mut tail).ok()?; }
    Some(format!("{}\n{}", String::from_utf8_lossy(&head), String::from_utf8_lossy(&tail)))
}

/// Head and tail of a long output, so a failed call stays classifiable from the cache alone.
fn compact(text: &str) -> String {
    let count = text.chars().count();
    if count <= KEEP_CHARS * 2 { return text.to_string(); }
    let head: String = text.chars().take(KEEP_CHARS).collect();
    let tail: String = text.chars().skip(count - KEEP_CHARS).collect();
    format!("{head}\n…\n{tail}")
}

#[derive(Default)]
pub(super) struct Outputs { path: Option<PathBuf>, map: Map<String, Value>, dirty: bool }

impl Outputs {
    pub(super) fn load(home: &Path) -> Self { Self::from_path(root(home).join("release-retro-outputs.json")) }

    pub(super) fn from_path(path: PathBuf) -> Self {
        let map = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_object().cloned()).unwrap_or_default();
        Self { path: Some(path), map, dirty: false }
    }

    /// The output of background call `id`: its file while it exists, else the kept copy.
    pub(super) fn read(&self, id: &str, path: &str) -> Option<String> {
        output_file(path).or_else(|| self.map.get(id).and_then(|e| e["output"].as_str()).map(str::to_string))
    }

    /// Keep a failed call's output the first time it is seen.
    pub(super) fn keep(&mut self, id: &str, at: f64, output: &str) {
        if self.path.is_none() || self.map.contains_key(id) { return; }
        self.map.insert(id.to_string(), json!({"at_ms": at, "output": compact(output)}));
        self.dirty = true;
    }

    pub(super) fn save(mut self, now_ms: f64) {
        let Some(path) = self.path.take() else { return };
        let before = self.map.len();
        self.map.retain(|_, e| e["at_ms"].as_f64().is_some_and(|t| now_ms - t < KEEP_MS));
        if self.dirty || self.map.len() != before { let _ = write_json(&path, &Value::Object(self.map)); }
    }
}
