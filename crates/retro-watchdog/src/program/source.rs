//! Source fingerprints describe files observed now, not proof of a running build's inputs.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

fn read(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

fn text(path: &Path) -> Option<String> {
    String::from_utf8(read(path, 8192)?)
        .ok()
        .map(|s| s.trim().into())
}

pub fn file(path: &Path, hash_source: bool) -> Value {
    let metadata = path.metadata().ok();
    let digest = hash_source
        .then(|| read(path, 256 * 1024))
        .flatten()
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
    json!({"path":path,"bytes":metadata.as_ref().map(|m|m.len()),
        "modified_at_ms":metadata.and_then(|m|m.modified().ok()).and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|d|d.as_millis()),
        "sha256":digest})
}

fn safe_ref(value: &str) -> bool {
    value.starts_with("refs/")
        && !value.split('/').any(|p| p == "..")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
}

pub fn repository(cwd: &Path) -> Option<Value> {
    let root = cwd
        .ancestors()
        .take(32)
        .find(|path| path.join(".git").exists())?;
    let marker = root.join(".git");
    let git = if marker.is_dir() {
        marker
    } else {
        let target = text(&marker)?.strip_prefix("gitdir: ")?.to_string();
        root.join(target)
    };
    let common = text(&git.join("commondir"))
        .map(|v| git.join(v))
        .unwrap_or_else(|| git.clone());
    let head = text(&git.join("HEAD"))?;
    let reference = head.strip_prefix("ref: ").filter(|r| safe_ref(r));
    let commit = match reference {
        Some(reference) => text(&git.join(reference))
            .or_else(|| text(&common.join(reference)))
            .or_else(|| {
                let packed =
                    String::from_utf8(read(&common.join("packed-refs"), 256 * 1024)?).ok()?;
                packed.lines().find_map(|line| {
                    line.split_once(' ')
                        .filter(|(_, r)| *r == reference)
                        .map(|(s, _)| s.to_string())
                })
            }),
        None => Some(head.clone()),
    }
    .filter(|s| [40, 64].contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit()));
    Some(
        json!({"root":root,"head":commit,"branch":reference.and_then(|r|r.strip_prefix("refs/heads/")),
        "worktree_state":"not_measured","semantics":"HEAD observed at capture; uncommitted changes and running build provenance are not inferred"}),
    )
}

pub fn invocation(name: &str, args: &str, cwd: Option<&Path>) -> (Option<PathBuf>, Option<String>) {
    let words: Vec<_> = args.split_whitespace().take(32).collect();
    let runtime = ["bun", "node", "npm", "pnpm", "yarn", "cargo", "xcodebuild"].contains(&name);
    if !runtime {
        return (None, None);
    }
    // Only fixed, recognized task words survive. Option values and arbitrary text never do.
    let operation = words.iter().skip(1).take(2).find_map(|w| {
        if [
            "test",
            "build",
            "check",
            "clippy",
            "install",
            "ci",
            "archive",
            "exportArchive",
        ]
        .contains(w)
        {
            Some(w.to_string())
        } else if w.starts_with("test:") {
            Some("test:*".into())
        } else if w.starts_with("build:") {
            Some("build:*".into())
        } else {
            None
        }
    });
    let entry = if ["bun", "node"].contains(&name) {
        words
            .get(1)
            .filter(|w| !w.starts_with('-') && !w.contains(['\'', '"', '=', ':']))
            .map(Path::new)
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| ["ts", "tsx", "js", "mjs", "cjs"].iter().any(|x| e == *x))
            })
            .and_then(|p| {
                if p.is_absolute() {
                    Some(p.to_path_buf())
                } else {
                    cwd.map(|cwd| cwd.join(p))
                }
            })
            .and_then(|p| p.canonicalize().ok())
            .filter(|p| p.is_file())
    } else {
        None
    };
    (entry, operation)
}
