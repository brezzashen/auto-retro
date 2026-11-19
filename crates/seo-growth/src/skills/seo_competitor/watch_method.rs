//! Paid probes of Gefei's keyword method in the daily round (report `gefei-keyword-method-20261110`):
//! live Google Ads volume and Trends-reference scaling for words the Labs database shows as 0 (G2, G7),
//! the birth date and season of the best candidates from 5-year and 30-day Trends curves (G1, G4),
//! freshly registered domains holding a new word (G6: the window closes within a week, 8kzo7qkezp),
//! and one quoted Trends comparison per group of same-need phrasings (G7, 41ynvpcdrl).

use super::files::{at, num_or, read_json, text};
use super::monid::{MonidCall, first, run_all, slug};
use super::watch_birth::{age_days, birth, curve, scaled_volume, seasonal_month};
use super::watch_persist::rising_months;
use super::watch_config::{DiscoveryConfig, MethodConfig};
use super::watch_judge::{DO, DROP, Judged, prejudge, score};
use super::watch_signals::{demand, fresh};
use super::watch_trends_surge::{ads_keyword, monthly};
use chrono::NaiveDate;
use serde_json::{Value, json};

fn trends(keywords: Vec<String>, range: &str, location: u32, out: String) -> MonidCall {
    MonidCall { provider: "dataforseo".into(), endpoint: "/keywords/google-trends-explore".into(), out,
        body: json!({"keywords": keywords, "location_code": location, "time_range": range, "item_types": ["google_trends_graph"]}) }
}

/// True for a source that finds words before keyword tools count them.
fn early(source: &str) -> bool { ["trends:", "anomaly:", "site-loop:", "similarweb-new:"].iter().any(|p| source.starts_with(p)) }

/// Google Ads volume for every live word the Labs database had no volume for; one call.
pub fn ads_volume(judged: &mut [Judged], cfg: &DiscoveryConfig, dir: &str, errors: &mut Vec<String>) {
    let ask: Vec<String> = judged.iter().filter(|j| j.verdict != DROP && j.candidate.volume == 0.0 && j.candidate.latest == 0.0)
        .filter_map(|j| ads_keyword(&j.candidate.keyword)).take(1000).collect();
    if ask.is_empty() { return; }
    let out = format!("{dir}/ads.json");
    let call = MonidCall { provider: "dataforseo".into(), endpoint: "/keywords/google-ads-search-volume".into(), out: out.clone(),
        body: json!({"keywords": ask, "location_code": cfg.location_code, "language_code": cfg.language_code}) };
    if let Err(e) = run_all(vec![call], 1) { errors.push(format!("ads volume: {e}")); return; }
    let Ok(Value::Array(rows)) = read_json(&out) else { errors.push("ads volume: no rows".into()); return };
    for row in &rows {
        let k = text(&row["keyword"]).to_lowercase();
        let Some(j) = judged.iter_mut().find(|j| ads_keyword(&j.candidate.keyword).as_deref() == Some(k.as_str())) else { continue };
        j.candidate.volume = num_or(&row["search_volume"], 0.0);
        if let Some(m) = monthly(row) { (j.candidate.latest, j.candidate.base) = (m.latest, m.base); }
        j.candidate.first_month = super::watch_signals::first_month(row);
    }
}

/// The candidates that get Trends curves: half early-source words still short of volume, the rest `做` words by score.
/// A word Trends cannot take (over 10 words or 80 characters) is never picked: its call would carry an empty
/// keyword (2026-11-10 08:00 round, 2 failed calls).
pub fn probe_picks(judged: &[Judged], cfg: &DiscoveryConfig, n: usize) -> Vec<usize> {
    let takes = |i: &usize| ads_keyword(&judged[*i].candidate.keyword).is_some();
    let mut news: Vec<usize> = (0..judged.len()).filter(takes).filter(|&i| judged[i].verdict != DROP && early(&judged[i].candidate.source)
        && demand(&judged[i].candidate) < cfg.min_volume).collect();
    news.sort_by(|&a, &b| judged[a].candidate.keyword.cmp(&judged[b].candidate.keyword));
    let mut dos: Vec<usize> = (0..judged.len()).filter(takes).filter(|&i| judged[i].verdict == DO).collect();
    dos.sort_by(|&a, &b| score(&judged[b].candidate).total_cmp(&score(&judged[a].candidate)));
    let mut out: Vec<usize> = news.into_iter().take(n / 2).collect();
    out.extend(dos.into_iter().take(n - out.len()));
    out
}

