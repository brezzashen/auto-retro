//! One site's push round: read its sitemap, check the files that keep it verified, then push the URLs
//! each engine has not accepted yet (Baidu, IndexNow, Bing URL Submission). Baidu and IndexNow also
//! re-push accepted URLs whose sitemap `lastmod` date or page head moved on. Baidu and Bing push within
//! today's quota; with no answer yet today Baidu sends one URL to learn `remain`, then fills what is left.

use super::push_bing::{bing_round, indexed, refresh_feed};
use super::push_config::{EngineState, PushSite, SiteState};
use super::push_heads::{head_changed, page_heads, remember_heads};
use super::push_engines::{BaiduAnswer, baidu_push, get, indexnow_push, pending, revised};
use super::watch_probe::{sitemap_content_dates, sitemap_urls};
use std::collections::BTreeMap;

/// Largest batch Baidu accepts in one request.
const BAIDU_BATCH: usize = 2000;

/// What one site's round found, for the page and the repair decision.
#[derive(serde::Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SiteRound {
    pub site: String,
    pub urls: usize,
    pub baidu: Option<String>,
    pub indexnow: Option<String>,
    /// Bing Webmaster URL Submission outcome.
    pub bing: Option<String>,
    /// Pages in Bing's index by its latest crawl statistics.
    pub bing_indexed: Option<i64>,
    /// `(path, HTTP status)` of every file the site must keep serving.
    pub files: Vec<(String, u16)>,
    /// `(engine, URLs it has accepted)` after this round, for the progress chart.
    pub accepted: Vec<(String, usize)>,
    /// `(key, description)` of every failure; the key names the broken thing, not the time.
    pub failures: Vec<(String, String)>,
}

/// Inputs shared by every engine call of one run.
pub struct Run<'a> {
    pub client: &'a reqwest::Client,
    /// Beijing date (YYYY-MM-DD); both engines' quotas reset on it.
    pub day: &'a str,
    pub now: &'a str,
    pub dry_run: bool,
}

/// Credentials of one run: the Baidu token of this site, the shared IndexNow key and the Bing API key.
pub struct Keys<'a> {
    pub baidu: Option<&'a str>,
    pub indexnow: &'a str,
    pub bing: Option<&'a str>,
}

pub async fn round(run: &Run<'_>, site: &PushSite, keys: &Keys<'_>, state: &mut SiteState) -> SiteRound {
    let (token, key) = (keys.baidu, keys.indexnow);
    let origin = site.site.trim_end_matches('/');
    let mut out = SiteRound { site: origin.to_string(), ..Default::default() };
    for path in &site.keep_files {
        let (status, body) = get(run.client, &format!("{origin}{path}")).await;
        if let Some(problem) = keep_file_problem(status, &body) {
            out.failures.push((format!("file:{origin}{path}"), format!("{origin}{path} {problem}，搜索引擎会因此取消站点验证")));
        }
        out.files.push((path.clone(), status));
    }
    let (status, xml) = get(run.client, &format!("{origin}/sitemap.xml")).await;
    let urls = sitemap_urls(&xml);
    out.urls = urls.len();
    if status != 200 || urls.is_empty() {
        out.failures.push((format!("sitemap:{origin}"), format!("{origin}/sitemap.xml 返回 HTTP {status}，含 {} 个地址", urls.len())));
        return out;
    }
    let dates = sitemap_content_dates(&xml);
    let heads = if token.is_some() || site.indexnow { page_heads(run.client, &urls).await } else { BTreeMap::new() };
    if let Some(token) = token {
        match baidu_round(run, origin, token, &urls, &dates, &heads, &mut state.baidu).await {
            Ok(line) => out.baidu = Some(line),
            Err(error) => { out.failures.push((format!("baidu:{origin}"), format!("百度推送失败：{error}"))); out.baidu = Some(format!("失败：{error}")); }
        }
    } else if site.baidu_token_file.is_some() {
        out.failures.push((format!("baidu-token:{origin}"), "百度推送 token 文件缺失或为空".into()));
    }
    if site.indexnow {
        match indexnow_round(run, origin, key, &urls, &dates, &heads, &mut state.indexnow).await {
            Ok(line) => out.indexnow = Some(line),
            Err(error) => { out.failures.push((format!("indexnow:{origin}"), format!("IndexNow 推送失败：{error}"))); out.indexnow = Some(format!("失败：{error}")); }
        }
    }
    match (site.bing_submit, keys.bing) {
        (true, Some(bing)) => {
            match bing_round(run, origin, bing, &urls, &mut state.bing).await {
                Ok(line) => out.bing = Some(line),
                Err(error) => { out.failures.push((format!("bing:{origin}"), format!("Bing URL 提交失败：{error}"))); out.bing = Some(format!("失败：{error}")); }
            }
            match refresh_feed(run, origin, bing, urls.len()).await {
                Ok(Some(line)) => out.bing = Some(format!("{}；{line}", out.bing.take().unwrap_or_default())),
                Ok(None) => {}
                Err(error) => out.failures.push((format!("bing-feed:{origin}"), format!("Bing sitemap 重新提交失败：{error}"))),
            }
            out.bing_indexed = indexed(run.client, origin, bing).await;
        }
        (true, None) => out.failures.push((format!("bing-key:{origin}"), "Bing Webmaster API key 文件缺失或为空".into())),
        _ => {}
    }
    let live = |pushed: &std::collections::BTreeMap<String, String>| urls.iter().filter(|u| pushed.contains_key(*u)).count();
    if token.is_some() { out.accepted.push(("百度".into(), live(&state.baidu.pushed))); }
    if site.indexnow { out.accepted.push(("IndexNow".into(), live(&state.indexnow.pushed))); }
    if site.bing_submit && keys.bing.is_some() { out.accepted.push(("Bing".into(), live(&state.bing.pushed))); }
    out
}

