//! Retro series pages (founder, 2026-11-12): each recurring watchdog retro family publishes one
//! encrypted report page, slug `<family>-retro`, in the report index group `retro`, rebuilt from
//! every reviewed window's `analysis/<id>/report.json` and `incidents/<id>.json`, so a window adds a
//! section `W<id>` instead of a page of repeated goal, criteria and waivers.
//! `cpu-watchdog retro-series <family> [--out <dir>] [--no-publish]` writes `<dir>/spec.json`, builds
//! it with `mars-skill-tools report build` and publishes it with `report publish-dir --encrypt`.

mod canary;
pub(super) mod conversation;
mod search;

use crate::monitor::root;
use crate::retro_recall::dated::{sgt_day, ymd};
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

pub const FAMILIES: [&str; 3] = ["search", "canary", "conversation"];
const CELL_CHARS: usize = 160;

// A home path in page text trips the report publication guard; pages write it as `~/`.
static HOME_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/?[Uu]sers/[A-Za-z0-9_.-]+/").unwrap());

/// One reviewed window: its incident record and the retro session's report.json.
pub struct Window {
    pub id: String,
    pub at_ms: f64,
    pub incident: Value,
    pub report: Value,
    /// The retro session's Chinese report.md; empty when absent.
    pub md: String,
    /// analysis/<id>/repeats.json (per-signature call counts) and followups.json (per-probe rates); null when absent.
    pub counts: Value,
    pub probes: Value,
}

impl Window {
    pub fn date(&self) -> String {
        ymd(sgt_day(self.at_ms))
    }

    pub fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
        value[key].as_array().map_or(&[], Vec::as_slice)
    }
}

pub fn slug(family: &str) -> String {
    format!("{family}-retro")
}

/// The link a window's report.json `report_url` names when it has no page of its own. The
/// conversation page is organised by problem thread, not by window (founder 2026-11-15), so its
/// windows link the page itself.
pub fn window_url(family: &str, id: &str) -> String {
    if family == "conversation" {
        return format!("https://reports.example.com/{}/", slug(family));
    }
    format!("https://reports.example.com/{}/#W{id}", slug(family))
}

/// The route prompt's closing steps: report.json, the series page, then the ack.
pub fn instruction(family: &str, id: &str, state: &Path) -> String {
    format!(
        "write {s}/analysis/{id}/report.md in Chinese and report.json with report_url {url}, run cpu-watchdog retro-series {family} ({what}) and give that link; publish a separate {slug}-{id} page only when the founder must decide something; then run cpu-watchdog monitor-ack {id}.",
        s = state.display(), url = window_url(family, id), slug = slug(family),
        what = if family == "conversation" { format!("it republishes the series page {} by problem thread, this window's fixes as new versions", slug(family)) } else { format!("it republishes the series page {} with this window as section W{id}", slug(family)) }
    )
}

/// Table-safe text: a home path becomes `~/`, pipes are escaped, lines join, long text is cut.
pub fn cell(value: &Value) -> String {
    let text = plain(value);
    let flat = HOME_PATH.replace_all(&text, "~/").replace('|', "\\|").replace('\n', " ");
    if flat.chars().count() > CELL_CHARS { format!("{}…", flat.chars().take(CELL_CHARS).collect::<String>()) } else { flat }
}