/// Birth date, season and scaled volume of the picked candidates, then their verdicts again.
pub fn probe(judged: &mut [Judged], cfg: &DiscoveryConfig, m: &MethodConfig, brands: &[String], dir: &str, today: NaiveDate, errors: &mut Vec<String>) {
    let picks = probe_picks(judged, cfg, m.trends_per_day);
    let path = |kind: &str, k: &str| format!("{dir}/{kind}_{}.json", slug(k));
    let mut calls = Vec::new();
    for &i in &picks {
        let k = ads_keyword(&judged[i].candidate.keyword).unwrap_or_default();
        calls.push(trends(vec![k.clone()], "past_5_years", cfg.location_code, path("trend5y", &k)));
        calls.push(trends(vec![k.clone(), m.reference.keyword.clone()], "past_30_days", cfg.location_code, path("trend30", &k)));
    }
    if let Err(e) = run_all(calls, 4) { errors.push(format!("trends probe: {e}")); }
    for &i in &picks {
        let k = ads_keyword(&judged[i].candidate.keyword).unwrap_or_default();
        let (Ok(years), Ok(month)) = (read_json(&path("trend5y", &k)), read_json(&path("trend30", &k))) else { continue };
        let weekly = curve(&years, 0);
        let c = &mut judged[i].candidate;
        c.seasonal = seasonal_month(&weekly).unwrap_or(0);
        c.rising_months = rising_months(&weekly);
        c.age_days = age_days(birth(&weekly), birth(&curve(&month, 0)), today).filter(|d| !ads_older(&c.first_month, today, *d));
        if demand(c) < cfg.min_volume { c.trends_volume = scaled_volume(&month, m.reference.volume).unwrap_or(0.0); }
    }
    hourly(judged, &picks, cfg, dir, errors);
    for i in picks { judged[i] = prejudge(&judged[i].candidate, cfg, brands); }
}

/// A 7-day curve for every picked word born within 30 days (Gefei, iwe36cwkry: a hot word is read by the
/// day and hour); `fade` is its mean over the last day against the curve's peak.
fn hourly(judged: &mut [Judged], picks: &[usize], cfg: &DiscoveryConfig, dir: &str, errors: &mut Vec<String>) {
    let born: Vec<(usize, String)> = picks.iter().copied().filter(|&i| super::watch_signals::newborn(&judged[i].candidate))
        .map(|i| (i, format!("{dir}/trend7d_{}.json", slug(&judged[i].candidate.keyword)))).collect();
    let calls = born.iter().map(|(i, p)| trends(vec![ads_keyword(&judged[*i].candidate.keyword).unwrap_or_default()], "past_7_days", cfg.location_code, p.clone())).collect();
    if let Err(e) = run_all(calls, 4) { errors.push(format!("trends 7 days: {e}")); }
    for (i, p) in born {
        let Ok(json) = read_json(&p) else { continue };
        judged[i].candidate.fade = fade(&curve(&json, 0));
    }
}

/// Mean of the last day's points (a 7-day curve is hourly) over the curve's peak; 0 without a peak.
pub fn fade(points: &[(NaiveDate, f64)]) -> f64 {
    let peak = points.iter().map(|p| p.1).fold(0.0, f64::max);
    let Some(last) = points.last().map(|p| p.0) else { return 0.0 };
    let day: Vec<f64> = points.iter().filter(|p| p.0 == last).map(|p| p.1).collect();
    if peak <= 0.0 || day.is_empty() { 0.0 } else { day.iter().sum::<f64>() / day.len() as f64 / peak }
}