/// Why a verification file is not really served: a status other than 200, an empty body, or the web
/// app's shell (`data-mars-app`), which example.com returns with 200 for any missing root file.
fn keep_file_problem(status: u16, body: &str) -> Option<String> {
    if status != 200 { return Some(format!("返回 HTTP {status}")); }
    if body.trim().is_empty() { return Some("返回空内容".into()); }
    if body.contains("data-mars-app") { return Some("返回的是网站首页 HTML（文件不存在，SPA 兜底）".into()); }
    None
}

/// Baidu serves Simplified Chinese searchers and gives a new site about ten URLs a day: `/zh-CN` home and
/// pages go first, then root `/`, then `/zh-TW` and English, and `/ja` `/ko` `/th` `/fr` `/it` `/es` last.
fn simplified_first(mut urls: Vec<String>, origin: &str) -> Vec<String> {
    let zh_home = format!("{origin}/zh-CN");
    let zh_below = format!("{zh_home}/");
    urls.sort_by_key(|url| {
        if *url == zh_home { 0u8 }
        else if url.starts_with(&zh_below) { 1 }
        else if *url == origin { 2 }
        else if ["/ja/", "/ko/", "/th/", "/fr/", "/it/", "/es/"].iter().any(|p| url.starts_with(&format!("{origin}{p}"))) { 4 }
        else { 3 }
    });
    urls
}

fn baidu_queue(urls: &[String], origin: &str, dates: &BTreeMap<String, String>, heads: &BTreeMap<String, String>, state: &EngineState) -> Vec<String> {
    let mut changed = revised(urls, dates, state);
    for url in head_changed(urls, heads, state) { if !changed.contains(&url) { changed.push(url); } }
    simplified_first(changed, origin).into_iter().chain(simplified_first(pending(urls, &state.pushed), origin)).collect()
}

fn remember_baidu_baselines(urls: &[String], dates: &BTreeMap<String, String>, heads: &BTreeMap<String, String>, queued: &[String], state: &mut EngineState) {
    for url in urls.iter().filter(|u| state.pushed.contains_key(*u) && !queued.contains(u)) {
        if let Some(d) = dates.get(url) { state.pushed_lastmod.entry(url.clone()).or_insert_with(|| d.clone()); }
        if let Some(h) = heads.get(url) { state.pushed_head.entry(url.clone()).or_insert_with(|| h.clone()); }
    }
}

fn today_room(state: &EngineState, day: &str) -> i64 {
    match &state.remain { Some((d, remain)) if d == day => *remain, _ => 1 }
}

async fn baidu_round(run: &Run<'_>, origin: &str, token: &str, urls: &[String], dates: &BTreeMap<String, String>, heads: &BTreeMap<String, String>, state: &mut EngineState) -> Result<String, String> {
    let mut left = baidu_queue(urls, origin, dates, heads, state);
    remember_baidu_baselines(urls, dates, heads, &left, state);
    if left.is_empty() { return Ok(format!("{} 个地址都已被百度接收，无内容更新", urls.len())); }
    if state.exhausted_day.as_deref() == Some(run.day) { return Ok(format!("今日配额已用完，待推 {} 个，明天继续", left.len())); }
    if run.dry_run { return Ok(format!("（演练）待推 {} 个", left.len())); }
    let mut sent = 0;
    for _ in 0..2 {
        let room = today_room(state, run.day);
        if room <= 0 || left.is_empty() { break; }
        let batch: Vec<String> = left.iter().take((room as usize).min(BAIDU_BATCH)).cloned().collect();
        match baidu_push(run.client, origin, token, &batch).await {
            BaiduAnswer::Accepted { success, remain, rejected } => {
                for url in batch.iter().filter(|u| !rejected.contains(u)).take(success.max(0) as usize) {
                    state.pushed.insert(url.clone(), run.now.to_string());
                    if let Some(d) = dates.get(url) { state.pushed_lastmod.insert(url.clone(), d.clone()); }
                    if let Some(h) = heads.get(url) { state.pushed_head.insert(url.clone(), h.clone()); }
                }
                sent += success;
                state.remain = Some((run.day.to_string(), remain));
                if remain <= 0 { state.exhausted_day = Some(run.day.to_string()); }
                if !rejected.is_empty() { state.last_result = Some(format!("百度拒收 {}", rejected.join(", "))); }
            }
            BaiduAnswer::OverQuota => {
                state.exhausted_day = Some(run.day.to_string());
                state.remain = Some((run.day.to_string(), 0));
                break;
            }
            BaiduAnswer::Failed(error) => return Err(error),
        }
        left = baidu_queue(urls, origin, dates, heads, state);
    }
    let remain = today_room(state, run.day).max(0);
    let line = format!("本次推送 {sent} 个，今日剩余配额 {remain}，已接收 {}/{}，待推 {}", state.pushed.len().min(urls.len()), urls.len(), left.len());
    state.last_result = Some(line.clone());
    Ok(line)
}

