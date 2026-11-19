//! Builds the bounded, objective evidence pack for `mars-skill-tools seo-competitor advisor`.
//! Reads `<run_dir>/analysis.json` (plus recent `history.jsonl` and `keyword-vetoes.jsonl` when
//! `state_dir` is present) and backfills `Anomaly` items on the fly if `analysis["anomalies"]` is
//! absent so both live runs and historical runs receive identical structured anomaly signals.

use super::files::read_json;
use super::watch_analyze::{QueryRow, Window};
use super::watch_anomalies::{self, Anomaly};
use super::watch_config::read_state;
use super::watch_rank::RankRow;
use super::watch_rank_history;
use super::watch_rank_run::Ranks;
use serde_json::Value;

const DEFAULT_SITE: &str = "https://example.com";

fn f(v: &Value, key: &str) -> f64 { v[key].as_f64().unwrap_or(0.0) }
fn s<'a>(v: &'a Value, key: &str) -> &'a str { v[key].as_str().unwrap_or_default() }

/// Reads saved `anomalies` from `analysis.json`, or computes the core subset from raw saved fields.
pub fn anomalies_of(analysis: &Value, state_dir: Option<&str>) -> Vec<Anomaly> {
    if let Some(saved) = analysis.get("anomalies").and_then(|v| serde_json::from_value::<Vec<Anomaly>>(v.clone()).ok()) {
        return saved;
    }
    let site = DEFAULT_SITE;
    let gsc_pages = &analysis["gsc"]["pages"];
    let query_rows = super::watch_analyze::query_rows(&analysis["gsc"]["queryPage"]);
    let mut out = Vec::new();
    out.extend(watch_anomalies::page_ctr_anomalies(gsc_pages, &query_rows, &Default::default(), site));
    out.extend(watch_anomalies::cannibal_anomalies(&query_rows, site));
    let traffic_json = &analysis["traffic"];
    let segments = traffic_json["segments"].as_array().map(|arr| arr.iter().map(|r| super::watch_segments::SegmentRow {
        window_hours: f(r, "window_hours") as u32, offset_hours: f(r, "offset_hours") as u32,
        source: s(r, "source").into(), segment: s(r, "segment").into(),
        visitors: f(r, "visitors"), cta: f(r, "cta"), signups: f(r, "signups"), senders: f(r, "senders"),
    }).collect()).unwrap_or_default();
    let batches = traffic_json["batches"].as_array().map(|arr| arr.iter().map(|r| super::watch_segments::BatchRow {
        os: s(r, "os").into(), browser: s(r, "browser").into(), timezone: s(r, "timezone").into(),
        visitors: f(r, "visitors"), pageview_only: f(r, "pageview_only"),
        first_seen: s(r, "first_seen").into(), last_seen: s(r, "last_seen").into(),
        ips: s(r, "ips").into(), landings: s(r, "landings").into(),
    }).collect()).unwrap_or_default();
    let traffic = super::watch_traffic::Traffic { segments, batches, ..Default::default() };
    out.extend(watch_anomalies::traffic_anomalies(Some(&traffic)));
    if let Some(dir) = state_dir {
        let state = read_state(&format!("{dir}/state.json"));
        let urls: Vec<String> = analysis["probes"].as_array().into_iter().flatten()
            .filter_map(|p| p["url"].as_str().map(str::to_string)).collect();
        out.extend(watch_anomalies::canonical_anomalies(&urls, &state, site));
        if let Some(rows) = analysis.get("ranks").and_then(|v| serde_json::from_value::<Vec<RankRow>>(v.clone()).ok()) {
            let ranks = Ranks { rows, history: watch_rank_history::read(dir) };
            let mut bing = super::watch_bing::BingView::default();
            if let Some(map) = analysis["bing"]["ranks"].as_object() {
                for (k, v) in map {
                    if let Ok(wr) = serde_json::from_value(v.clone()) { bing.ranks.insert(k.clone(), wr); }
                }
            }
            if let Some(map) = analysis["bing"]["crawled"].as_object() {
                for (k, v) in map { bing.crawled.insert(k.clone(), v.as_str().map(str::to_string)); }
            }
            out.extend(watch_anomalies::engine_gap_anomalies(Some(&ranks), Some(&bing)));
        }
    }
    let _ = (Window::default(), QueryRow::default());
    out
}

