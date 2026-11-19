//! The capture tree: one locked, detached worktree of the Mars_agent repository kept under the state
//! directory and moved to the commit being measured, never the shared checkout. Its tracked files
//! are reset and cleaned before each capture; ignored `node_modules` stay so `bun install` is quick.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const REPO: &str = "/Volumes/MarsWork/Workspaces/Mars_agent";
pub const BRANCH: &str = "origin/mastra";
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(600);
/// The offline settings prompt-dedup's capture uses (no network, model or CRM call).
const ENV: [(&str, &str); 5] = [("MODEL_PROFILE", "google"), ("GOOGLE_GENERATIVE_AI_API_KEY", "offline"),
    ("AGENT_SEMANTIC_RECALL", "off"), ("AGENT_OBSERVATIONAL_MEMORY", "off"), ("AGENT_STORAGE_FALLBACK_PATH", ":memory:")];
pub const SKILL_DIR: &str = "Mars_CRM/agent/src/mastra/agents/skills";

pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).stdin(Stdio::null()).output()?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Fetches mastra and returns `(head sha, sha of the last commit at least `days` before now)`.
pub fn heads(repo: &Path, days: u64) -> Result<(String, String)> {
    git(repo, &["fetch", "--quiet", "origin", "mastra"])?;
    let head = git(repo, &["rev-parse", BRANCH])?;
    let before = format!("{} days ago", days);
    let week = git(repo, &["rev-list", "-1", &format!("--before={before}"), BRANCH])?;
    Ok((head, week))
}

/// Moves the capture tree to `sha` (creating and locking it on first use) and installs dependencies.
pub fn checkout(repo: &Path, tree: &Path, sha: &str) -> Result<()> {
    if !tree.join(".git").exists() {
        std::fs::create_dir_all(tree.parent().context("tree parent")?)?;
        git(repo, &["worktree", "add", "--detach", &tree.to_string_lossy(), sha])?;
        git(repo, &["worktree", "lock", "--reason", "prompt-review weekly capture tree", &tree.to_string_lossy()])?;
    }
    git(tree, &["checkout", "--quiet", "--detach", "--force", sha])?;
    git(tree, &["clean", "-ffdq"])?;
    let status = Command::new("bun").args(["install", "--frozen-lockfile"]).current_dir(tree.join("Mars_CRM"))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
    if !status.status.success() {
        bail!("bun install: {}", String::from_utf8_lossy(&status.stderr).lines().last().unwrap_or_default());
    }
    Ok(())
}

/// Captures the first Gemini request body of the production agent at the tree's commit.
pub fn capture(tree: &Path, body: &Path) -> Result<Value> {
    let agent = tree.join("Mars_CRM/agent");
    let script = tree.join(".claude/skills/cache-optimization/scripts/measure-prefix.ts");
    let log = std::fs::File::create(body.with_extension("log"))?;
    let mut child = Command::new("bun").arg(&script).arg("--agent").arg(&agent).arg("--body").arg(body)
        .envs(ENV).current_dir(&agent).stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn()?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? { break status; }
        if started.elapsed() > CAPTURE_TIMEOUT {
            let _ = child.kill();
            bail!("capture timed out after {} s", CAPTURE_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    if !status.success() {
        bail!("capture failed ({status}); see {}", body.with_extension("log").display());
    }
    Ok(serde_json::from_slice(&std::fs::read(body)?)?)
}

/// Bundled skill sources at the tree's commit: `(path relative to the repository, text)`.
pub fn skill_sources(tree: &Path) -> Result<Vec<(String, String)>> {
    let list = git(tree, &["ls-files", "--", SKILL_DIR])?;
    Ok(list.lines().filter(|p| p.ends_with(".ts")).filter_map(|p| std::fs::read_to_string(tree.join(p)).ok().map(|t| (p.to_string(), t))).collect())
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("PROMPT_REVIEW_STATE").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state/prompt-review"))
}