/// A value as `key: value; …` text, never JSON: a cut JSON cell no longer parses, and the report
/// publication guard refuses a string that starts like JSON and does not parse
/// (`mars-skill-tools` report/guard/scan.rs; search-retro 812751635617774196739, whose object
/// `after` was cut to `{"replay":…,"status":"NOT-MEASU…` and stopped the series publish).
fn plain(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Object(m) => m.iter().map(|(k, v)| format!("{k}: {}", plain(v))).collect::<Vec<_>>().join("; "),
        Value::Array(a) => a.iter().map(plain).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// The page link of a window that had its own page before the series existed.
pub fn own_page(family: &str, w: &Window) -> Option<String> {
    w.report["report_url"].as_str().filter(|u| u.contains(&format!("/{}-{}/", slug(family), w.id))).map(|u| format!("原窗口页 {u}"))
}

/// An `xychart-beta` section of the last `count` windows; one `line` per named series.
pub fn trend(title: &str, windows: &[&Window], count: usize, unit: &str, series: &[(&str, &dyn Fn(&Window) -> Option<f64>)]) -> Value {
    let recent: Vec<&Window> = windows.iter().copied().filter(|w| series.iter().any(|(_, f)| f(w).is_some())).collect();
    let recent = &recent[recent.len().saturating_sub(count)..];
    if recent.is_empty() {
        return json!({"title": title, "body": ["没有可画的窗口数据"], "vizWaiver": "没有数据"});
    }
    let values: Vec<Vec<f64>> = series.iter().map(|(_, f)| recent.iter().map(|w| f(w).unwrap_or(0.0)).collect()).collect();
    let (low, high) = values.iter().flatten().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let names: Vec<&str> = series.iter().map(|(name, _)| *name).collect();
    let mut mermaid = format!("xychart-beta\n  title \"{title}（最近 {} 个窗口）\"\n", recent.len());
    if names.len() > 1 {
        mermaid = format!("xychart-beta\n  %% series: {}\n  title \"{title}（最近 {} 个窗口）\"\n", names.join(", "), recent.len());
    }
    let axis = (1..=recent.len()).map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
    mermaid.push_str(&format!("  x-axis [{axis}]\n  y-axis \"{unit}\" {} --> {}\n", low.floor().max(0.0) as i64, high.ceil() as i64 + 1));
    for row in &values {
        mermaid.push_str(&format!("  line [{}]\n", row.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(", ")));
    }
    let span = format!("横轴为窗口序号，1 = {}，{} = {}（最新）", recent[0].date(), recent.len(), recent[recent.len() - 1].date());
    json!({"title": title, "diagrams": [{"mermaid": mermaid.trim_end(), "caption": span}]})
}

/// A Markdown table section with header `columns`; `vizWaiver` because the numbers sit in the trend.
pub fn table(title: &str, columns: &[&str], rows: Vec<Vec<String>>) -> Value {
    let mut body = vec![format!("| {} |", columns.join(" | ")), format!("|{}|", vec!["---"; columns.len()].join("|"))];
    body.extend(rows.into_iter().map(|row| format!("| {} |", row.join(" | "))));
    json!({"title": title, "body": body, "vizWaiver": "逐条清单；数值变化见上方趋势图"})
}

/// Reviewed windows of `family`, oldest first: a report.json whose `report_url` names the family's
/// window page or series anchor, joined with its incident record.
fn windows(home: &Path, family: &str) -> Vec<Window> {
    let state = root(home);
    let marker = Regex::new(&format!("/{}[-/]", regex::escape(&slug(family)))).unwrap();
    let Ok(dirs) = std::fs::read_dir(state.join("analysis")) else { return Vec::new() };
    let read = |path: PathBuf| std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let mut list: Vec<Window> = dirs
        .flatten()
        .filter_map(|dir| {
            let id = dir.file_name().to_string_lossy().to_string();
            let report = read(dir.path().join("report.json")).filter(|r| r["report_url"].as_str().is_some_and(|u| marker.is_match(u)))?;
            let incident = read(state.join("incidents").join(format!("{id}.json")))?;
            let at_ms = incident["window"]["to_ms"].as_f64().or(incident["stats"]["window"]["to_ms"].as_f64()).or(incident["detected_at_ms"].as_f64())?;
            let md = std::fs::read_to_string(dir.path().join("report.md")).unwrap_or_default();
            let (counts, probes) = (read(dir.path().join("repeats.json")).unwrap_or(Value::Null), read(dir.path().join("followups.json")).unwrap_or(Value::Null));
            Some(Window { id, at_ms, incident, report, md, counts, probes })
        })
        .collect();
    list.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
    list
}

fn reviewed(family: &str, w: &Window) -> bool {
    match family {
        "search" => search::reviewed(w),
        "canary" => canary::reviewed(w),
        _ => conversation::reviewed(w),
    }
}

fn spec(family: &str, windows: &[&Window], home: &Path) -> Value {
    let mut spec = match family {
        "search" => search::spec(windows),
        "canary" => canary::spec(windows),
        _ => conversation::spec(windows, home),
    };
    let fixed = json!({"slug": slug(family), "date": windows[windows.len() - 1].date(), "group": "retro", "environment": "local", "runId": slug(family)});
    for (key, value) in fixed.as_object().into_iter().flatten() {
        spec[key] = value.clone();
    }
    // The conversation page carries one case per problem thread, built by its own spec.
    if family != "conversation" {
        spec["cases"] = json!(windows.iter().rev().map(|w| if family == "search" { search::case(w) } else { canary::case(w) }).collect::<Vec<_>>());
    }
    spec
}

fn run(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program).args(args).output().with_context(|| format!("{program} {}", args.join(" ")))?;
    if !out.status.success() {
        bail!("{program} {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// `cpu-watchdog retro-series <family> [--out <dir>] [--no-publish]`.
pub fn cli(home: &Path, args: Vec<String>) -> Result<()> {
    let family = args.first().filter(|f| FAMILIES.contains(&f.as_str())).with_context(|| format!("usage: cpu-watchdog retro-series <{}> [--out <dir>] [--no-publish]", FAMILIES.join("|")))?;
    let out = args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)).map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("{}-series", slug(family))));
    let all = windows(home, family);
    let list: Vec<&Window> = all.iter().filter(|w| reviewed(family, w)).collect();
    if list.is_empty() {
        bail!("no reviewed {} window under {}", slug(family), root(home).join("analysis").display());
    }
    std::fs::create_dir_all(&out)?;
    let spec_path = out.join("spec.json");
    std::fs::write(&spec_path, serde_json::to_string_pretty(&spec(family, &list, home))?)?;
    println!("spec {} ({} windows)", spec_path.display(), list.len());
    if args.iter().any(|a| a == "--no-publish") {
        return Ok(());
    }
    let tools = home.join(".local/bin/mars-skill-tools");
    let tools = tools.to_string_lossy();
    run(&tools, &["report", "build", &spec_path.to_string_lossy()])?;
    let published = run(&tools, &["report", "publish-dir", &out.join("report").to_string_lossy(), "--encrypt"])?;
    let url = published.lines().find_map(|l| l.split_whitespace().find(|w| w.starts_with("https://reports.example.com"))).unwrap_or("");
    println!("published {url}");
    Ok(())
}

#[cfg(test)]
#[path = "retro_series/tests.rs"]
mod tests;
