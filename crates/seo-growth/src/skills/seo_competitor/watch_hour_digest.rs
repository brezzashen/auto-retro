//! `mars-skill-tools seo-competitor hourDigest --run <run_dir> [--state <state_dir>]`: the inputs of the
//! hourly retro in one print, so the tab reads them once instead of walking the 1 MB `analysis.json`
//! with a dozen jq calls. Blocks too large to print whole are named with the jq path that reads them.

use super::files::read_json;
use super::watch_trends::{Pull, needs_judgment};
use super::watch_trends_ledger::labels;
use chrono::DateTime;
use serde_json::Value;

const USAGE: &str = "Usage: mars-skill-tools seo-competitor hourDigest --run <run_dir> [--state <state_dir>]";

fn arg(argv: &[String], name: &str) -> Option<String> {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

fn compact(v: &Value) -> String { serde_json::to_string(v).unwrap_or_default() }

/// One compact JSON line per array item, or `<label>: null|[]` when there is nothing to print.
fn rows(out: &mut Vec<String>, label: &str, v: &Value) {
    match v.as_array() {
        Some(items) if !items.is_empty() => { out.push(format!("{label} ({}):", items.len())); out.extend(items.iter().map(compact)); }
        _ => out.push(format!("{label}: {}", compact(v))),
    }
}

fn traffic(a: &Value, out: &mut Vec<String>) {
    out.push("## traffic".into());
    let t = &a["traffic"];
    if t.is_null() { out.push("traffic: null（本轮 PostHog 失败，原因见 errors）".into()); return; }
    for key in ["segments_error", "payments_error"] {
        if !t[key].is_null() { out.push(format!("{key}: {}", compact(&t[key]))); }
    }
    for (window, offset, name) in [(1, 0, "本小时"), (1, 1, "前一小时"), (1, 24, "昨天同一小时"), (24, 0, "近 24 小时")] {
        let picked: Vec<&Value> = t["segments"].as_array().into_iter().flatten()
            .filter(|s| s["window_hours"] == window && s["offset_hours"] == offset).collect();
        out.push(format!("segments {name}（window_hours {window}, offset_hours {offset}，{} 行）:", picked.len()));
        out.extend(picked.into_iter().map(compact));
    }
    rows(out, "sources", &t["sources"]);
    rows(out, "batches", &t["batches"]);
    rows(out, "payments", &t["payments"]);
    let hour = &t["hour"];
    for key in ["visitors_error", "db_error"] {
        if !hour[key].is_null() { out.push(format!("hour.{key}: {}", compact(&hour[key]))); }
    }
    rows(out, "hour.visitors", &hour["visitors"]);
    let db = &hour["db"];
    out.push(format!("hour.db.counts: {}", compact(&db["counts"])));
    rows(out, "hour.db.orders", &db["orders"]);
    rows(out, "hour.db.subscription_changes", &db["subscription_changes"]);
    let followup = &db["followup"];
    out.push(format!("hour.db.followup: {}", compact(&serde_json::json!({
        "from": followup["from"], "to": followup["to"], "signups": followup["signups"], "chatted": followup["chatted"], "paid": followup["paid"] }))));
    rows(out, "hour.db.followup.accounts", &followup["accounts"]);
}

fn first_incomplete(a: &Value) -> Option<DateTime<chrono::FixedOffset>> {
    a["gsc"]["hourly"]["metadata"]["firstIncompleteHour"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok())
}

/// The newest earlier run directory with an `analysis.json`, by its `YYYY-MM-DDTHH` name.
fn previous_run(run_dir: &str) -> Option<String> {
    let path = std::path::Path::new(run_dir.trim_end_matches('/'));
    let name = path.file_name()?.to_string_lossy().to_string();
    let mut earlier: Vec<String> = std::fs::read_dir(path.parent()?).ok()?.filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| *n < name).collect();
    earlier.sort();
    earlier.into_iter().rev().map(|n| format!("{}/{n}", path.parent().unwrap().display()))
        .find(|dir| std::path::Path::new(&format!("{dir}/analysis.json")).exists())
}

/// Search Console hours completed since the previous run (Search Console trails the clock by about 14 hours).
fn gsc(a: &Value, run_dir: &str, out: &mut Vec<String>) {
    out.push("## gsc".into());
    let current = first_incomplete(a);
    let prev_dir = previous_run(run_dir);
    let prev = prev_dir.as_ref().and_then(|d| read_json(&format!("{d}/analysis.json")).ok()).and_then(|p| first_incomplete(&p));
    out.push(format!("firstIncompleteHour: 本轮 {}，上一轮 {}（{}）",
        current.map(|d| d.to_rfc3339()).unwrap_or("无".into()), prev.map(|d| d.to_rfc3339()).unwrap_or("无".into()), prev_dir.unwrap_or("无上一轮".into())));
    let (Some(cur), Some(prev)) = (current, prev) else { return };
    if cur <= prev { out.push("GSC 本小时无新数据（firstIncompleteHour 没前进）".into()); return; }
    let fresh: Vec<&Value> = a["gsc"]["hourly"]["rows"].as_array().into_iter().flatten().filter(|r| {
        r["keys"][0].as_str().and_then(|k| DateTime::parse_from_rfc3339(k).ok()).is_some_and(|k| k >= prev && k < cur)
    }).collect();
    out.push(format!("新完成的小时（{} 行，keys 是太平洋时间）:", fresh.len()));
    out.extend(fresh.into_iter().map(compact));
}

