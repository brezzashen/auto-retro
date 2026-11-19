//! Anomaly operators of the keyword room (founder 2026-11-07, report `seo-anomaly-operators-20261107`):
//! momentum of a keyword's latest month against its own 11-month level, the forum share of its first
//! page, and the advertiser price against organic difficulty. DataForSEO's `search_volume` is the
//! 12-month mean (399 of 400 ledger words checked), so a riser is judged on its latest month.

use super::files::{at, is_forum};
use super::watch_discover::Candidate;
use super::watch_trends_surge::{monthly, ratio};
use serde_json::Value;

/// A latest month at least this many times the 11 months before it is a surge.
pub const SURGE_RATIO: f64 = 2.0;
/// Latest-month searches a surge needs.
pub const SURGE_MIN: f64 = 100.0;
/// Forum posts among the first five results from which no site serves the query yet.
pub const FORUM_TOP5: usize = 2;
/// Advertisers pay at least this per click while organic difficulty stays at most `PRICED_KD`.
const PRICED_CPC: f64 = 3.0;
const PRICED_KD: f64 = 15.0;
/// Days within which a word counts as new; past `NEWBORN_DAYS` it must also have risen three months in a row (G14).
pub const FRESH_DAYS: i64 = 365;
pub const NEWBORN_DAYS: i64 = 30;
/// Last-day share of the 7-day peak under which a hot word is noted as fading (Gefei, iwe36cwkry; a note only:
/// "不能一概而论地认为新词一定是一波流", u7fwqnm12g).
const FADED: f64 = 0.5;

/// `(latest month, mean of the up to 11 months before it)` of a Labs keyword object; zeros without history.
pub fn months(data: &Value) -> (f64, f64) {
    monthly(at(data, &["keyword_info"])).map_or((0.0, 0.0), |m| (m.latest, m.base))
}

/// `YYYY-MM` of the first month whose Google Ads searches reach a tenth of the word's best month, from
/// an object holding `monthly_searches`; empty without history.
pub fn first_month(holder: &Value) -> String {
    let mut ms: Vec<(i64, i64, f64)> = super::files::items(at(holder, &["monthly_searches"])).iter()
        .filter_map(|m| Some((m["year"].as_i64()?, m["month"].as_i64()?, m["search_volume"].as_f64().unwrap_or(0.0)))).collect();
    ms.sort_by_key(|m| (m.0, m.1));
    let max = ms.iter().map(|m| m.2).fold(0.0, f64::max);
    ms.iter().find(|m| max > 0.0 && m.2 >= max / 10.0).map_or(String::new(), |m| format!("{}-{:02}", m.0, m.1))
}

/// Latest month over its 11-month mean; 0 without history.
pub fn surge(c: &Candidate) -> f64 { if c.latest > 0.0 { ratio(c.latest, c.base) } else { 0.0 } }

pub fn surging(c: &Candidate) -> bool { c.latest >= SURGE_MIN && surge(c) >= SURGE_RATIO }

/// Monthly searches the judge counts: the mean, the latest month or the Trends-scaled estimate, whichever is highest.
pub fn demand(c: &Candidate) -> f64 { c.volume.max(c.latest).max(c.trends_volume) }

/// A word that appeared within a year counts as rising (Gefei, conkcr1mid); within 30 days is best (0vz7te71m0).
/// Past 30 days the credit needs a sustained rise (`watch_persist`, WeChat 2025-04-12).
pub fn fresh(c: &Candidate) -> bool {
    c.seasonal == 0 && c.age_days.is_some_and(|d| d <= NEWBORN_DAYS || (d <= FRESH_DAYS && super::watch_persist::sustained(c)))
}
pub fn newborn(c: &Candidate) -> bool { c.age_days.is_some_and(|d| d <= NEWBORN_DAYS) && c.seasonal == 0 }

/// Advertisers price the click but organic results do not compete for it.
pub fn priced(c: &Candidate) -> bool { c.cpc >= PRICED_CPC && c.kd.is_some_and(|kd| kd <= PRICED_KD) }

/// Ordering bonus: ×1.5 for a surge, the money signals of `watch_money` (×1.5 for a priced click), ×2 for a word
/// born within 30 days, ×1.3 within a year.
pub fn weight(c: &Candidate) -> f64 {
    let age = if newborn(c) { 2.0 } else if fresh(c) { 1.3 } else { 1.0 };
    (if surging(c) { 1.5 } else { 1.0 }) * super::watch_money::weight(c) * age
}

