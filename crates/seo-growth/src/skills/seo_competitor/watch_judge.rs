//! The judge, data helper and devil's advocate of the daily keyword room: rule verdicts on every
//! scouted keyword, first-page SERPs with real domains and backlink ranks for the best ones, the
//! win rate, and one objection per keyword that can still demote it.

use super::files::{at, items, read_json, serp_domain, text};
use super::metrics::win_rate;
use super::monid::{MonidCall, first, run_all, slug};
use super::watch_config::{DiscoveryConfig, Seed};
use super::watch_discover::{Candidate, Rival, phrase_words, shares_stem};
use super::watch_signals as signals;
use super::{watch_money as money, watch_persist as persist};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

pub const DO: &str = "做";
pub const WATCH: &str = "观察";
pub const DROP: &str = "放弃";
/// Backlink rank from which a first-page domain counts as a giant.
pub const GIANT_RANK: f64 = 80.0;
/// Win-rate bonus when the top 3 are all inner pages (Gefei, tspz9aydgq: low KD because no homepage ranks).
const INNER_TOP3_BONUS: f64 = 0.1;
/// Gefei trap 3 (60wmnukjzg): huge searches with near-zero difficulty are usually junk or misread.
const ODD_VOLUME: f64 = 10_000.0;
const ODD_KD: f64 = 5.0;

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Judged {
    #[serde(flatten)]
    pub candidate: Candidate,
    pub verdict: String,
    pub reason: String,
    pub win_rate: Option<f64>,
    /// First-page `(domain, rank)`; rank 0 when DataForSEO has none.
    pub top: Vec<(String, f64)>,
    pub ai_overview: bool,
    /// Homepages among the first-page organic results, and whether the top 3 are all inner pages (G3).
    pub homepages: usize,
    pub inner_top3: bool,
    /// Result type in which Google answers the query itself (G4); empty when none.
    pub direct: String,
    /// The query Google searched instead of a misspelled keyword (G4); empty when none.
    pub respelled: String,
    /// Domains holding the word registered in the last 30 days, when checked (G6).
    pub fresh_domains: Option<usize>,
    /// Same-need phrasings merged into this word's section, with their Trends share of it (G7).
    pub synonyms: Vec<String>,
    /// The weakest point the devil's advocate found; empty before it ran.
    pub objection: String,
}

fn words(text: &str) -> Vec<&str> { text.split(|c: char| !c.is_alphanumeric() && c != '.').filter(|w| !w.is_empty()).collect() }

/// True when `keyword` names a brand: a one-word brand must be a whole word, a longer one a substring.
pub fn is_brand(keyword: &str, brands: &[String]) -> bool {
    let ws = words(keyword);
    brands.iter().any(|b| if b.contains(' ') { keyword.contains(b.as_str()) } else { ws.contains(&b.as_str()) })
}

/// True when a word of `keyword` is a product model number ("icd-ux570"): letters and digits in one
/// token of five or more characters. Product names are a trap (Gefei, by8y500z8c, fca2guh3u2).
pub fn is_model(keyword: &str) -> bool {
    words(keyword).iter().any(|w| w.chars().count() >= 5 && w.chars().any(|c| c.is_ascii_digit()) && w.chars().any(|c| c.is_alphabetic())
        && !w.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// The landing page a keyword fits: the seed that found it when on-topic, else the seed sharing the most stems.
pub fn fit_page<'a>(c: &Candidate, seeds: &'a [Seed]) -> Option<&'a Seed> {
    if let Some(s) = seeds.iter().find(|s| s.page == c.page && shares_stem(&c.keyword, &s.fit)) { return Some(s); }
    let hits = |s: &Seed| words(&c.keyword).iter().filter(|w| shares_stem(w, &s.fit)).count()
        + s.fit.iter().map(|p| phrase_words(&c.keyword, p)).sum::<usize>();
    seeds.iter().filter(|s| hits(s) > 0).max_by_key(|s| hits(s))
}