fn format_overview(a: &Value) -> String {
    let (l_imp, l_clk) = (f(&a["last24h"], "impressions"), f(&a["last24h"], "clicks"));
    let (p_imp, p_clk) = (f(&a["previous24h"], "impressions"), f(&a["previous24h"], "clicks"));
    let end = a["gsc"]["hourly"]["metadata"]["firstIncompleteHour"].as_str().unwrap_or("未知");
    let (idx, tot) = (a["indexed"][0].as_u64().unwrap_or(0), a["indexed"][1].as_u64().unwrap_or(0));
    let seg_sum = |off: u64, key: &str| -> f64 {
        a["traffic"]["segments"].as_array().into_iter().flatten()
            .filter(|r| r["window_hours"].as_u64() == Some(24) && r["offset_hours"].as_u64() == Some(off))
            .map(|r| f(r, key)).sum()
    };
    let visitors = match segments_failure(&a["traffic"]) {
        Some(reason) => format!("PostHog 访客分类本轮未读到（{reason}）：这是读取失败，不是 0 人，不得据此判断埋点断流；按天真实访客：{}", day_visitors(&a["traffic"])),
        None => format!("PostHog 近 24h 生产真实访客：{:.0} 人（前 24h {:.0} 人），点 CTA {:.0} 人，新注册 {:.0} 人，发消息 {:.0} 人",
            seg_sum(0, "visitors"), seg_sum(24, "visitors"), seg_sum(0, "cta"), seg_sum(0, "signups"), seg_sum(0, "senders")),
    };
    format!(
        "## 1. 核心搜索与转化概览（时间：{}）\n- GSC 最近 24 个完整小时（截至 {end}）：展示 {l_imp:.0}、点击 {l_clk:.0}（前 24h 展示 {p_imp:.0}、点击 {p_clk:.0}）\n- URL Inspection 已查收录：{idx}/{tot}\n- {visitors}\n- 新页面闸门：{}\n",
        s(a, "at"), s(&a["newPageGate"], "note")
    )
}

/// Why the 24-hour visitor segments are missing: no PostHog block at all, or a failed segments query.
fn segments_failure(traffic: &Value) -> Option<String> {
    if traffic.is_null() { return Some("本轮没有 PostHog 数据".into()); }
    traffic["segments_error"].as_str().map(|e| e.chars().take(80).collect())
}

fn day_visitors(traffic: &Value) -> String {
    let days: Vec<String> = traffic["days"].as_array().into_iter().flatten().rev().take(3)
        .map(|d| format!("{} {:.0} 人", s(d, "day"), f(d, "visitors"))).collect();
    if days.is_empty() { "无".into() } else { days.join("、") }
}

fn format_anomalies(anomalies: &[Anomaly]) -> String {
    if anomalies.is_empty() { return "## 2. 异常指标捕捉\n- 本轮无超阈值异常指标。\n".into(); }
    let lines: Vec<String> = anomalies.iter().map(|a| {
        format!("- [{}][{}] {}：{}（对照 {}）\n  - 假设A：{} | 假设B（反方）：{}",
            a.severity, a.category, a.subject, a.current, a.baseline, a.hypothesis_a, a.hypothesis_b)
    }).collect();
    format!("## 2. 异常指标捕捉（{} 项）\n{}\n", anomalies.len(), lines.join("\n"))
}

fn format_pages_and_ranks(a: &Value) -> String {
    let mut out = String::from("## 3. GSC 真实页面点击与目标词排名收敛（节选）\n");
    for row in a["gsc"]["pages"]["rows"].as_array().into_iter().flatten().take(10) {
        let url = row["keys"][0].as_str().unwrap_or_default();
        out.push_str(&format!("- 页面 {url}：展示 {:.0}，点击 {:.0}，CTR {:.2}%，均名 {:.1}\n",
            f(row, "impressions"), f(row, "clicks"), f(row, "ctr") * 100.0, f(row, "position")));
    }
    if let Some(ranks) = a["ranks"].as_array() {
        let mut sorted: Vec<&Value> = ranks.iter().collect();
        sorted.sort_by(|x, y| f(y, "impressions").total_cmp(&f(x, "impressions")));
        out.push_str("- 目标词代表行（按 GSC 展示排序前 12）：\n");
        for r in sorted.into_iter().take(12) {
            out.push_str(&format!("  - 「{}」({})：有效名次 {:.1}（SERP {:?} / GSC {:.1}，展示 {:.0}，7d变化 {:?}，停滞 {}d，榜首 {}）\n",
                s(r, "keyword"), s(r, "page"), f(r, "rank"), r["serp"].as_f64(), f(r, "gscPosition"),
                f(r, "impressions"), r["delta7"].as_f64(), r["stalledDays"].as_i64().unwrap_or(0), s(r, "leader")));
        }
    }
    if let Some(b) = a.get("bing").filter(|v| !v.is_null()) {
        let uncrawled = b["crawled"].as_object().map_or(0, |m| m.values().filter(|v| v.is_null()).count());
        out.push_str(&format!("- Bing 收录页数：{:?}，已查目标页中从未抓取：{uncrawled} 页\n", b["inIndex"].as_i64()));
    }
    out
}