/// Shape of one first page (G3, Gefei tspz9aydgq, k2z73gxq2h): homepages among the organic results
/// and whether the top 3 are all inner pages. A homepage has a URL breadcrumb with no `›` path.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shape { pub homepages: usize, pub inner_top3: bool }

pub fn is_homepage(item: &Value) -> bool {
    let crumb = super::files::text(at(item, &["breadcrumb"]));
    let url = super::files::text(at(item, &["url"]));
    if crumb.contains("://") { return !crumb.contains('›'); }
    let path = url.split_once("://").map_or("", |(_, r)| r).split_once('/').map_or("", |(_, p)| p);
    !url.contains("google.com/goto") && !url.is_empty() && path.trim_matches('/').is_empty()
}

pub fn shape(organic: &[&Value]) -> Shape {
    let homes: Vec<bool> = organic.iter().take(10).map(|r| is_homepage(r)).collect();
    Shape { homepages: homes.iter().filter(|h| **h).count(), inner_top3: homes.len() >= 3 && homes.iter().take(3).all(|h| !h) }
}

/// Result types where Google answers the query itself (Gefei trap 2 "谷歌直达", 60wmnukjzg; a site keeps about 8%, u73lwbkov8).
const DIRECT: [&str; 3] = ["answer_box", "calculator", "currency_box"];

pub fn direct_answer(results: &[Value]) -> Option<String> {
    results.iter().map(|r| super::files::text(at(r, &["type"]))).find(|t| DIRECT.contains(t)).map(str::to_string)
}

/// The query Google searched instead when it treats the keyword as a misspelling (Gefei, elibtfox5q).
pub fn respelled(serp: &Value) -> Option<String> {
    let kind = super::files::text(at(serp, &["spell", "type"]));
    let word = super::files::text(at(serp, &["spell", "keyword"]));
    (["showing_results_for", "did_you_mean", "including_results_for"].contains(&kind) && !word.is_empty()).then(|| word.to_lowercase())
}

/// Forum posts among the first `n` results.
pub fn forums(top: &[(String, f64)], n: usize) -> usize { top.iter().take(n).filter(|(d, _)| is_forum(d)).count() }

/// The rank the win rate reads for one first-page result: a forum post is one thread, beatable by a page built for the query.
pub fn contest_rank(domain: &str, rank: f64) -> f64 { if is_forum(domain) { 0.0 } else { rank } }

