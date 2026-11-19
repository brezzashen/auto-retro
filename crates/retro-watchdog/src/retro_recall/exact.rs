//! The exact half of retro recall. Seeds come from the counts the watch already took
//! deterministically: the failing commands themselves, scanned with `local-search tools` across
//! transcript tool calls, so recall finds the same invocation in other sessions. Their output
//! was the seed until 2026-11-02, but the history corpus never indexes tool calls or output, so
//! those queries could not match (5 of 5 empty, recall retro 813940471853705769812). Every
//! recalled or cited `path:line` is read back from its file.
use serde_json::{Value, json};
use std::{collections::{BTreeMap, HashSet}, io::BufRead, path::{Path, PathBuf}};

const SEEDS: usize = 8;
const SEED_WORDS: usize = 4;
/// Shell plumbing that names no tool: dropped so the seed keeps the program and its subcommand.
const SHELL_WORDS: [&str; 17] = ["cd", "export", "echo", "ls", "mkdir", "rm", "cat", "tail", "head", "grep", "tee", "sleep", "true", "then", "do", "done", "env"];

/// The seed of a command: its first run of two or more adjacent plain words (program names,
/// subcommands, `--long` flags) as a quoted phrase, which `tools` matches verbatim; else its
/// first plain words, matched each on its own. Assignments, paths, quoted text, numbers, ids
/// and shell plumbing are never words. Separate words alone let `bun run deliver ios` match a
/// `deliver beta-web` call that mentions iOS in its purpose.
pub(super) fn command_words(command: &str) -> Option<String> {
    let (mut words, mut run): (Vec<String>, Vec<&str>) = (Vec::new(), Vec::new());
    for t in command.split(' ') {
        let plain = (t.starts_with(|c: char| c.is_ascii_alphabetic()) || t.starts_with("--") && t.len() > 2)
            && t.chars().all(|c| c.is_ascii_alphabetic() || matches!(c, '-' | '_' | '.' | ':'))
            && !SHELL_WORDS.contains(&t.to_ascii_lowercase().as_str());
        if plain && run.len() < SEED_WORDS {
            run.push(t);
            let word = t.to_ascii_lowercase();
            if !words.contains(&word) && words.len() < SEED_WORDS { words.push(word); }
            continue;
        }
        if run.len() >= 2 { break; }
        run.clear();
    }
    if run.len() >= 2 { return Some(format!("\"{}\"", run.join(" ").to_ascii_lowercase())); }
    (words.len() >= 2).then(|| words.join(" "))
}

/// The `tools` words of a seed: a quoted phrase is one word, anything else splits on spaces.
pub(super) fn seed_args(seed: &str) -> Vec<&str> {
    match seed.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(phrase) => vec![phrase],
        None => seed.split_whitespace().collect(),
    }
}

fn push(out: &mut Vec<(String, String)>, source: String, command: Option<&str>) {
    if let Some(words) = command.and_then(command_words) {
        if out.len() < SEEDS && !out.iter().any(|(_, w)| *w == words) { out.push((source, words)); }
    }
}

/// `(source, words)` tool-call scans from the incident's exact counts: release commands the
/// classifier left unclassified first, then one example per instruction-error class.
pub(super) fn seeds(incident: &Value) -> Vec<(String, String)> {
    let commands = &incident["stats"]["commands"];
    let mut out = Vec::new();
    for e in commands["unclassified"]["examples"].as_array().into_iter().flatten() {
        push(&mut out, "commands.unclassified".into(), e["command"].as_str());
    }
    for (class, group) in commands["classes"].as_object().into_iter().flatten() {
        push(&mut out, format!("commands.classes.{class}"), group["examples"][0]["command"].as_str());
    }
    out
}