fn format_traffic_and_trends(a: &Value) -> String {
    let mut out = String::from("## 4. 流量来源、可疑批次与异常上升词\n");
    for src in a["traffic"]["sources"].as_array().into_iter().flatten().take(10) {
        out.push_str(&format!("- 来源 {} · {}：访客 {:.0}（只看一页 {:.0}，落地 {}）\n",
            s(src, "day"), s(src, "source"), f(src, "visitors"), f(src, "pageview_only"), s(src, "landing")));
    }
    for b in a["traffic"]["batches"].as_array().into_iter().flatten().take(6) {
        out.push_str(&format!("- 同设备批次 {}/{}/{}：{:.0} 人（只看一页 {:.0}，IP {}，落地 {}）\n",
            s(b, "os"), s(b, "browser"), s(b, "timezone"), f(b, "visitors"), f(b, "pageview_only"), s(b, "ips"), s(b, "landings")));
    }
    for r in a["trends"]["rising"].as_array().into_iter().flatten().filter(|r| !s(r, "flag").is_empty()).take(10) {
        out.push_str(&format!("- Trends 上升词「{}」[{} / {}]：当月 {:?}，倍数 {:?}，周加速 {:?}，来源 {}\n",
            s(r, "query"), s(r, "flag"), s(r, "market"), r["volume"].as_f64(), r["surge"].as_f64(), r["accel"].as_f64(), s(r, "source")));
    }
    out
}

fn format_findings_and_history(a: &Value, state_dir: Option<&str>) -> String {
    let mut out = String::from("## 5. Watchdog 待办事项与近期历史\n");
    for item in a["findings"].as_array().into_iter().flatten().take(20) {
        out.push_str(&format!("- [{}] {} ({}) → {}\n", s(item, "severity"), s(item, "title"), s(item, "key"), s(item, "action")));
    }
    if let Some(dir) = state_dir {
        if let Ok(hist) = std::fs::read_to_string(format!("{dir}/history.jsonl")) {
            let tail: Vec<&str> = hist.lines().rev().take(6).collect();
            out.push_str(&format!("- 最近 {} 轮 history.jsonl：\n  {}\n", tail.len(), tail.into_iter().rev().collect::<Vec<_>>().join("\n  ")));
        }
        if let Ok(vetoes) = std::fs::read_to_string(format!("{dir}/keyword-vetoes.jsonl")) {
            let tail: Vec<&str> = vetoes.lines().rev().take(6).collect();
            if !tail.is_empty() { out.push_str(&format!("- 近期已否决词（节选）：\n  {}\n", tail.join("\n  "))); }
        }
    }
    out
}

pub fn build_pack(run_dir: &str, state_dir: Option<&str>, draft_path: Option<&str>, lead: &str) -> Result<(String, Vec<Anomaly>), String> {
    let analysis = read_json(&format!("{run_dir}/analysis.json"))?;
    let anomalies = anomalies_of(&analysis, state_dir);
    let draft_section = match draft_path {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            format!("## 6. {lead} 主导会话的初判与拟执行方案（待你红队反驳）\n\n{}\n", text.trim())
        }
        None => format!("## 6. {lead} 初判草案\n（未传入 --draft，当前为独立先行红队模式：请直接从原始数据指出易踩误区、关键异常与高杠杆建议。）\n"),
    };
    let pack = [
        format_overview(&analysis),
        format_anomalies(&anomalies),
        format_pages_and_ranks(&analysis),
        format_traffic_and_trends(&analysis),
        format_findings_and_history(&analysis, state_dir),
        draft_section,
    ].join("\n");
    Ok((pack, anomalies))
}
