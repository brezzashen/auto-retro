//! Weekly Baidu ranks of our mainland China words (founder 2026-11-15, report
//! `ai-coach-keyword-watch-20261115` item 2 B): Google has no volume for `AI恋爱教练`, so the
//! Baidu first 50 results of each `baidu.words` entry are fetched through SerpApi (`monid`,
//! `/baidu/search` with `rn` 50, $0.015 a search) every `everyDays`. No Baidu index or volume
//! source exists; the rank and the sites holding the top of the page are the signal.
//! The last run and every word's rank live in `<state>/baidu.json`.

use super::files::{read_json, stringify, write_text};
use super::watch_config::TargetKeyword;
use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct BaiduConfig {
    pub every_days: i64,
    /// Results per search (`rn`); a word outside them is `前 N 名外`.
    pub depth: u32,
    /// Hosts that count as ours, `www.` stripped.
    pub hosts: Vec<String>,
    pub words: Vec<TargetKeyword>,
}

impl Default for BaiduConfig {
    fn default() -> Self {
        Self { every_days: 7, depth: 50, hosts: vec!["example.cn".into(), "example.com".into()], words: Vec::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BaiduRank {
    pub page: String,
    /// Position of our first result; `None` means outside the fetched depth.
    pub position: Option<u32>,
    pub url: Option<String>,
    /// Position at the run before this one; read with `previous_at`.
    pub previous: Option<u32>,
    /// When the word was last measured before this run; `None`: this is its first run.
    pub previous_at: Option<String>,
    /// Hosts of the first three organic results.
    pub leaders: Vec<String>,
    pub at: String,
}

/// `<state>/baidu.json`.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BaiduState {
    pub last_run: Option<String>,
    pub ranks: BTreeMap<String, BaiduRank>,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BaiduView {
    pub depth: u32,
    pub checked_now: bool,
    pub next_run: Option<String>,
    pub ranks: BTreeMap<String, BaiduRank>,
}

fn host_of(link: &str) -> Option<String> {
    Some(reqwest::Url::parse(link).ok()?.host_str()?.trim_start_matches("www.").to_string())
}

/// `(position, url)` of the first organic result on one of `hosts`, and the hosts of the first three results.
pub fn read_serp(answer: &Value, hosts: &[String]) -> (Option<(u32, String)>, Vec<String>) {
    let results = answer["organic_results"].as_array().cloned().unwrap_or_default();
    let ours = results.iter().find_map(|r| {
        let link = r["link"].as_str()?;
        hosts.contains(&host_of(link)?).then(|| (r["position"].as_u64().unwrap_or(0) as u32, link.to_string()))
    });
    let leaders = results.iter().filter_map(|r| host_of(r["link"].as_str()?)).take(3).collect();
    (ours, leaders)
}

/// Whether a run is due: never run, or `every_days` since the last one.
pub fn due(last_run: Option<&str>, now: DateTime<FixedOffset>, every_days: i64) -> bool {
    match last_run.and_then(|t| DateTime::parse_from_rfc3339(t).ok()) {
        Some(t) => now - t >= Duration::days(every_days),
        None => true,
    }
}

fn serp(word: &str, depth: u32, out: &str) -> Result<Value, String> {
    let query = json!({"q": word, "rn": depth}).to_string();
    let status = std::process::Command::new("monid").args(["run", "-p", "serpapi", "-e", "/baidu/search", "--query", &query, "-w", "150", "-o", out])
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).output().map_err(|e| format!("monid: {e}"))?;
    if !status.status.success() { return Err(format!("monid 百度「{word}」失败：{}", String::from_utf8_lossy(&status.stderr).lines().last().unwrap_or_default())); }
    read_json(out)
}

/// The rank a fresh answer gives `word`, carrying the earlier measurement as `previous`.
pub fn next_rank(before: Option<&BaiduRank>, page: &str, answer: &Value, hosts: &[String], at: &str) -> BaiduRank {
    let (ours, leaders) = read_serp(answer, hosts);
    BaiduRank { page: page.to_string(), position: ours.as_ref().map(|o| o.0), url: ours.map(|o| o.1),
        previous: before.and_then(|r| r.position), previous_at: before.map(|r| r.at.clone()), leaders, at: at.to_string() }
}

/// One run's Baidu view; `None` when no words are configured. Failures land in `errors`.
pub fn collect(config: &BaiduConfig, state_dir: &str, now: DateTime<FixedOffset>, dry_run: bool, errors: &mut Vec<String>) -> Option<BaiduView> {
    if config.words.is_empty() { return None; }
    let path = format!("{state_dir}/baidu.json");
    let mut state: BaiduState = read_json(&path).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let run = !dry_run && due(state.last_run.as_deref(), now, config.every_days);
    if run {
        let stamp = now.to_rfc3339();
        let dir = format!("{state_dir}/baidu/{}", &stamp[..10]);
        let _ = std::fs::create_dir_all(&dir);
        for t in &config.words {
            let out = format!("{dir}/serp_{}.json", super::watch_trends_market::file_key(&t.keyword));
            match serp(&t.keyword, config.depth, &out) {
                Ok(answer) => {
                    let rank = next_rank(state.ranks.get(&t.keyword), &t.page, &answer, &config.hosts, &stamp);
                    state.ranks.insert(t.keyword.clone(), rank);
                }
                Err(e) => errors.push(format!("baidu: {e}")),
            }
        }
        state.last_run = Some(stamp);
        if let Err(e) = stringify(&state).and_then(|text| write_text(&path, &text)) { errors.push(format!("baidu: {e}")); }
    }
    let next_run = state.last_run.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| (t + Duration::days(config.every_days)).format("%Y-%m-%d %H:%M").to_string());
    Some(BaiduView { depth: config.depth, checked_now: run, next_run, ranks: state.ranks })
}

fn rank_text(position: Option<u32>, depth: u32) -> String {
    position.map_or(format!("前 {depth} 名外"), |p| p.to_string())
}

/// The page section `百度排名（中国大陆，每周）`.
pub fn section(view: Option<&BaiduView>) -> String {
    let Some(v) = view else { return String::new() };
    let when = if v.checked_now { "本次已查".to_string() } else { format!("下次检查 {}", v.next_run.as_deref().unwrap_or("—")) };
    let mut md = format!("## 百度排名（中国大陆，每周）\n\n每周查一次百度前 {} 名里 example.cn / example.com 的位置，{when}。\n\n", v.depth);
    if v.ranks.is_empty() { md.push_str("还没有查过百度名次。\n\n"); return md; }
    md.push_str("| 关键词 | 页面 | 百度名次 | 上次 | 首页前 3 | 检查时间 |\n|---|---|---|---|---|---|\n");
    let mut ranks: Vec<(&String, &BaiduRank)> = v.ranks.iter().collect();
    ranks.sort_by_key(|(k, r)| (r.position.unwrap_or(u32::MAX), (*k).clone()));
    for (word, r) in ranks {
        let previous = r.previous_at.as_ref().map_or("—".to_string(), |_| rank_text(r.previous, v.depth));
        md.push_str(&format!("| {word} | {} | {} | {previous} | {} | {} |\n", r.page, rank_text(r.position, v.depth),
            r.leaders.join("、"), r.at.get(..16).unwrap_or(&r.at).replace('T', " ")));
    }
    md.push('\n');
    md
}

#[cfg(test)]
#[path = "watch_baidu_tests.rs"]
mod tests;
