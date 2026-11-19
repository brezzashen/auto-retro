//! Bing Webmaster API (`ssl.bing.com/webmaster/api.svc/json`, key from Settings → API access): URL
//! Submission within the site's daily quota (100 a day, shared with the console's URL Submission page)
//! and the crawl statistics that give Bing's own count of indexed pages. IndexNow tells Bing a URL
//! changed; URL Submission asks it to crawl the URL, which is what a new site without links needs.

use super::push_config::EngineState;
use super::push_engines::pending;
use super::push_site::Run;
use serde_json::Value;

const API: &str = "https://ssl.bing.com/webmaster/api.svc/json";
/// Largest batch `SubmitUrlBatch` accepts.
const BING_BATCH: usize = 500;

/// Bing names a site by its origin with a trailing slash, as registered in Webmaster Tools.
fn site_url(origin: &str) -> String {
    format!("{}/", origin.trim_end_matches('/'))
}

/// Error text without the request URL, which carries the API key, followed by the causes reqwest
/// hides behind "error sending request" (DNS, connect, TLS, reset).
fn quiet(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut text = error.to_string();
    let mut cause = std::error::Error::source(&error);
    while let Some(inner) = cause {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        cause = inner.source();
    }
    text
}

/// Sends the request, and once more after a transport failure: on 2026-11-10 08:53 one `GetFeeds`
/// call failed with "error sending request" while every other call of the run, and the same call
/// a minute later, answered 200. `resend` is false for a request that must not be repeated once it
/// may have reached Bing (`SubmitUrlBatch` spends quota), which is then retried only when the
/// connection itself never opened.
async fn send(request: reqwest::RequestBuilder, resend: bool) -> Result<reqwest::Response, String> {
    let again = request.try_clone();
    let first = match request.send().await { Ok(resp) => return Ok(resp), Err(error) => error };
    let Some(again) = again.filter(|_| resend || first.is_connect()) else { return Err(quiet(first)) };
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    again.send().await.map_err(|second| format!("{}（重试一次仍失败）", quiet(second)))
}

async fn get_json(client: &reqwest::Client, method: &str, origin: &str, key: &str) -> Result<Value, String> {
    let resp = send(client.get(format!("{API}/{method}")).query(&[("siteUrl", site_url(origin).as_str()), ("apikey", key)]), true).await?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(quiet)?;
    if status != 200 { return Err(format!("{method} HTTP {status}: {}", body.chars().take(120).collect::<String>())); }
    serde_json::from_str(&body).map_err(|e| format!("{method}: {e}"))
}

/// URLs Bing still accepts today for this site.
pub fn daily_quota(answer: &Value) -> i64 {
    answer["d"]["DailyQuota"].as_i64().unwrap_or(0)
}

/// Pages Bing reports in its index on the latest day of `GetCrawlStats`.
pub fn latest_in_index(answer: &Value) -> Option<i64> {
    answer["d"].as_array()?.iter().max_by_key(|row| row["Date"].as_str().unwrap_or("").to_string()).and_then(|row| row["InIndex"].as_i64())
}

async fn submit(client: &reqwest::Client, origin: &str, key: &str, urls: &[String]) -> Result<(), String> {
    let body = serde_json::json!({"siteUrl": site_url(origin), "urlList": urls});
    let resp = send(client.post(format!("{API}/SubmitUrlBatch")).query(&[("apikey", key)]).json(&body), false).await?;
    let status = resp.status().as_u16();
    if status == 200 { Ok(()) } else { Err(format!("SubmitUrlBatch HTTP {status}: {}", resp.text().await.unwrap_or_default().chars().take(160).collect::<String>())) }
}

/// Bing's indexed-page count for the site, or `None` when the API gives none.
pub async fn indexed(client: &reqwest::Client, origin: &str, key: &str) -> Option<i64> {
    get_json(client, "GetCrawlStats", origin, key).await.ok().as_ref().and_then(latest_in_index)
}

/// Submits the sitemap URLs Bing has not been sent yet, as many as today's quota allows.
pub async fn bing_round(run: &Run<'_>, origin: &str, key: &str, urls: &[String], state: &mut EngineState) -> Result<String, String> {
    let left = pending(urls, &state.pushed);
    if left.is_empty() { return Ok(format!("{} 个地址都已提交", urls.len())); }
    if run.dry_run { return Ok(format!("（演练）待提交 {} 个", left.len())); }
    let quota = daily_quota(&get_json(run.client, "GetUrlSubmissionQuota", origin, key).await?);
    state.remain = Some((run.day.to_string(), quota));
    if quota <= 0 { return Ok(format!("今日配额已用完，待提交 {} 个，明天继续", left.len())); }
    let batch: Vec<String> = left.iter().take((quota as usize).min(BING_BATCH)).cloned().collect();
    submit(run.client, origin, key, &batch).await?;
    for url in &batch { state.pushed.insert(url.clone(), run.now.to_string()); }
    let line = format!("本次提交 {} 个，已提交 {}/{}，待提交 {}", batch.len(), urls.len() - left.len() + batch.len(), urls.len(), left.len() - batch.len());
    state.last_result = Some(line.clone());
    Ok(line)
}

