//! The two push APIs: Baidu API submission (`data.zz.baidu.com/urls`, page `普通收录`; a new site gets a
//! small daily quota) and IndexNow (`api.indexnow.org`), plus the answer parsing the watchdog decides on.

use super::push_config::EngineState;
use serde_json::Value;
use std::collections::BTreeMap;

/// What Baidu said about one push request.
#[derive(Debug, PartialEq)]
pub enum BaiduAnswer {
    /// `success` URLs accepted, `remain` left today; `rejected` were not the site's or not valid.
    Accepted { success: i64, remain: i64, rejected: Vec<String> },
    OverQuota,
    Failed(String),
}

pub fn baidu_answer(status: u16, body: &str) -> BaiduAnswer {
    let Ok(json) = serde_json::from_str::<Value>(body) else { return BaiduAnswer::Failed(format!("HTTP {status}: {}", body.chars().take(120).collect::<String>())) };
    if let Some(message) = json["message"].as_str() {
        return if message.contains("over quota") { BaiduAnswer::OverQuota } else { BaiduAnswer::Failed(format!("{}: {message}", json["error"])) };
    }
    let rejected = ["not_same_site", "not_valid"].iter()
        .flat_map(|k| json[*k].as_array().cloned().unwrap_or_default())
        .filter_map(|v| v.as_str().map(str::to_string)).collect();
    BaiduAnswer::Accepted { success: json["success"].as_i64().unwrap_or(0), remain: json["remain"].as_i64().unwrap_or(0), rejected }
}

/// Sitemap URLs the engine has not accepted yet, in sitemap order.
pub fn pending(urls: &[String], pushed: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    urls.iter().filter(|u| !pushed.contains_key(*u)).cloned().collect()
}

/// Accepted URLs whose sitemap content date moved past the one the engine last saw, in sitemap order.
/// A URL accepted before dates were recorded counts as revised when its date is on or after the day it
/// was accepted (`pushed` holds RFC 3339 times; their first ten characters are the date).
pub fn revised(urls: &[String], dates: &BTreeMap<String, String>, state: &EngineState) -> Vec<String> {
    urls.iter().filter(|url| {
        let (Some(date), Some(at)) = (dates.get(*url), state.pushed.get(*url)) else { return false };
        match state.pushed_lastmod.get(*url) {
            Some(seen) => date > seen,
            None => date.as_str() >= at.get(..10).unwrap_or(at),
        }
    }).cloned().collect()
}

pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; MarsSeoPush/1.0; +https://example.com)")
        .timeout(std::time::Duration::from_secs(30))
        .build().map_err(|e| e.to_string())
}

/// HTTP status and body of a GET; status 0 when the request itself failed.
pub async fn get(client: &reqwest::Client, url: &str) -> (u16, String) {
    match client.get(url).send().await {
        Ok(resp) => { let status = resp.status().as_u16(); (status, resp.text().await.unwrap_or_default()) }
        Err(_) => (0, String::new()),
    }
}

pub async fn baidu_push(client: &reqwest::Client, site: &str, token: &str, urls: &[String]) -> BaiduAnswer {
    let endpoint = format!("http://data.zz.baidu.com/urls?site={site}&token={token}");
    match client.post(&endpoint).header("Content-Type", "text/plain").body(urls.join("\n")).send().await {
        Ok(resp) => { let status = resp.status().as_u16(); baidu_answer(status, &resp.text().await.unwrap_or_default()) }
        Err(error) => BaiduAnswer::Failed(error.without_url().to_string()),
    }
}

/// Posts `urls` of `site` to IndexNow; 200 and 202 both mean accepted.
pub async fn indexnow_push(client: &reqwest::Client, site: &str, key: &str, urls: &[String]) -> Result<u16, String> {
    let host = site.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
    let body = serde_json::json!({"host": host, "key": key, "keyLocation": format!("{site}/indexnow-{key}.txt"), "urlList": urls});
    let resp = client.post("https://api.indexnow.org/indexnow").json(&body).send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status == 200 || status == 202 { Ok(status) } else { Err(format!("HTTP {status}: {}", resp.text().await.unwrap_or_default().chars().take(120).collect::<String>())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baidu_answers_map_to_accepted_quota_and_failure() {
        assert_eq!(baidu_answer(200, r#"{"remain":9,"success":1}"#), BaiduAnswer::Accepted { success: 1, remain: 9, rejected: vec![] });
        assert_eq!(baidu_answer(400, r#"{"error":400,"message":"over quota"}"#), BaiduAnswer::OverQuota);
        assert_eq!(baidu_answer(401, r#"{"error":401,"message":"token is not valid"}"#), BaiduAnswer::Failed("401: token is not valid".into()));
        let mixed = baidu_answer(200, r#"{"remain":3,"success":1,"not_same_site":["https://x.cn/a"],"not_valid":["bad"]}"#);
        assert_eq!(mixed, BaiduAnswer::Accepted { success: 1, remain: 3, rejected: vec!["https://x.cn/a".into(), "bad".into()] });
    }

    #[test]
    fn pending_keeps_sitemap_order_and_skips_accepted_urls() {
        let urls = vec!["a".to_string(), "b".into(), "c".into()];
        let pushed = [("b".to_string(), "t".to_string())].into_iter().collect();
        assert_eq!(pending(&urls, &pushed), vec!["a".to_string(), "c".into()]);
    }

    #[test]
    fn revised_urls_are_accepted_ones_whose_content_date_moved_on() {
        let urls: Vec<String> = ["new", "same", "moved", "legacy-old", "legacy-today", "undated"].map(String::from).into();
        let dates = [("new", "2026-11-04"), ("same", "2026-11-03"), ("moved", "2026-11-04"), ("legacy-old", "2026-11-02"), ("legacy-today", "2026-11-04")]
            .map(|(u, d)| (u.to_string(), d.to_string())).into_iter().collect();
        let mut state = EngineState::default();
        for url in ["same", "moved", "legacy-old", "legacy-today", "undated"] { state.pushed.insert(url.into(), "2026-11-04T00:23:00+08:00".into()); }
        state.pushed_lastmod.insert("same".into(), "2026-11-03".into());
        state.pushed_lastmod.insert("moved".into(), "2026-11-03".into());
        assert_eq!(revised(&urls, &dates, &state), ["moved", "legacy-today"]);
    }
}