/// The rule verdict before any SERP is read.
pub fn prejudge(c: &Candidate, cfg: &DiscoveryConfig, brands: &[String]) -> Judged {
    let seed = fit_page(c, &cfg.seeds);
    let judged_page = c.source.starts_with("anomaly:").then(|| c.page.clone());
    let page = seed.map(|s| s.page.clone()).or(judged_page).unwrap_or_default();
    let verdict = |v: &str, reason: String| Judged { candidate: Candidate { page: page.clone(), ..c.clone() }, verdict: v.into(), reason, ..Default::default() };
    if c.keyword.contains("mars") || is_brand(&c.keyword, brands) { return verdict(DROP, "品牌词".into()); }
    if cfg.blocked.iter().any(|b| words(&c.keyword).contains(&b.as_str()) || phrase_words(&c.keyword, b) > 0) { return verdict(DROP, "品牌安全".into()); }
    if is_model(&c.keyword) { return verdict(DROP, "产品型号词".into()); }
    if c.intent == "navigational" && !seed.is_some_and(|s| s.core_market) { return verdict(DROP, "导航意图".into()); }
    if c.kd.is_some_and(|kd| kd > cfg.drop_kd) { return verdict(DROP, format!("KD {} 过高", c.kd.unwrap_or_default())); }
    if page.is_empty() { return verdict(DROP, "与 Mars 功能不对口".into()); }
    if c.seasonal > 0 { return verdict(WATCH, format!("季节词：Trends 5 年里每年 {} 月高峰", c.seasonal)); }
    if let (Some(rule), Some(kd)) = (seed.and_then(|s| s.evergreen.as_ref()), c.kd)
        && c.volume >= rule.min_volume && kd <= rule.max_kd {
        return verdict(DO, format!("常青需求：月搜 {}、KD {kd}、趋势 季 {:+}% / 年 {:+}%", c.volume, c.quarterly, c.yearly));
    }
    let demand = signals::demand(c);
    if demand < cfg.min_volume { return verdict(WATCH, format!("月搜 {demand} 不足 {}", cfg.min_volume)); }
    match c.kd {
        None if !signals::fresh(c) => verdict(WATCH, format!("缺 KD{}", persist::note(c))),
        Some(kd) if kd > cfg.max_kd => verdict(WATCH, format!("KD {kd} 偏高")),
        _ if c.quarterly <= 0.0 && c.yearly <= 0.0 && !signals::surging(c) && !signals::fresh(c) => verdict(WATCH, format!("近季度、近一年和近月都没有上升{}", persist::note(c))),
        _ => verdict(DO, format!("月搜 {}、KD {}、趋势 季 {:+}% / 年 {:+}%{}", c.volume, c.kd.map_or("—".into(), |k| k.to_string()), c.quarterly, c.yearly, signals::notes(c))),
    }
}

/// Ordering of keywords judged `做`: searches (latest month when higher) weighted by the stronger
/// of the two trends, then by the surge and priced-click bonuses.
pub fn score(c: &Candidate) -> f64 { signals::demand(c) * (1.0 + c.quarterly.max(c.yearly).max(0.0) / 100.0) * signals::weight(c) }

/// Fetches the SERP of the best `serp_per_day` pre-judged keywords and the backlink rank of every
/// first-page domain, then records win rate, AI Overview and the devil's advocate on each.
pub fn verify(judged: &mut [Judged], cfg: &DiscoveryConfig, dir: &str, errors: &mut Vec<String>) {
    let mut picks: Vec<usize> = (0..judged.len()).filter(|&i| judged[i].verdict == DO).collect();
    picks.sort_by(|&a, &b| score(&judged[b].candidate).total_cmp(&score(&judged[a].candidate)));
    picks.truncate(cfg.serp_per_day);
    let serp_path = |k: &str| format!("{dir}/serp_{}.json", slug(k));
    let calls = picks.iter().map(|&i| MonidCall { provider: "dataforseo".into(), endpoint: "/serp/google-organic".into(),
        body: json!({"keyword": judged[i].candidate.keyword, "location_code": cfg.location_code, "language_code": cfg.language_code, "depth": 10}),
        out: serp_path(&judged[i].candidate.keyword) }).collect();
    if let Err(e) = run_all(calls, 6) { errors.push(format!("serp: {e}")); }
    let mut domains = vec![cfg.domain.clone()];
    for &i in &picks {
        let Ok(serp) = read_json(&serp_path(&judged[i].candidate.keyword)) else { continue };
        let results = items(at(first(&serp), &["items"]));
        let organic: Vec<&serde_json::Value> = results.iter().filter(|r| text(at(r, &["type"])) == "organic").take(10).collect();
        let shape = signals::shape(&organic);
        let j = &mut judged[i];
        j.ai_overview = results.iter().any(|r| text(at(r, &["type"])) == "ai_overview");
        (j.homepages, j.inner_top3) = (shape.homepages, shape.inner_top3);
        j.direct = signals::direct_answer(results).unwrap_or_default();
        j.respelled = signals::respelled(first(&serp)).unwrap_or_default();
        (j.candidate.price_pages, j.candidate.app_listings) = money::serp_signals(&organic);
        j.top = organic.iter().map(|r| (serp_domain(r), 0.0)).filter(|(d, _)| !d.is_empty()).collect();
        domains.extend(j.top.iter().map(|(d, _)| d.clone()).filter(|d| !domains.contains(d)).collect::<Vec<_>>());
    }
    domains.sort();
    domains.dedup();
    let rank_path = format!("{dir}/ranks.json");
    let rank_call = MonidCall { provider: "dataforseo".into(), endpoint: "/backlinks/bulk-ranks".into(), body: json!({"targets": domains, "rank_scale": "one_hundred"}), out: rank_path.clone() };
    if let Err(e) = run_all(vec![rank_call], 1) { errors.push(format!("ranks: {e}")); }
    let ranks: BTreeMap<String, f64> = read_json(&rank_path).map(|j| items(at(first(&j), &["items"])).iter()
        .map(|i| (text(at(i, &["target"])).to_string(), at(i, &["rank"]).as_f64().unwrap_or(0.0))).collect()).unwrap_or_default();
    let ours = ranks.get(&cfg.domain).copied().unwrap_or(0.0);
    let ranked = !ranks.is_empty();
    for &i in &picks {
        let j = &mut judged[i];
        if j.top.is_empty() { j.verdict = WATCH.into(); j.objection = "没有拿到前 10 结果，明天重试".into(); continue; }
        if !ranked { j.verdict = WATCH.into(); j.objection = "没有拿到域名权重，明天重试".into(); continue; }
        for (d, r) in j.top.iter_mut() { *r = ranks.get(d.as_str()).copied().unwrap_or(if d == "google.com" { 100.0 } else { 0.0 }); }
        let rate = win_rate(&j.top.iter().map(|(d, r)| Some(signals::contest_rank(d, *r))).collect::<Vec<_>>(), j.candidate.kd, ours);
        let rate = if j.inner_top3 { (rate + INNER_TOP3_BONUS).min(1.0) } else { rate };
        j.win_rate = Some((rate * 100.0).round() / 100.0);
        devil(j, cfg.min_win_rate);
    }
    for j in judged.iter_mut().filter(|j| j.verdict == DO && j.win_rate.is_none()) {
        j.verdict = WATCH.into();
        j.objection = "今天的 SERP 名额已用完，明天补数".into();
    }
}