fn search_and_signals(a: &Value, out: &mut Vec<String>) {
    out.push("## anomalies".into());
    rows(out, "anomalies", &a["anomalies"]);
    out.push("## findings（P0/P1 全列，P2 只计数；全部：jq '.findings[]'）".into());
    let all = a["findings"].as_array().cloned().unwrap_or_default();
    let p2 = all.iter().filter(|f| f["severity"] == "P2").count();
    out.extend(all.iter().filter(|f| f["severity"] != "P2").map(|f| compact(&serde_json::json!({"severity": f["severity"], "key": f["key"], "title": f["title"]}))));
    out.push(format!("P2: {p2} 条"));
    out.push("## geo".into());
    for key in ["hour", "skipped", "unmeasured", "spentTodayUsd"] { out.push(format!("geo.{key}: {}", compact(&a["geo"][key]))); }
    rows(out, "geo.records", &a["geo"]["records"]);
    out.push("## aisaSeo（overview、aiMode、props 按词读：jq '.aisaSeo.props[\"<词>\"]'）".into());
    for key in ["skipped", "spentTodayUsd", "aiModeRun", "links"] { out.push(format!("aisaSeo.{key}: {}", compact(&a["aisaSeo"][key]))); }
    out.push("## creatorPosts".into());
    out.push(compact(&a["creatorPosts"]));
    let ads = &a["googleAds"];
    out.push("## googleAds（关键词行：jq '.googleAds.tabs.keywords_today'）".into());
    out.push(format!("source {} · updated {} · failed {}", compact(&ads["source"]), compact(&ads["updated"]), compact(&ads["failed"])));
    for tab in ["campaigns_today", "campaigns_hourly", "campaigns_7d"] {
        if !ads["tabs"][tab].is_null() { rows(out, tab, &ads["tabs"][tab]); }
    }
    let pool = &a["cpcPool"];
    out.push("## cpcPool（带标记的词：jq '.cpcPool.flaggedRows'）".into());
    for key in ["counts", "expand", "newFlags", "proposals"] { out.push(format!("cpcPool.{key}: {}", compact(&pool[key]))); }
    out.push("## errors".into());
    out.push(compact(&a["errors"]));
}

/// Trends risers the retro still has to judge (step 5), with their sort numbers.
fn trends(state_dir: &str, out: &mut Vec<String>) {
    out.push(format!("## 待判断的异常上升词（{state_dir}/trends/latest.json 减去 trend-labels.jsonl 已判的词）"));
    let pull = read_json(&format!("{state_dir}/trends/latest.json"))
        .and_then(|v| serde_json::from_value::<Pull>(v).map_err(|e| e.to_string()));
    let pull = match pull { Ok(pull) => pull, Err(error) => { out.push(format!("trends/latest.json 不可读：{error}")); return; } };
    let judged = labels(state_dir);
    let waiting: Vec<String> = pull.rising.iter().filter(|r| needs_judgment(r) && !judged.contains_key(&r.query.trim().to_lowercase()))
        .map(|r| serde_json::to_string(r).unwrap_or_default()).collect();
    out.push(format!("拉取时间 {}，待判断 {} 个:", pull.at, waiting.len()));
    out.extend(waiting);
}

/// The newest summary under `<state>/retro/` and where the copied report.md section starts.
fn context(run_dir: &str, state_dir: &str, out: &mut Vec<String>) {
    out.push("## 上一轮复盘摘要".into());
    let newest = std::fs::read_dir(format!("{state_dir}/retro")).ok().into_iter().flatten().filter_map(Result::ok)
        .map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).max();
    match newest.and_then(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t))) {
        Some((path, text)) => { out.push(path.display().to_string()); out.push(text.trim().into()); }
        None => out.push("无".into()),
    }
    let report = format!("{run_dir}/report.md");
    let lines: Vec<String> = std::fs::read_to_string(&report).unwrap_or_default().lines().map(String::from).collect();
    let start = lines.iter().position(|l| l.starts_with("## 访客类型分布"));
    let end = start.and_then(|s| lines.iter().skip(s + 1).position(|l| l.starts_with("## ")).map(|n| s + n + 1));
    out.push(match (start, end) {
        (Some(s), Some(e)) => format!("「## 访客类型分布」整节：sed -n '{},{}p' {report}", s + 1, e),
        (Some(s), None) => format!("「## 访客类型分布」整节：sed -n '{},$p' {report}", s + 1),
        _ => format!("{report} 没有「## 访客类型分布」一节"),
    });
}

pub fn digest(run_dir: &str, state_dir: &str) -> Result<String, String> {
    let a = read_json(&format!("{run_dir}/analysis.json"))?;
    let mut out = vec![format!("# hourDigest {run_dir}（生成于 {}）", compact(&a["at"]))];
    traffic(&a, &mut out);
    gsc(&a, run_dir, &mut out);
    search_and_signals(&a, &mut out);
    trends(state_dir, &mut out);
    context(run_dir, state_dir, &mut out);
    Ok(out.join("\n"))
}

pub fn main(argv: &[String]) -> i32 {
    if argv.iter().any(|a| a == "-h" || a == "--help") { println!("{USAGE}"); return 0; }
    let Some(run_dir) = arg(argv, "--run") else { eprintln!("missing --run\n{USAGE}"); return 2 };
    let home = std::env::var("HOME").unwrap_or_default();
    let state_dir = arg(argv, "--state").unwrap_or_else(|| format!("{home}/.local/state/seo-watch"));
    match digest(&run_dir, &state_dir) {
        Ok(text) => { println!("{text}"); 0 }
        Err(error) => { eprintln!("hourDigest: {error}"); 1 }
    }
}
