//! Previews each new keyword pool proposal in Google Ads (founder 2026-11-19: proposals are previewed every hour and
//! shown in the hourly retro, and nothing is applied until the founder picks it there). A proposal file name carries
//! a hash of its rows (`cpc_pool_view::proposal`), so each distinct set is uploaded for preview once;
//! `<state>/cpc-pool/previews.json` keeps every file's result.

use super::files::{read_json, stringify, write_text};
use serde_json::{json, Map, Value};
use std::process::Command;

const RUNNER: &str = ".claude/skills/agent-browser/scripts/lease-page.js";
const STEPS: &str = ".claude/skills/seo-competitor/scripts/ads-bulk.js";
const KINDS: [(&str, &str); 2] = [("add", "加词"), ("pause", "暂停")];

/// Previews one file with `ads-bulk.js` (no `ADS_APPLY`) and returns its `<out>/bulk-<name>.json`.
pub type Previewer<'a> = &'a dyn Fn(&str, &str) -> Result<Value, String>;

fn path(state_dir: &str) -> String { format!("{state_dir}/cpc-pool/previews.json") }
fn name(file: &str) -> String { std::path::Path::new(file).file_name().map_or_else(|| file.to_string(), |n| n.to_string_lossy().into_owned()) }

fn upload(file: &str, out: &str) -> Result<Value, String> {
    let run = Command::new("bun")
        .args([RUNNER, "--label", "ads-bulk", "--out", out, "--profile", "9223", "--timeout", "70000", "--budget", "420000", STEPS])
        .env("ADS_UPLOAD_FILE", file)
        .env_remove("ADS_APPLY")
        .output()
        .map_err(|e| format!("bun: {e}"))?;
    read_json(&format!("{out}/bulk-{}.json", name(file))).map_err(|error| {
        let tail = String::from_utf8_lossy(&run.stderr).lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        format!("ads-bulk.js exited {} without a result ({error}): {tail}", run.status)
    })
}

/// Who reads this run's preview rows: the hourly watch (`Retro`), whose retro asks the founder about each preview once,
/// or a manual `cpcPool` run (`Manual`), which shows them without using up that question.
#[derive(Clone, Copy, PartialEq)]
pub enum Reader { Retro, Manual }

/// Adds `proposals.preview` to the pool summary: one row per proposal file; `fresh` marks a preview no hourly retro
/// was shown yet (`shown` in `previews.json` is set by the first watch run that reports it).
pub fn run(summary: &mut Value, state_dir: &str, run_dir: &str, at: &str, reader: Reader, dry_run: bool, errors: &mut Vec<String>) {
    run_with(summary, state_dir, run_dir, (at, reader), dry_run, errors, &upload);
}

pub fn run_with(summary: &mut Value, state_dir: &str, run_dir: &str, (at, reader): (&str, Reader), dry_run: bool, errors: &mut Vec<String>, previewer: Previewer) {
    let mut known: Map<String, Value> = read_json(&path(state_dir)).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let mut rows = Vec::new();
    let mut changed = false;
    for (kind, _) in KINDS {
        let Some(file) = summary["proposals"][kind].as_str().map(String::from) else { continue };
        let key = name(&file);
        if let Some(done) = known.get_mut(&key) {
            let fresh = done["shown"].is_null() || done["shown"] == at;
            if done["shown"].is_null() && reader == Reader::Retro && !dry_run { done["shown"] = json!(at); changed = true; }
            rows.push(row(kind, done, fresh));
            continue;
        }
        if dry_run { rows.push(json!({"kind": kind, "file": file, "skipped": "dry run"})); continue; }
        let out = format!("{run_dir}/ads-preview");
        match std::fs::create_dir_all(&out).map_err(|e| e.to_string()).and_then(|_| previewer(&file, &out)) {
            Ok(result) => {
                let shown = if reader == Reader::Retro { json!(at) } else { Value::Null };
                let entry = json!({"file": file, "at": at, "shown": shown, "ok": result["ok"], "validChanges": result["validChanges"], "preview": result["preview"], "results": result["results"]});
                rows.push(row(kind, &entry, true));
                known.insert(key, entry);
                changed = true;
            }
            Err(error) => {
                errors.push(format!("cpc pool preview {key}: {error}"));
                rows.push(json!({"kind": kind, "file": file, "error": error}));
            }
        }
    }
    let save = || std::fs::create_dir_all(format!("{state_dir}/cpc-pool")).map_err(|e| e.to_string())
        .and_then(|_| stringify(&Value::Object(known.clone()))).and_then(|body| write_text(&path(state_dir), &body));
    if changed && let Err(error) = save() {
        errors.push(format!("cpc pool previews: {error}"));
    }
    summary["proposals"]["preview"] = json!(rows);
}

fn row(kind: &str, entry: &Value, fresh: bool) -> Value {
    let mut row = entry.clone();
    row["kind"] = json!(kind);
    row["fresh"] = json!(fresh);
    row
}

/// The proposal lines of the watch page section: the words, then each file's preview result; never applied here.
pub fn section(proposals: &Value) -> String {
    let mut out = String::new();
    for (kind, label) in KINDS {
        let words: Vec<String> = proposals[format!("{kind}Rows")].as_array().into_iter().flatten()
            .map(|r| format!("{}（{} / {}）", r["keyword"].as_str().unwrap_or(""), r["campaign"].as_str().unwrap_or(""), r["adGroup"].as_str().unwrap_or(""))).collect();
        if words.is_empty() { continue; }
        let preview = proposals["preview"].as_array().into_iter().flatten().find(|p| p["kind"] == kind);
        let state = match preview {
            None => "未预览".to_string(),
            Some(p) if p["error"].is_string() => format!("预览失败：{}", p["error"].as_str().unwrap_or("")),
            Some(p) if p["skipped"].is_string() => "演练未预览".to_string(),
            Some(p) => {
                let when = format!("{} ", p["at"].as_str().unwrap_or("").get(11..16).unwrap_or(""));
                let valid = p["validChanges"].as_u64().map_or(String::new(), |n| format!("，{n} 条有效"));
                format!("{when}在 Ads 预览{}{valid}", if p["ok"] == true { "通过" } else { "有错误" })
            }
        };
        out.push_str(&format!("提案{label} {} 个：{}。{state}；未应用，等你在每小时复盘里确认。\n\n", words.len(), words.join("、")));
    }
    out
}