/// True when Google Ads counted searches more than a month before the Trends birth: a low-volume word's
/// Trends curve is mostly null, and denser sampling looks like a birth (verified 2026-11-10: "transcribe
/// voice notes", 1,600 searches in 2025-10, read as 16 days old).
pub fn ads_older(first_month: &str, today: NaiveDate, age: i64) -> bool {
    let born = today - chrono::Duration::days(age + 31);
    !first_month.is_empty() && first_month < born.format("%Y-%m").to_string().as_str()
}

/// Count of domains holding the word registered after `since` (`YYYY-MM-DD`); with `before`, of domains named
/// exactly after the word (any TLD) registered between the two dates.
pub(super) fn whois_call(keyword: &str, since: &str, before: Option<&str>, out: String) -> MonidCall {
    let compact: String = keyword.split_whitespace().collect();
    let hyphen = keyword.split_whitespace().collect::<Vec<_>>().join("-");
    let after = json!(["created_datetime", ">", since]);
    let window = before.map_or(after.clone(), |b| json!([after, "and", ["created_datetime", "<", b]]));
    // The dated count decides a drop, so it takes only a domain whose name is the word; "contains" matched
    // 24 domains for "red flags" in 8 months, "checkeredflagsportsinc.org" among them (2026-11-10).
    let like = |w: &str| if before.is_some() { format!("{w}.%") } else { format!("%{w}%") };
    MonidCall { provider: "dataforseo".into(), endpoint: "/domain/whois".into(), out,
        body: json!({"filters": [[["domain", "like", like(&compact)], "or", ["domain", "like", like(&hyphen)]], "and", window],
            "order_by": ["created_datetime,desc"], "limit": 10}) }
}

/// G6: how many domains holding a new `做` word of at most four words were registered in 30 days. A note, never a
/// veto: "不能一概而论地认为新词一定是一波流" (u7fwqnm12g), and a registered domain is a reason to move first (iwe36cwkry).
pub fn domain_window(judged: &mut [Judged], m: &MethodConfig, dir: &str, today: NaiveDate, errors: &mut Vec<String>) {
    let mut picks: Vec<usize> = (0..judged.len()).filter(|&i| judged[i].verdict == DO && fresh(&judged[i].candidate)
        && judged[i].candidate.keyword.split_whitespace().count() <= 4).collect();
    picks.sort_by(|&a, &b| score(&judged[b].candidate).total_cmp(&score(&judged[a].candidate)));
    picks.truncate(m.domain_checks_per_day);
    let since = (today - chrono::Duration::days(30)).to_string();
    let path = |k: &str| format!("{dir}/whois_{}.json", slug(k));
    let calls = picks.iter().map(|&i| whois_call(&judged[i].candidate.keyword, &since, None, path(&judged[i].candidate.keyword))).collect();
    if let Err(e) = run_all(calls, 2) { errors.push(format!("whois: {e}")); }
    for i in picks {
        let Ok(json) = read_json(&path(&judged[i].candidate.keyword)) else { continue };
        // An empty answer carries "total_count": null with "items_count": 0.
        let Some(n) = at(first(&json), &["total_count"]).as_u64().or_else(|| at(first(&json), &["items_count"]).as_u64()).map(|n| n as usize) else { continue };
        close_window(&mut judged[i], n, m.max_fresh_domains);
    }
}

pub fn close_window(j: &mut Judged, fresh_domains: usize, max: usize) {
    j.fresh_domains = Some(fresh_domains);
    if fresh_domains >= max {
        j.objection.push_str(&format!("；近 30 天已注册 {fresh_domains} 个含该词的域名，窗口正在关闭，今天上线"));
    } else if fresh_domains > 0 {
        j.objection.push_str(&format!("；近 30 天已有 {fresh_domains} 个同名域名注册，尽快上线"));
    }
}

#[cfg(test)]
#[path = "watch_method_tests.rs"]
mod tests;