/// The operators that fired, for the verdict reason; empty when none did.
pub fn notes(c: &Candidate) -> String {
    let mut out = Vec::new();
    if surging(c) { out.push(format!("近月 {} = 前 11 月均 {:.0} ×{:.1}", c.latest, c.base, surge(c))); }
    if priced(c) { out.push(format!("CPC ${:.2} 而 KD {}", c.cpc, c.kd.unwrap_or_default())); }
    out.extend(super::watch_money::notes(c));
    if let Some(d) = c.age_days.filter(|_| fresh(c)) { out.push(format!("Trends 显示 {d} 天前出现")); }
    if c.fade > 0.0 && c.fade < FADED { out.push(format!("近 24 小时热度只剩 7 天峰值的 {:.0}%", c.fade * 100.0)); }
    if c.trends_volume > c.volume.max(c.latest) { out.push(format!("Trends 参照词换算月搜约 {}", c.trends_volume)); }
    let stale = super::watch_persist::note(c);
    if out.is_empty() { stale } else { format!("；{}{stale}", out.join("；")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn word(volume: f64, latest: f64, base: f64, cpc: f64, kd: f64) -> Candidate {
        Candidate { keyword: "w".into(), volume, latest, base, cpc, kd: Some(kd), ..Default::default() }
    }

    #[test]
    fn a_riser_counts_its_latest_month_and_its_surge() {
        // "how to grow your network", 2026-11-04: mean 170, September 1000 over an 11-month mean of 78.
        let c = word(170.0, 1000.0, 78.2, 0.0, 0.0);
        assert!(surging(&c) && demand(&c) == 1000.0 && weight(&c) == 1.5);
        assert!(notes(&c).contains("×12.8"));
        assert!(!surging(&word(40.0, 90.0, 30.0, 0.0, 0.0)), "under 100 searches is no surge");
        assert!(!surging(&word(590.0, 720.0, 480.0, 0.0, 0.0)), "×1.5 is no surge");
    }

    #[test]
    fn a_priced_click_needs_cpc_and_low_difficulty() {
        assert!(priced(&word(70.0, 0.0, 0.0, 51.92, 0.0)));
        assert!(!priced(&word(70.0, 0.0, 0.0, 51.92, 30.0)) && !priced(&word(70.0, 0.0, 0.0, 0.5, 0.0)));
    }

    #[test]
    fn monthly_history_reads_from_keyword_info() {
        let ms: Vec<_> = (1..=12).map(|m| json!({"year": 2026, "month": m, "search_volume": if m == 12 { 600 } else { 100 }})).collect();
        assert_eq!(months(&json!({"keyword_info": {"monthly_searches": ms}})), (600.0, 100.0));
        assert_eq!(months(&json!({"keyword_info": {}})), (0.0, 0.0));
    }

    #[test]
    fn a_new_word_ranks_by_its_age_unless_seasonal() {
        let aged = |age: i64, seasonal: u32| Candidate { age_days: Some(age), seasonal, rising_months: 3, ..word(100.0, 0.0, 0.0, 0.0, 10.0) };
        assert_eq!((weight(&aged(12, 0)), weight(&aged(200, 0)), weight(&aged(800, 0)), weight(&aged(12, 2))), (2.0, 1.3, 1.0, 1.0));
        let spiked = Candidate { rising_months: 1, ..aged(200, 0) };
        assert!(!fresh(&spiked) && weight(&spiked) == 1.0 && notes(&spiked).contains("只连续上升 1 个月"), "G14: past 30 days a word must keep rising");
        assert!(fresh(&Candidate { rising_months: 0, ..aged(12, 0) }), "a word within 30 days needs no history");
        assert!(notes(&aged(12, 0)).contains("12 天前出现"));
        let scaled = Candidate { trends_volume: 1800.0, ..word(0.0, 0.0, 0.0, 0.0, 10.0) };
        assert_eq!(demand(&scaled), 1800.0);
        assert!(notes(&scaled).contains("约 1800"));
    }

    #[test]
    fn homepages_are_read_from_url_breadcrumbs() {
        // Shapes from the 2026-11-09 SERP cache: a goto link with a path breadcrumb, a bare-host breadcrumb, a forum post.
        let inner = json!({"url": "https://google.com/goto?url=x", "breadcrumb": "https://www.scienceofpeople.com › Blog › Relationships"});
        let home = json!({"url": "https://google.com/goto?url=y", "breadcrumb": "https://flirtcopilot.com"});
        let post = json!({"url": "https://google.com/goto?url=z", "breadcrumb": "30+ answers  ·  4 months ago", "website_name": "Quora"});
        assert!(is_homepage(&home) && !is_homepage(&inner) && !is_homepage(&post));
        assert!(is_homepage(&json!({"url": "https://www.text.com/"})) && !is_homepage(&json!({"url": "https://text.com/blog/x"})));
        assert_eq!(shape(&[&inner, &post, &inner, &home]), Shape { homepages: 1, inner_top3: true });
        assert_eq!(shape(&[&home, &inner, &inner]), Shape { homepages: 1, inner_top3: false });
    }

    #[test]
    fn a_direct_answer_and_a_respelled_query_are_found() {
        assert_eq!(direct_answer(&[json!({"type": "ai_overview"}), json!({"type": "answer_box"})]).as_deref(), Some("answer_box"));
        assert_eq!(direct_answer(&[json!({"type": "ai_overview"}), json!({"type": "featured_snippet"})]), None, "a snippet still links a site");
        assert_eq!(respelled(&json!({"spell": {"keyword": "Text Message Analyzer", "type": "showing_results_for"}})).as_deref(), Some("text message analyzer"));
        assert_eq!(respelled(&json!({"spell": null})), None);
    }

    #[test]
    fn forum_posts_are_beatable_and_counted() {
        let top = vec![("youtube.com".to_string(), 100.0), ("quora.com".into(), 95.0), ("openup.com".into(), 40.0)];
        assert_eq!(forums(&top, 5), 2);
        assert_eq!((contest_rank("quora.com", 95.0), contest_rank("openup.com", 40.0)), (0.0, 40.0));
    }
}
