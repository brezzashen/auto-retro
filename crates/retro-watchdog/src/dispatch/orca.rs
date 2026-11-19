//! The Orca CLI as the dispatcher sees it. Orca prints its refusals as `{"ok": false, "error": …}`
//! on stdout with an empty stderr; a refusal proves nothing was created.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{path::{Path, PathBuf}, process::{Command, Stdio}};

const ORCA: &str = "/Applications/Orca.app/Contents/Resources/bin/orca";

/// Orca answered and refused the request, so no terminal or worktree exists for it.
#[derive(Debug)]
pub(crate) struct OrcaRefused(pub String);
impl std::fmt::Display for OrcaRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl std::error::Error for OrcaRefused {}

pub(crate) fn orca(args: &[&str]) -> Result<Value> {
    let out = Command::new(ORCA).args(args).stdin(Stdio::null()).output().context("spawn orca")?;
    if !out.status.success() {
        let head = args[..2.min(args.len())].join(" ");
        if let Some(code) = refusal(&out.stdout) {
            return Err(anyhow::Error::new(OrcaRefused(format!("orca {head} refused: {code}"))));
        }
        bail!("orca {head} exited {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_slice(&out.stdout).context("orca output is not JSON")
}

fn refusal(stdout: &[u8]) -> Option<String> {
    let reply: Value = serde_json::from_slice(stdout).ok()?;
    (reply["ok"] == false).then(|| {
        let error = &reply["error"];
        error["code"].as_str().or(error["message"].as_str()).unwrap_or("ok=false").to_string()
    })
}

/// Orca registers a worktree by its real path; a symlinked checkout path is `selector_not_found`.
pub(crate) fn worktree_selector(repo: &str) -> String {
    let real: PathBuf = std::fs::canonicalize(Path::new(repo)).unwrap_or_else(|_| repo.into());
    format!("path:{}", real.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_reads_the_error_code_from_stdout() {
        let reply = br#"{"id":"x","ok":false,"error":{"code":"selector_not_found","message":"selector_not_found"}}"#;
        assert_eq!(refusal(reply).as_deref(), Some("selector_not_found"));
        assert_eq!(refusal(br#"{"ok":true,"result":{}}"#), None);
        assert_eq!(refusal(b"not json"), None);
    }

    #[test]
    fn selector_resolves_a_symlinked_checkout() {
        let base = std::env::temp_dir().join(format!("orca-selector-{}", std::process::id()));
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let expected = format!("path:{}", std::fs::canonicalize(&real).unwrap().display());
        assert_eq!(worktree_selector(link.to_str().unwrap()), expected);
        std::fs::remove_dir_all(base).unwrap();
    }
}
