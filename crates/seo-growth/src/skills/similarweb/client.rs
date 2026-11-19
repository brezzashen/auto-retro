//! Direct HTTP client for `https://pro.similarweb.com` and `https://pro.similarweb.com` using the cached `.similarweb.com`
//! cookie jar in `~/.local/state/similarweb/cookies.json` (auto-refreshed via `sync-cookies.js` on
//! gateway 9223 when `pro.similarweb.com` refuses the session).

use chrono::{Datelike, NaiveDate, Utc};
use reqwest::Client;
use reqwest::header::{ACCEPT, COOKIE, HeaderMap, HeaderValue, REFERER, USER_AGENT};
use serde_json::{Value, json};
use std::process::Command;
use std::time::Duration;

const DASH_ORIGIN: &str = "https://pro.similarweb.com";
const SIM_ORIGIN: &str = "https://pro.similarweb.com";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";
const LEASE_PAGE: &str = ".claude/skills/agent-browser/scripts/lease-page.js";
const SYNC_SCRIPT: &str = ".claude/skills/similarweb/scripts/sync-cookies.js";

pub fn cookie_file() -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    Ok(format!("{home}/.local/state/similarweb/cookies.json"))
}

pub fn load_cookie_state() -> Result<Value, String> {
    let path = cookie_file()?;
    let raw = std::fs::read_to_string(&path).map_err(|_| format!("missing {path}; run `similarweb sync-cookies`"))?;
    serde_json::from_str(&raw).map_err(|e| format!("invalid {path}: {e}"))
}

pub fn cookie_header(state: &Value) -> Result<String, String> {
    let list = state["cookies"].as_array().ok_or_else(|| "cookies.json missing cookies array".to_string())?;
    let pairs: Vec<String> = list.iter().filter_map(|c| Some(format!("{}={}", c["name"].as_str()?, c["value"].as_str()?))).collect();
    if pairs.is_empty() { return Err("cookies.json has no cookies".into()); }
    Ok(pairs.join("; "))
}

/// Runs `.claude/skills/similarweb/scripts/sync-cookies.js` on gateway 9223 to mint/refresh `GMITM_ec`
/// and `aws-waf-token` into `~/.local/state/similarweb/cookies.json`.
pub fn sync_cookies(node: Option<u32>) -> Result<Value, String> {
    let out_dir = std::env::temp_dir().join("similarweb-cookie-sync");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("bun");
    cmd.args([LEASE_PAGE, "--label", "similarweb-sync", "--out", &out_dir.to_string_lossy(), "--profile", "9223", "--timeout", "25000", SYNC_SCRIPT]);
    if let Some(n) = node { cmd.env("SIMILARWEB_NODE", n.to_string()); }
    let out = cmd.output().map_err(|e| format!("bun sync-cookies: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("sync-cookies failed: {}", err.lines().last().unwrap_or_default()));
    }
    load_cookie_state()
}

fn build_headers(cookie: &str, sw_page: &str) -> Result<HeaderMap, String> {
    let mut h = HeaderMap::new();
    h.insert(COOKIE, HeaderValue::from_str(cookie).map_err(|e| e.to_string())?);
    h.insert(USER_AGENT, HeaderValue::from_static(UA));
    h.insert(ACCEPT, HeaderValue::from_static("application/json, text/plain, */*"));
    h.insert(REFERER, HeaderValue::from_static("https://pro.similarweb.com/"));
    h.insert("X-Requested-With", HeaderValue::from_static("XMLHttpRequest"));
    if let Ok(v) = HeaderValue::from_str(sw_page) { h.insert("x-sw-page", v); }
    Ok(h)
}

fn is_expired_body(text: &str) -> bool {
    text.trim_start().starts_with('<') || text.contains("授权信息无效") || text.contains("请重新登陆")
}

/// Executes an HTTP request against `pro.similarweb.com` or `pro.similarweb.com`; if the cookie session expired,
/// automatically runs `sync_cookies(None)` once and retries.
pub fn request_json(method: &str, url: &str, sw_page: &str, auto_sync: bool) -> Result<Value, String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let client = Client::builder().timeout(Duration::from_secs(25)).build().map_err(|e| e.to_string())?;
    let mut state = load_cookie_state().or_else(|e| if auto_sync { sync_cookies(None) } else { Err(e) })?;
    for attempt in 0..2 {
        let cookie = cookie_header(&state)?;
        let headers = build_headers(&cookie, sw_page)?;
        let (status, text) = rt.block_on(async {
            let req = if method == "POST" { client.post(url) } else { client.get(url) };
            let res = req.headers(headers).send().await.map_err(|e| format!("{method} {url}: {e}"))?;
            let status = res.status().as_u16();
            let text = res.text().await.map_err(|e| e.to_string())?;
            Ok::<(u16, String), String>((status, text))
        })?;
        if status == 200 && !is_expired_body(&text) {
            return serde_json::from_str(&text).map_err(|e| format!("JSON parse error from {url}: {e}"));
        }
        // A 400 is the API refusing the query (an interval it does not allow), not the session.
        if attempt == 0 && auto_sync && status != 400 {
            state = sync_cookies(None)?;
            continue;
        }
        return Err(format!("HTTP {status} from {url}: {}", text.chars().take(200).collect::<String>()));
    }
    Err("unreachable".into())
}

