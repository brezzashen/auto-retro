//! Persistence of a word past its first month (G14; report `gefei-keyword-method-20261110` section 8,
//! P10 and P14). Gefei, WeChat 2025-04-12: a rise that lasts three months or more shows lasting demand,
//! "如果能持续上升（如3个月以上）表明需求具有延续性".
//! A community member's rule, not Gefei's own (Zhang Yu, contest retro, WeChat 2026-03-10): a word over
//! 30 days old with a competitor domain on it for over 20 days is given up,
//! "这个词出现超过30天，而且有竞品网站域名做了超过20天，我都果断放弃".

use super::files::{at, read_json};
use super::monid::{first, run_all, slug};
use super::watch_config::MethodConfig;
use super::watch_discover::Candidate;
use super::watch_judge::{DO, DROP, Judged, score};
use super::watch_method::whois_call;
use super::watch_signals::{FRESH_DAYS, NEWBORN_DAYS};
use chrono::{Duration, NaiveDate};

/// Weeks of the 5-year curve averaged into one month.
const BLOCK_WEEKS: usize = 4;
/// Months in a row a word older than `NEWBORN_DAYS` must rise to keep its rising credit.
pub const SUSTAINED_MONTHS: u32 = 3;
/// Days a competitor domain holding the word must have existed for the member's rule.
const RIVAL_DOMAIN_DAYS: i64 = 20;
/// A domain registered up to this many days before the Trends birth still counts as built for the word.
const BIRTH_SLACK_DAYS: i64 = 30;

/// Consecutive 4-week blocks of complete weeks, counted back from the latest, whose mean is above the
/// block before it.
pub fn rising_months(weekly: &[(NaiveDate, f64)]) -> u32 {
    let means: Vec<f64> = weekly.rchunks(BLOCK_WEEKS).filter(|b| b.len() == BLOCK_WEEKS)
        .map(|b| b.iter().map(|p| p.1).sum::<f64>() / BLOCK_WEEKS as f64).collect();
    means.windows(2).take_while(|w| w[0] > w[1]).count() as u32
}

pub fn sustained(c: &Candidate) -> bool { c.rising_months >= SUSTAINED_MONTHS }

/// A hot word past its window: it appeared 31 to 365 days ago and has not risen three months in a row.
pub fn past_window(c: &Candidate) -> bool {
    c.seasonal == 0 && c.age_days.is_some_and(|d| d > NEWBORN_DAYS && d <= FRESH_DAYS) && !sustained(c)
}

/// Why a word past its window lost the new-word credit; empty for any other word.
pub fn note(c: &Candidate) -> String {
    if !past_window(c) { return String::new(); }
    format!("；Trends {} 天前出现，只连续上升 {} 个月（需 {SUSTAINED_MONTHS} 个月），不计新词加权", c.age_days.unwrap_or_default(), c.rising_months)
}

/// The member's rule on `做` words past their window (at most `domainChecksPerDay`, four words or fewer):
/// one whois count of domains named exactly after the word (any TLD), registered after its birth and more
/// than 20 days ago.
pub fn rival_domains(judged: &mut [Judged], m: &MethodConfig, dir: &str, today: NaiveDate, errors: &mut Vec<String>) {
    let mut picks: Vec<usize> = (0..judged.len()).filter(|&i| judged[i].verdict == DO && past_window(&judged[i].candidate)
        && judged[i].candidate.keyword.split_whitespace().count() <= 4).collect();
    picks.sort_by(|&a, &b| score(&judged[b].candidate).total_cmp(&score(&judged[a].candidate)));
    picks.truncate(m.domain_checks_per_day);
    let before = (today - Duration::days(RIVAL_DOMAIN_DAYS)).to_string();
    let path = |k: &str| format!("{dir}/whois_old_{}.json", slug(k));
    let calls = picks.iter().map(|&i| {
        let c = &judged[i].candidate;
        let since = today - Duration::days(c.age_days.unwrap_or_default() + BIRTH_SLACK_DAYS);
        whois_call(&c.keyword, &since.to_string(), Some(&before), path(&c.keyword))
    }).collect();
    if let Err(e) = run_all(calls, 2) { errors.push(format!("whois (old domains): {e}")); }
    for i in picks {
        let Ok(json) = read_json(&path(&judged[i].candidate.keyword)) else { continue };
        let Some(n) = at(first(&json), &["total_count"]).as_u64().or_else(|| at(first(&json), &["items_count"]).as_u64()) else { continue };
        give_up(&mut judged[i], n as usize);
    }
}

/// Drops a word past its window when a competitor domain has held it for more than 20 days.
pub fn give_up(j: &mut Judged, old_domains: usize) {
    if old_domains == 0 || !past_window(&j.candidate) { return; }
    j.verdict = DROP.into();
    j.reason = format!("群友规则（张瑜 2026-03-10 比赛复盘，非哥飞本人）：出现 {} 天且没有连续上升 {SUSTAINED_MONTHS} 个月，已有 {old_domains} 个含该词的域名注册超过 {RIVAL_DOMAIN_DAYS} 天",
        j.candidate.age_days.unwrap_or_default());
}

#[cfg(test)]
#[path = "watch_persist_tests.rs"]
mod tests;