/// The devil's advocate: the weakest point of a `做` keyword; a vetoing point demotes it to `观察`.
/// Forum posts are single threads, so they never count as giants.
pub fn devil(j: &mut Judged, min_win_rate: f64) {
    let rate = j.win_rate.unwrap_or(0.0);
    let giants = j.top.iter().filter(|(d, r)| signals::contest_rank(d, *r) >= GIANT_RANK).count();
    let demand = signals::demand(&j.candidate);
    let kd = j.candidate.kd.unwrap_or(100.0);
    let (veto, point) = if !j.respelled.is_empty() {
        (true, format!("typo 词：谷歌改搜「{}」", j.respelled))
    } else if !j.direct.is_empty() {
        (true, format!("谷歌直接给答案（{}），站点只能分到很少点击", j.direct))
    } else if rate < min_win_rate {
        (true, format!("胜率 {rate:.2} 低于 {min_win_rate:.2}"))
    } else if giants * 2 >= j.top.len() {
        (true, format!("前 10 有 {giants} 个大站（rank ≥ {GIANT_RANK}）"))
    } else if j.homepages * 2 >= j.top.len() {
        (true, format!("前 10 有 {} 个首页，没有内页空位", j.homepages))
    } else if demand >= ODD_VOLUME && kd <= ODD_KD && !j.inner_top3 {
        (true, format!("月搜 {demand} 而 KD {kd}，前 3 有首页：量大 KD 奇低，先人工核实"))
    } else if j.ai_overview {
        (false, "首屏有 AI Overview，点击会被分走".to_string())
    } else if demand < 100.0 {
        (false, format!("月搜只有 {demand}"))
    } else {
        (false, "未发现明显弱点".to_string())
    };
    if veto { j.verdict = WATCH.into(); }
    let forums = signals::forums(&j.top, 5);
    let mut objection = point;
    if !veto && forums >= signals::FORUM_TOP5 { objection.push_str(&format!("；前 5 有 {forums} 个论坛帖，还没有专门页面承接")); }
    if !veto && j.inner_top3 { objection.push_str("；前 3 全是内页"); }
    if !veto { objection.push_str(&money::serp_note(&j.candidate)); }
    j.objection = objection;
}

/// Every registry brand plus the configured extra ones, lowercase.
pub fn brands(rivals: &[Rival], cfg: &DiscoveryConfig) -> Vec<String> {
    rivals.iter().flat_map(|r| r.brand.iter().cloned()).chain(cfg.extra_brands.iter().map(|b| b.to_lowercase())).collect()
}

#[cfg(test)]
#[path = "watch_judge_tests.rs"]
mod tests;