fn month_end(year: i32, month: u32) -> u32 {
    let (y, m) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).and_then(|d| d.pred_opt()).map_or(30, |d| d.day())
}

/// `(year, month)` of `YYYY-MM` or `YYYY-MM-DD`.
fn parse_month(text: &str) -> Option<(i32, u32)> {
    let (year, rest) = text.split_once('-')?;
    Some((year.parse().ok()?, rest.get(..2).unwrap_or(rest).parse().ok().filter(|m| (1..=12).contains(m))?))
}

/// Monthly window `(from, to)` as `YYYY%7CMM%7CDD`: `to` defaults to the last complete month and
/// `from` to two months before `to`.
pub fn default_window(from_arg: Option<&str>, to_arg: Option<&str>) -> (String, String) {
    let today = Utc::now().date_naive();
    let last_complete = if today.month() == 1 { (today.year() - 1, 12) } else { (today.year(), today.month() - 1) };
    let (end_y, end_m) = to_arg.and_then(parse_month).unwrap_or(last_complete);
    let (start_y, start_m) = from_arg.and_then(parse_month).unwrap_or(if end_m > 2 { (end_y, end_m - 2) } else { (end_y - 1, end_m + 10) });
    (format!("{start_y:04}%7C{start_m:02}%7C01"), format!("{end_y:04}%7C{end_m:02}%7C{:02}", month_end(end_y, end_m)))
}

fn day(date: NaiveDate) -> String { date.format("%Y%%7C%m%%7C%d").to_string() }

/// Last-28-days window ending three days before today (UTC), the lag `pro.similarweb.com` allowed on 2026-11-10.
pub fn window_28d() -> (String, String) {
    let to = Utc::now().date_naive() - chrono::Duration::days(3);
    (day(to - chrono::Duration::days(27)), day(to))
}

/// The window to use after a 400 body `Interval is invalid - A--B. Allowed interval is C--D.`: a
/// daily `C--D` is the rolling window itself; a monthly one (`2025-08--2026-10`) is the whole published
/// range, so the window becomes the three months ending at its last month.
pub fn allowed_interval(error: &str) -> Option<(String, String)> {
    let (from, to) = error.split("Allowed interval is ").nth(1)?.split_once("--")?;
    let as_day = |s: &str| NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok().map(day);
    match (as_day(from), as_day(to)) {
        (Some(from), Some(to)) => Some((from, to)),
        _ => parse_month(to.get(..7)?).map(|_| default_window(None, to.get(..7))),
    }
}

/// Runs `call(from, to)`; when the server refuses the window and names the one it allows, moves
/// `window` there and retries once.
pub fn in_allowed_window<T>(window: &mut (String, String), call: impl Fn(&str, &str) -> Result<T, String>) -> Result<T, String> {
    match call(&window.0, &window.1) {
        Err(e) => match allowed_interval(&e) {
            Some(allowed) if allowed != *window => {
                *window = allowed;
                call(&window.0, &window.1)
            }
            _ => Err(e),
        },
        ok => ok,
    }
}

pub fn fetch_status(auto_sync: bool) -> Result<Value, String> {
    let state = load_cookie_state().or_else(|e| if auto_sync { sync_cookies(None) } else { Err(e) })?;
    let sub = request_json("GET", &format!("{DASH_ORIGIN}/api/subscription/self"), DASH_ORIGIN, auto_sync).unwrap_or(Value::Null);
    let audit = request_json("GET", &format!("{DASH_ORIGIN}/api/auditing/self"), DASH_ORIGIN, false).unwrap_or(Value::Null);
    let ident = request_json("GET", &format!("{SIM_ORIGIN}/api/identities"), SIM_ORIGIN, auto_sync)?;
    Ok(json!({
        "savedAt": state["savedAt"],
        "node": state["node"],
        "cookieCount": state["cookies"].as_array().map_or(0, Vec::len),
        "subscription": sub["data"],
        "auditing": audit["data"],
        "similarwebIdentity": ident,
    }))
}