/// Transcript `path:line` hits of a `local-search tools` scan. Its stdout holds hit lines
/// (`  <path:line>  [<Tool>] …`) or one `NONE: no tool call …` line; anything else is not a scan.
pub(super) fn tool_hits(text: &str) -> anyhow::Result<HashSet<String>> {
    let found: HashSet<String> = text.lines().filter_map(|l| {
        let mut t = l.split_whitespace();
        let (place, tool) = (t.next()?, t.next()?);
        (place.starts_with('/') && tool.starts_with('[') && place.rsplit_once(':').is_some_and(|(_, n)| n.parse::<usize>().is_ok_and(|n| n > 0)))
            .then(|| place.to_owned())
    }).collect();
    anyhow::ensure!(!found.is_empty() || text.lines().any(|l| l.starts_with("NONE: no tool call")),
        "seed scan did not run through local-search tools");
    Ok(found)
}

/// `path:line` or `path:line-end`, absolute or `~/`.
fn split(home: &Path, locator: &str) -> Option<(PathBuf, usize)> {
    let (path, line) = locator.rsplit_once(':')?;
    let line = line.split('-').next()?.parse::<usize>().ok().filter(|n| *n > 0)?;
    let path = path.strip_prefix("~/").map_or_else(|| PathBuf::from(path), |rest| home.join(rest));
    path.is_absolute().then_some((path, line))
}

/// The wanted 1-based lines of one file; a missing file or line is simply absent.
fn lines(path: &Path, wanted: &[usize]) -> BTreeMap<usize, String> {
    let (Some(&last), Ok(file)) = (wanted.iter().max(), std::fs::File::open(path)) else { return BTreeMap::new() };
    std::io::BufReader::new(file).split(b'\n').map_while(Result::ok).take(last).enumerate()
        .filter(|(i, _)| wanted.contains(&(i + 1)))
        .map(|(i, l)| (i + 1, String::from_utf8_lossy(&l).into_owned())).collect()
}

/// Each locator read back with its own timestamp, so a semantic neighbour from before the
/// window is background, never an occurrence inside it.
pub(super) fn read_back(home: &Path, locators: &[String], window: &Value) -> Vec<Value> {
    let mut by_file: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (path, n) in locators.iter().filter_map(|l| split(home, l)) { by_file.entry(path).or_default().push(n); }
    let read: BTreeMap<_, _> = by_file.iter().map(|(p, ns)| (p.clone(), lines(p, ns))).collect();
    let (from, to) = (window["from_ms"].as_f64(), window["to_ms"].as_f64());
    locators.iter().map(|l| {
        let line = split(home, l).and_then(|(p, n)| read.get(&p)?.get(&n).cloned());
        let at = line.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()).and_then(|v| line_ms(&v));
        let in_window = match (at, from, to) { (Some(t), Some(f), Some(e)) => json!(t >= f && t <= e), _ => Value::Null };
        json!({"locator": l, "exists": line.is_some(), "at_ms": at, "in_window": in_window})
    }).collect()
}

/// Epoch ms of one transcript line: Claude and Codex lines carry `timestamp`, Antigravity brain
/// transcripts (`~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`)
/// carry `created_at`. Before, every brain hit read back `at_ms: null, in_window: null` (993 of
/// 18,625 read-back hits across all receipts on 2026-11-11, all of them brain lines; search-retro
/// 8242256107124251741 could not place a1b92138…transcript.jsonl:134 in its window).
fn line_ms(v: &Value) -> Option<f64> {
    v["timestamp"].as_str().or_else(|| v["created_at"].as_str()).and_then(crate::telemetry::transcript_ms)
}

#[cfg(test)]
mod line_ms_tests {
    use serde_json::json;

    #[test]
    fn a_brain_line_is_placed_by_created_at() {
        let brain = json!({"step_index": 133, "type": "PLANNER_RESPONSE", "created_at": "2026-11-10T16:56:18Z"});
        assert_eq!(super::line_ms(&brain), Some(1_794_329_778_000.0));
        let claude = json!({"type": "assistant", "timestamp": "2026-11-10T16:56:18.500Z", "created_at": "2020-01-01T00:00:00Z"});
        assert_eq!(super::line_ms(&claude), Some(1_794_329_778_500.0), "timestamp wins when both exist");
        assert_eq!(super::line_ms(&json!({"type": "summary"})), None);
    }
}

/// A cited `path:line` that names a real line of a real file.
pub(super) fn exists(home: &Path, locator: &str) -> bool {
    split(home, locator).is_some_and(|(path, n)| lines(&path, &[n]).contains_key(&n))
}
