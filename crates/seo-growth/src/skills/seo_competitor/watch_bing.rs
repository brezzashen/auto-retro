//! Bing in the hourly SEO watchdog; DuckDuckGo and Yahoo serve its index, so this is their view too.
//! Every run reads the Bing Webmaster API (free: indexed count, sitemap feed, query positions, and the
//! crawl state of a few target pages in rotation) and checks the first Bing page of `serpPerRun` target
//! words through SerpApi (`monid`, $0.015 a search). DataForSEO `/serp/bing-organic` is not used: on
//! 2026-11-09 it answered "personal crm" with dictionary pages and "exampleapp" with the LA Rams.
//! Rotation and the last rank of every word live in `<state>/bing.json`.

use super::files::{stringify, write_text};
use super::push_bing::{latest_in_index, wcf_ms};
use super::watch_analyze::Finding;
use super::watch_config::TargetKeyword;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const API: &str = "https://ssl.bing.com/webmaster/api.svc/json";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct BingConfig {
    /// Bing Webmaster API key file, relative to `$HOME`.
    pub api_key_file: String,
    /// Target words whose Bing first page is fetched per run.
    pub serp_per_run: usize,
    /// Target pages passed to `GetUrlInfo` per run.
    pub url_info_per_run: usize,
}

impl Default for BingConfig {
    fn default() -> Self {
        Self { api_key_file: ".config/mars-seo/bing-webmaster.apikey".into(), serp_per_run: 3, url_info_per_run: 6 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WordRank {
    pub page: String,
    /// First-page position of a example.com result; `None` means not on the first page.
    pub position: Option<u32>,
    pub url: Option<String>,
    pub at: String,
}

/// `<state>/bing.json`.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BingState {
    pub serp_cursor: usize,
    pub url_cursor: usize,
    pub ranks: BTreeMap<String, WordRank>,
    /// Target page → day Bing last crawled it (`None`: never).
    pub crawled: BTreeMap<String, Option<String>>,
}

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueryRow {
    pub query: String,
    pub impressions: i64,
    pub clicks: i64,
    /// Impression-weighted mean of Bing's weekly average position; 0 when Bing gave none.
    pub position: f64,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BingView {
    pub in_index: Option<i64>,
    /// `(URLs Bing counted in sitemap.xml, day it last read it)`.
    pub feed: Option<(i64, String)>,
    pub queries: Vec<QueryRow>,
    pub ranks: BTreeMap<String, WordRank>,
    pub crawled: BTreeMap<String, Option<String>>,
    pub checked_words: Vec<String>,
}

pub fn day_of(ms: i64) -> Option<String> {
    (ms > 0).then(|| chrono::DateTime::from_timestamp_millis(ms).map(|t| t.format("%Y-%m-%d").to_string())).flatten()
}

/// Bing's weekly query rows summed per query, most impressions first.
pub fn query_rows(stats: &Value, limit: usize) -> Vec<QueryRow> {
    let mut by: BTreeMap<String, (i64, i64, f64)> = BTreeMap::new();
    for row in stats["d"].as_array().into_iter().flatten() {
        let (Some(query), Some(seen)) = (row["Query"].as_str(), row["Impressions"].as_i64()) else { continue };
        let entry = by.entry(query.to_string()).or_default();
        entry.0 += seen;
        entry.1 += row["Clicks"].as_i64().unwrap_or(0);
        if let Some(p) = row["AvgImpressionPosition"].as_f64().filter(|p| *p > 0.0) { entry.2 += p * seen as f64; }
    }
    let mut rows: Vec<QueryRow> = by.into_iter().map(|(query, (impressions, clicks, weighted))| {
        let position = if impressions > 0 { (weighted / impressions as f64 * 10.0).round() / 10.0 } else { 0.0 };
        QueryRow { query, impressions, clicks, position }
    }).collect();
    rows.sort_by(|a, b| b.impressions.cmp(&a.impressions).then(a.query.cmp(&b.query)));
    rows.truncate(limit);
    rows
}

/// `(position, url)` of the first organic result on `host` or its `www.`.
pub fn serp_position(answer: &Value, host: &str) -> Option<(u32, String)> {
    answer["organic_results"].as_array()?.iter().find_map(|r| {
        let link = r["link"].as_str()?;
        let found = reqwest::Url::parse(link).ok()?.host_str()?.trim_start_matches("www.") == host;
        found.then(|| (r["position"].as_u64().unwrap_or(0) as u32, link.to_string()))
    })
}

/// SerpApi market of a target page by its locale prefix.
pub fn market_of(page: &str) -> &'static str {
    if page.starts_with("/ja/") { "jp" } else if page.starts_with("/ko/") { "kr" } else if page.starts_with("/th/") { "th" } else if page.starts_with("/fr/") { "fr" } else if page.starts_with("/it/") { "it" } else if page.starts_with("/es/") { "es" } else if page.starts_with("/zh-TW") { "tw" } else { "us" }
}

/// `n` items of `items` from `cursor` on, wrapping; returns them and the next cursor.
pub fn rotate<T: Clone>(items: &[T], cursor: usize, n: usize) -> (Vec<T>, usize) {
    if items.is_empty() { return (Vec::new(), 0); }
    let take = n.min(items.len());
    ((0..take).map(|i| items[(cursor + i) % items.len()].clone()).collect(), (cursor + take) % items.len())
}

fn api(method: &str, site: &str, key: &str, extra: &str) -> Result<Value, String> {
    let url = format!("{API}/{method}?siteUrl={}&apikey={key}{extra}", urlencoding(&format!("{}/", site.trim_end_matches('/'))));
    let resp = crate::http::send(reqwest::Method::GET, &url, &[], None, Some(std::time::Duration::from_secs(30)))
        .map_err(|_| format!("Bing {method} 请求失败"))?;
    if resp.status != 200 { return Err(format!("Bing {method} HTTP {}", resp.status)); }
    serde_json::from_slice(&resp.body).map_err(|e| format!("Bing {method}: {e}"))
}

fn urlencoding(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Bing SERP through AIsa (`serpapi.bing-search`), else `monid run`; the answer is cached in `out` either way.
fn serp(word: &str, cc: &str, out: &str) -> Result<Value, String> {
    match super::aisa::run_tool("serpapi.bing-search", json!({"engine": "bing", "q": word, "cc": cc})) {
        Some(Ok(answer)) => {
            super::files::write_text(out, &serde_json::to_string_pretty(&answer).map_err(|e| e.to_string())?)?;
            return Ok(answer);
        }
        Some(Err(error)) => eprintln!("aisa Bing「{word}」failed, retrying on monid: {error}"),
        None => {}
    }
    let query = json!({"q": word, "cc": cc}).to_string();
    let status = std::process::Command::new("monid").args(["run", "-p", "serpapi", "-e", "/bing/search", "--query", &query, "-w", "120", "-o", out])
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).output().map_err(|e| format!("monid: {e}"))?;
    if !status.status.success() { return Err(format!("monid Bing「{word}」失败：{}", String::from_utf8_lossy(&status.stderr).lines().last().unwrap_or_default())); }
    super::files::read_json(out)
}

/// One run's Bing view; API failures land in `errors`, never stop the watchdog.
pub fn collect(config: &BingConfig, site: &str, targets: &[TargetKeyword], state_dir: &str, now: &str, dry_run: bool, errors: &mut Vec<String>) -> Option<BingView> {
    let home = std::env::var("HOME").ok()?;
    let key = std::fs::read_to_string(format!("{home}/{}", config.api_key_file)).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    let Some(key) = key else { errors.push("bing: API key 文件缺失".into()); return None };
    let path = format!("{state_dir}/bing.json");
    let mut state: BingState = super::files::read_json(&path).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let mut view = BingView::default();
    let mut call = |method: &str, extra: &str| api(method, site, &key, extra).map_err(|e| errors.push(format!("bing: {e}"))).ok();
    view.in_index = call("GetCrawlStats", "").as_ref().and_then(latest_in_index);
    let feed_url = format!("{}/sitemap.xml", site.trim_end_matches('/'));
    view.feed = call("GetFeeds", "").and_then(|f| f["d"].as_array()?.iter().find(|x| x["Url"].as_str() == Some(feed_url.as_str())).cloned())
        .map(|f| (f["UrlCount"].as_i64().unwrap_or(0), wcf_ms(&f["LastCrawled"]).and_then(day_of).unwrap_or_default()));
    view.queries = call("GetQueryStats", "").map(|s| query_rows(&s, 15)).unwrap_or_default();
    let mut pages: Vec<String> = targets.iter().map(|t| format!("{}{}", site.trim_end_matches('/'), t.page)).collect();
    pages.sort();
    pages.dedup();
    let (due, next) = rotate(&pages, state.url_cursor, config.url_info_per_run);
    for url in &due {
        if let Some(info) = call("GetUrlInfo", &format!("&url={}", urlencoding(url))) {
            state.crawled.insert(url.clone(), wcf_ms(&info["d"]["LastCrawledDate"]).and_then(day_of));
        }
        std::thread::sleep(std::time::Duration::from_millis(2500));
    }
    state.url_cursor = next;
    let host = reqwest::Url::parse(site).ok()?.host_str()?.to_string();
    let (words, next) = rotate(targets, state.serp_cursor, config.serp_per_run);
    if !dry_run {
        let dir = format!("{state_dir}/bing/{}", &now[..10]);
        let _ = std::fs::create_dir_all(&dir);
        for t in &words {
            let out = format!("{dir}/serp_{}.json", super::monid::slug(&t.keyword));
            match serp(&t.keyword, market_of(&t.page), &out) {
                Ok(answer) => {
                    let found = serp_position(&answer, &host);
                    state.ranks.insert(t.keyword.clone(), WordRank { page: t.page.clone(), position: found.as_ref().map(|f| f.0), url: found.map(|f| f.1), at: now.to_string() });
                }
                Err(e) => errors.push(format!("bing: {e}")),
            }
        }
        state.serp_cursor = next;
        if let Err(e) = stringify(&state).and_then(|text| write_text(&path, &text)) { errors.push(format!("bing: {e}")); }
    }
    view.checked_words = words.iter().map(|t| t.keyword.clone()).collect();
    view.ranks = state.ranks;
    view.crawled = state.crawled;
    Some(view)
}

/// P2 while target pages Bing has never crawled remain: they cannot show on Bing, DuckDuckGo or Yahoo.
pub fn findings(view: &BingView) -> Vec<Finding> {
    let never: Vec<&String> = view.crawled.iter().filter(|(_, day)| day.is_none()).map(|(url, _)| url).collect();
    if never.is_empty() { return Vec::new(); }
    vec![Finding { key: format!("bing:uncrawled:{}", never.len()), severity: "P2".into(),
        title: format!("Bing 从未抓取 {} 个目标页（DuckDuckGo、Yahoo 同样搜不到）", never.len()),
        action: "seo-push 已提交 URL 与 sitemap；查 Bing Webmaster 的 URL 检查与 Site Scan，补外链与内链".into() }]
}

/// The page section `Bing 排名与收录`.
pub fn section(view: Option<&BingView>) -> String {
    let Some(v) = view else { return String::new() };
    let feed = v.feed.as_ref().map_or("—".to_string(), |(n, day)| format!("{n} 个地址（{day} 读取）"));
    let never = v.crawled.values().filter(|d| d.is_none()).count();
    let mut md = format!("## Bing 排名与收录\n\nDuckDuckGo 与 Yahoo 用 Bing 的索引。Bing 收录 {} 页；Bing 读到的 sitemap：{feed}；目标页已查 {} 个，从未抓取 {never} 个。本次查排名：{}。\n\n",
        v.in_index.map_or("—".into(), |n| n.to_string()), v.crawled.len(), if v.checked_words.is_empty() { "无（演练）".into() } else { v.checked_words.join("、") });
    if v.ranks.is_empty() { md.push_str("还没有查过目标词的 Bing 名次。\n"); } else { md.push_str("| 目标词 | 页面 | Bing 名次 | 检查时间 |\n|---|---|---|---|\n"); }
    let mut ranks: Vec<(&String, &WordRank)> = v.ranks.iter().collect();
    ranks.sort_by_key(|(k, r)| (r.position.unwrap_or(u32::MAX), (*k).clone()));
    for (word, r) in ranks {
        md.push_str(&format!("| {word} | {} | {} | {} |\n", r.page, r.position.map_or("首页外".into(), |p| p.to_string()), r.at.get(..16).unwrap_or(&r.at).replace('T', " ")));
    }
    md.push_str("\n| Bing 查询 | 展示 | 点击 | 平均名次 |\n|---|---|---|---|\n");
    for q in &v.queries { md.push_str(&format!("| {} | {} | {} | {} |\n", q.query.replace('|', "\\|"), q.impressions, q.clicks, if q.position > 0.0 { q.position.to_string() } else { "—".into() })); }
    md.push('\n');
    md
}

#[cfg(test)]
#[path = "watch_bing_tests.rs"]
mod tests;