/// Website keywords over `from`..`to`; `is_window` marks a rolling last-28-days window, whose rows
/// carry `KwWindowVolume` in place of `KwVolume`.
#[allow(clippy::too_many_arguments)]
pub fn fetch_keywords(domain: &str, source: &str, include_branded: bool, include_non_branded: bool, sort: &str, page_num: usize, page_size: usize, country: u32, from: &str, to: &str, is_window: bool, auto_sync: bool) -> Result<Value, String> {
    let sw_page = format!("https://pro.similarweb.com/#/organicsearch/pageAnalysis/website-keyword-v2/*/999/{}", if is_window { "28d" } else { "3m" });
    let pf = format!("%5B%7B%22url%22%3A%22{domain}%22%2C%22searchType%22%3A%22domain%22%7D%5D");
    let url = format!(
        "{SIM_ORIGIN}/widgetApi/WebsiteAnalysisV2/WebsiteAnalysis/Table?sort={sort}&asc=false&country={country}&to={to}&from={from}&isWindow={is_window}&keys={domain}&pageFilterJson={pf}&includeSubDomains=true&IncludeNoneBranded={include_non_branded}&IncludeBranded={include_branded}&page={page_num}&pageSize={page_size}&timeGranularity=Monthly&webSource=Total&sourceType={source}"
    );
    request_json("POST", &url, &sw_page, auto_sync)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monthly_window_ends_on_the_last_day_of_the_named_month() {
        assert_eq!(default_window(None, Some("2026-09-30")), ("2026%7C07%7C01".to_string(), "2026%7C09%7C30".to_string()));
        assert_eq!(default_window(None, Some("2026-02")), ("2025%7C12%7C01".to_string(), "2026%7C02%7C28".to_string()));
        assert_eq!(default_window(Some("2026-01"), Some("2026-01")), ("2026%7C01%7C01".to_string(), "2026%7C01%7C31".to_string()));
    }

    #[test]
    fn refused_window_moves_to_the_interval_the_server_names() {
        // Body `pro.similarweb.com` returned on 2026-11-10 for a window ending one day late.
        let body = r#"HTTP 400 from https://pro.similarweb.com/x: {"Interval":["Interval is invalid - 2026-10-12--2026-11-08. Allowed interval is 2026-10-11--2026-11-07."]}"#;
        let mut window = ("2026%7C10%7C12".to_string(), "2026%7C11%7C08".to_string());
        let calls = std::cell::Cell::new(0);
        let got = in_allowed_window(&mut window, |from, to| {
            calls.set(calls.get() + 1);
            if to == "2026%7C11%7C07" { Ok(from.to_string()) } else { Err(body.to_string()) }
        });
        assert_eq!(got, Ok("2026%7C10%7C11".to_string()));
        assert_eq!((calls.get(), window.1.as_str()), (2, "2026%7C11%7C07"));
        assert_eq!(in_allowed_window(&mut window, |_, _| Err::<(), _>("HTTP 500".to_string())), Err("HTTP 500".to_string()));
    }

    #[test]
    fn unpublished_month_moves_to_the_three_months_ending_at_the_last_published_one() {
        // Body `pro.similarweb.com` returned on 2026-11-10 for a monthly window ending in 2026-11.
        let body = r#"{"Interval":["Interval is invalid - 2026-09-01--2026-11-30. Allowed interval is 2025-08--2026-10.","Unique visitors are not published for 2026-11."]}"#;
        assert_eq!(allowed_interval(body), Some(("2026%7C08%7C01".to_string(), "2026%7C10%7C31".to_string())));
        assert_eq!(allowed_interval("HTTP 400: bad sort"), None);
    }

    #[test]
    fn rolling_window_spans_28_days() {
        let (from, to) = window_28d();
        let parse = |s: &str| NaiveDate::parse_from_str(&s.replace("%7C", "-"), "%Y-%m-%d").unwrap();
        assert_eq!((parse(&to) - parse(&from)).num_days(), 27);
    }
}