/// Milliseconds since the epoch of a WCF date (`/Date(1793861954000)/`, `/Date(-11644473600000)/`).
pub fn wcf_ms(value: &Value) -> Option<i64> {
    let text = value.as_str()?;
    text.strip_prefix("/Date(")?.split([')', '+']).next()?.parse().ok()
}

/// Why Bing's copy of `<origin>/sitemap.xml` needs a resubmit, or `None`: the feed is missing, or it
/// counts fewer URLs than the live sitemap and was last submitted over a day ago (Bing re-reads a
/// submitted feed within days, so a daily resubmit only nudges it).
pub fn stale_feed(feeds: &Value, origin: &str, live: usize, now_ms: i64) -> Option<String> {
    let url = format!("{}/sitemap.xml", origin.trim_end_matches('/'));
    let Some(feed) = feeds["d"].as_array().into_iter().flatten().find(|f| f["Url"].as_str() == Some(url.as_str())) else {
        return Some(format!("Bing 没有 {url}"));
    };
    let counted = feed["UrlCount"].as_i64().unwrap_or(0);
    let submitted = wcf_ms(&feed["Submitted"]).unwrap_or(i64::MIN);
    (counted < live as i64 && now_ms.saturating_sub(submitted) > 24 * 3_600_000)
        .then(|| format!("Bing 记录的 sitemap 只有 {counted} 个地址，线上 {live} 个"))
}

/// Resubmits the sitemap when Bing's copy is stale; returns the line for the page, `None` when current.
pub async fn refresh_feed(run: &Run<'_>, origin: &str, key: &str, live: usize) -> Result<Option<String>, String> {
    let feeds = get_json(run.client, "GetFeeds", origin, key).await?;
    let Some(reason) = stale_feed(&feeds, origin, live, chrono::Utc::now().timestamp_millis()) else { return Ok(None) };
    if run.dry_run { return Ok(Some(format!("（演练）{reason}，会重新提交 sitemap"))); }
    let body = serde_json::json!({"siteUrl": site_url(origin), "feedUrl": format!("{}/sitemap.xml", origin.trim_end_matches('/'))});
    let resp = send(run.client.post(format!("{API}/SubmitFeed")).query(&[("apikey", key)]).json(&body), true).await?;
    let status = resp.status().as_u16();
    if status != 200 { return Err(format!("SubmitFeed HTTP {status}")); }
    Ok(Some(format!("{reason}，已重新提交 sitemap")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_feed_submitted_over_a_day_ago_is_resubmitted() {
        let feeds: Value = serde_json::from_str(r#"{"d":[{"Url":"https://example.com/sitemap.xml","UrlCount":48,"Submitted":"/Date(1793861953701)/"},{"Url":"https://www.example.com/sitemap.xml","UrlCount":5,"Submitted":"/Date(-11644473600000)/"}]}"#).unwrap();
        let day = 24 * 3_600_000;
        assert!(stale_feed(&feeds, "https://example.com", 461, 1793861953701 + day + 1).unwrap().contains("48"));
        assert_eq!(stale_feed(&feeds, "https://example.com", 461, 1793861953701 + day - 1), None, "submitted within a day");
        assert_eq!(stale_feed(&feeds, "https://example.com", 48, 1793861953701 + 2 * day), None, "Bing already counts every URL");
        assert!(stale_feed(&serde_json::json!({"d": []}), "https://example.cn", 153, 0).unwrap().contains("没有"));
        assert_eq!(wcf_ms(&serde_json::json!("/Date(-11644473600000)/")), Some(-11644473600000));
    }

    #[tokio::test]
    async fn a_transport_failure_is_retried_and_names_its_cause_without_the_key() {
        let client = reqwest::Client::new();
        let error = send(client.get("http://127.0.0.1:9/x").query(&[("apikey", "SECRET-KEY")]), true).await.unwrap_err();
        assert!(error.contains("重试一次"), "{error}");
        assert!(error.len() > "error sending request（重试一次仍失败）".len(), "cause chain kept: {error}");
        assert!(!error.contains("SECRET-KEY"), "{error}");
    }

    #[test]
    fn quota_and_index_count_come_from_the_api_answers() {
        let quota: Value = serde_json::from_str(r#"{"d":{"__type":"UrlSubmissionQuota","DailyQuota":100,"MonthlyQuota":2500}}"#).unwrap();
        assert_eq!(daily_quota(&quota), 100);
        assert_eq!(daily_quota(&serde_json::json!({"d": null})), 0);
        let stats: Value = serde_json::from_str(r#"{"d":[{"Date":"/Date(1785801600000)/","InIndex":5},{"Date":"/Date(1785974400000)/","InIndex":8}]}"#).unwrap();
        assert_eq!(latest_in_index(&stats), Some(8));
        assert_eq!(latest_in_index(&serde_json::json!({"d": []})), None);
    }

    #[test]
    fn site_url_carries_one_trailing_slash() {
        assert_eq!(site_url("https://example.com"), "https://example.com/");
        assert_eq!(site_url("https://example.cn/"), "https://example.cn/");
    }
}