/// Pushes the URLs IndexNow has not accepted yet plus the accepted ones whose sitemap content date or
/// page head (title, description, canonical) moved on.
async fn indexnow_round(run: &Run<'_>, origin: &str, key: &str, urls: &[String], dates: &BTreeMap<String, String>, heads: &BTreeMap<String, String>, state: &mut EngineState) -> Result<String, String> {
    let fresh = pending(urls, &state.pushed);
    let mut changed = revised(urls, dates, state);
    for url in head_changed(urls, heads, state) { if !changed.contains(&url) { changed.push(url); } }
    let left: Vec<String> = fresh.iter().chain(&changed).cloned().collect();
    if left.is_empty() {
        remember_dates(urls, dates, state);
        remember_heads(heads, state);
        return Ok(format!("{} 个地址都已推送，无内容更新", urls.len()));
    }
    if run.dry_run { return Ok(format!("（演练）待推 {} 个：新地址 {}，内容更新 {}", left.len(), fresh.len(), changed.len())); }
    let status = indexnow_push(run.client, origin, key, &left).await?;
    for url in &left { state.pushed.insert(url.clone(), run.now.to_string()); }
    remember_dates(urls, dates, state);
    remember_heads(heads, state);
    let line = format!("本次推送 {} 个（HTTP {status}）：新地址 {}，内容更新 {}；已推 {}/{}", left.len(), fresh.len(), changed.len(), urls.len(), urls.len());
    state.last_result = Some(line.clone());
    Ok(line)
}

/// Records the content date of every accepted URL, so the next round re-pushes only a later revision.
fn remember_dates(urls: &[String], dates: &BTreeMap<String, String>, state: &mut EngineState) {
    for url in urls.iter().filter(|url| state.pushed.contains_key(*url)) {
        if let Some(date) = dates.get(url) { state.pushed_lastmod.insert(url.clone(), date.clone()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keep_file_answered_by_the_app_shell_is_missing() {
        assert_eq!(keep_file_problem(200, "<?xml version=\"1.0\"?><users><user>X</user></users>"), None);
        assert!(keep_file_problem(200, "<!doctype html>\n<html lang=\"en\" data-mars-app>").is_some());
        assert!(keep_file_problem(200, "  ").is_some());
        assert!(keep_file_problem(404, "x").is_some());
    }

    #[test]
    fn baidu_gets_the_simplified_chinese_pages_first_in_sitemap_order() {
        let urls = ["https://a.b/ja/p", "https://a.b", "https://a.b/zh-CN/x", "https://a.b/zh-CN", "https://a.b/zh-TW/x", "https://a.b/zh-CNN"].map(String::from).to_vec();
        assert_eq!(simplified_first(urls, "https://a.b"), ["https://a.b/zh-CN", "https://a.b/zh-CN/x", "https://a.b", "https://a.b/zh-TW/x", "https://a.b/zh-CNN", "https://a.b/ja/p"]);
    }

    #[test]
    fn baidu_queue_puts_head_and_date_revisions_ahead_of_new_urls() {
        let urls = ["https://a.b", "https://a.b/zh-CN", "https://a.b/zh-CN/new"].map(String::from).to_vec();
        let dates = BTreeMap::new();
        let heads: BTreeMap<String, String> = [("https://a.b", "h2"), ("https://a.b/zh-CN", "zh2"), ("https://a.b/zh-CN/new", "n1")].map(|(u, h)| (u.into(), h.into())).into_iter().collect();
        let mut state = EngineState::default();
        for (u, h) in [("https://a.b", "h1"), ("https://a.b/zh-CN", "zh1")] {
            state.pushed.insert(u.into(), "2026-11-10T17:33:18+08:00".into());
            state.pushed_head.insert(u.into(), h.into());
        }
        let queued = baidu_queue(&urls, "https://a.b", &dates, &heads, &state);
        assert_eq!(queued, ["https://a.b/zh-CN", "https://a.b", "https://a.b/zh-CN/new"]);
        remember_baidu_baselines(&urls, &dates, &heads, &queued, &mut state);
        assert_eq!(state.pushed_head.get("https://a.b").map(String::as_str), Some("h1"));
    }

    #[test]
    fn room_is_one_probe_url_until_baidu_answers_today() {
        let mut state = EngineState::default();
        assert_eq!(today_room(&state, "2026-11-03"), 1);
        state.remain = Some(("2026-11-03".into(), 9));
        assert_eq!(today_room(&state, "2026-11-03"), 9);
        assert_eq!(today_room(&state, "2026-11-04"), 1);
    }
}
