//! Git and Orca reads for the release snapshot. Commit relations are immutable, so they are cached
//! for the process lifetime; `behind origin/mastra` moves with fetches and is cached briefly.
use super::snapshot::Git;
use std::{cell::RefCell, collections::{BTreeSet, HashMap}, process::{Command, Stdio}, time::{Duration, Instant}};

const REPO: &str = "/path/to/workspace";
const BEHIND_TTL: Duration = Duration::from_secs(300);

#[derive(Default)]
pub(crate) struct RepoGit {
    relations: RefCell<HashMap<(String, String), (&'static str, Vec<String>)>>,
    behind: RefCell<HashMap<String, (Instant, Option<u64>)>>,
    pushed: RefCell<HashMap<String, (Instant, bool)>>,
    carried: RefCell<HashMap<String, (Instant, bool)>>,
}

fn git(cwd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("/usr/bin/git").arg("-C").arg(cwd).args(args)
        .stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn changed(from: &str, to: &str) -> BTreeSet<String> {
    git(REPO, &["diff", "--name-only", &format!("{from}..{to}")]).unwrap_or_default().lines().map(str::to_string).collect()
}

/// Paths changed by `head`'s commits since `base` that have no patch-equal commit on `other`'s
/// side (`git cherry`), so a rebased copy of the same change on both sides is not counted twice.
fn unique_changes(base: &str, head: &str, other: &str) -> BTreeSet<String> {
    let Some(cherry) = git(REPO, &["cherry", other, head, base]) else { return changed(base, head) };
    let own: Vec<&str> = cherry.lines().filter_map(|l| l.strip_prefix("+ ")).map(str::trim).collect();
    if own.is_empty() { return BTreeSet::new(); }
    let mut args = vec!["log", "--no-walk", "--name-only", "--format=", "--diff-merges=first-parent"];
    args.extend(own);
    match git(REPO, &args) {
        Some(out) => out.lines().filter(|l| !l.is_empty()).map(str::to_string).collect(),
        None => changed(base, head),
    }
}

impl Git for RepoGit {
    fn resolve(&self, cwd: &str, rev: &str) -> Option<String> {
        let dir = if std::path::Path::new(cwd).is_dir() { cwd } else { REPO };
        git(dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
    }

    fn prefix(&self, cwd: &str) -> String {
        git(cwd, &["rev-parse", "--show-prefix"]).unwrap_or_default()
    }

    /// `same`, `contains` (a carries b), `contained` (b carries a) or `divergent` with the paths
    /// both sides changed since their merge base.
    fn relation(&self, a: &str, b: &str) -> (&'static str, Vec<String>) {
        let key = (a.to_string(), b.to_string());
        if let Some(hit) = self.relations.borrow().get(&key) { return hit.clone(); }
        let value = match git(REPO, &["merge-base", a, b]) {
            Some(base) if base == b => ("contains", Vec::new()),
            Some(base) if base == a => ("contained", Vec::new()),
            Some(base) => ("divergent", unique_changes(&base, a, b).intersection(&unique_changes(&base, b, a)).cloned().collect()),
            None => ("unknown", Vec::new()),
        };
        self.relations.borrow_mut().insert(key, value.clone());
        value
    }

    fn behind(&self, sha: &str) -> Option<u64> {
        if let Some((at, n)) = self.behind.borrow().get(sha) { if at.elapsed() < BEHIND_TTL { return *n; } }
        let n = git(REPO, &["rev-list", "--count", &format!("{sha}..origin/mastra")]).and_then(|t| t.parse().ok());
        self.behind.borrow_mut().insert(sha.to_string(), (Instant::now(), n));
        n
    }

    /// A pushed commit stays pushed, so `true` is cached for good and `false` only briefly.
    fn pushed(&self, sha: &str) -> bool {
        if let Some((at, yes)) = self.pushed.borrow().get(sha) { if *yes || at.elapsed() < BEHIND_TTL { return *yes; } }
        let yes = git(REPO, &["merge-base", "--is-ancestor", sha, "origin/mastra"]).is_some();
        self.pushed.borrow_mut().insert(sha.to_string(), (Instant::now(), yes));
        yes
    }

    /// `origin/mastra` moves, so both answers are cached only briefly.
    fn carried(&self, sha: &str, paths: &[String]) -> bool {
        if paths.is_empty() { return false; }
        let key = format!("{sha}\0{}", paths.join("\0"));
        if let Some((at, yes)) = self.carried.borrow().get(&key) { if at.elapsed() < BEHIND_TTL { return *yes; } }
        let mut args = vec!["diff", "--quiet", sha, "origin/mastra", "--"];
        args.extend(paths.iter().map(String::as_str));
        let yes = git(REPO, &args).is_some();
        self.carried.borrow_mut().insert(key, (Instant::now(), yes));
        yes
    }
}

/// Orca tab id → title and writable terminal handle, so agents in the queue can message each other.
pub(crate) fn orca_tabs() -> super::snapshot::Tabs {
    if cfg!(test) { return HashMap::new(); }
    crate::dispatch::orca(&["terminal", "list", "--json"]).ok()
        .and_then(|v| v["result"]["terminals"].as_array().cloned()).unwrap_or_default().iter()
        .filter(|t| t["writable"] != false)
        .filter_map(|t| Some((t["tabId"].as_str()?.to_string(), serde_json::json!({"title": t["title"], "terminal": t["handle"]})))).collect()
}
