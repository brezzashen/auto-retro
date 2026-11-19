//! Same-need phrasings among the day's `做` words (G7): words with the same content words in any
//! order or number ("ai relationship advice", "relationship advice ai") are one need. One Trends call
//! compares them in double quotes, so Google does not merge them (Gefei, 41ynvpcdrl); the strongest
//! stays the word, the others ride in its section instead of taking a daily slot (one need, one page:
//! c8z1ixs1md). Word order, joined forms and variants can each get their own first page (zd5llz4bnz), so a
//! phrasing folds only when its first page shares `SAME_SERP` domains with the best one's.

use super::monid::{MonidCall, run_all, slug};
use super::watch_birth::curve;
use super::watch_config::DiscoveryConfig;
use super::watch_judge::{DO, Judged, WATCH, score};
use serde_json::json;
use std::collections::BTreeMap;

const STOP: [&str; 12] = ["a", "an", "the", "in", "for", "of", "to", "on", "my", "your", "with", "and"];
/// `做` words, best first, searched for phrasings of one need.
const TOP: usize = 10;
const MAX_GROUPS: usize = 3;
/// Trends compares at most this many words in one call.
const MAX_WORDS: usize = 5;
/// First-page domains two phrasings must share to count as one need.
const SAME_SERP: usize = 5;
/// Weeks the Trends comparison averages.
const WEEKS: usize = 4;

/// Content words, singular, sorted: the need a phrasing expresses.
pub fn need(keyword: &str) -> String {
    let mut words: Vec<String> = keyword.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !STOP.contains(w))
        .map(|w| if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") { w[..w.len() - 1].to_string() } else { w.to_string() }).collect();
    words.sort();
    words.join(" ")
}

fn shared(a: &[(String, f64)], b: &[(String, f64)]) -> usize { a.iter().filter(|(d, _)| b.iter().any(|(e, _)| e == d)).count() }

/// Groups of two or more top `做` words with the same page, need and first page, best first within each group.
pub fn groups(judged: &[Judged]) -> Vec<Vec<usize>> {
    let mut dos: Vec<usize> = (0..judged.len()).filter(|&i| judged[i].verdict == DO).collect();
    dos.sort_by(|&a, &b| score(&judged[b].candidate).total_cmp(&score(&judged[a].candidate)));
    let mut by_need: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for i in dos.into_iter().take(TOP) { by_need.entry((judged[i].candidate.page.clone(), need(&judged[i].candidate.keyword))).or_default().push(i); }
    let same = |g: &mut Vec<usize>| { let best = judged[g[0]].top.clone(); g.retain(|&i| shared(&best, &judged[i].top) >= SAME_SERP); };
    let mut out: Vec<Vec<usize>> = by_need.into_values().map(|mut g| { same(&mut g); g.truncate(MAX_WORDS); g }).filter(|g| g.len() >= 2).collect();
    out.sort_by_key(|g| g[0]);
    out.truncate(MAX_GROUPS);
    out
}

/// Keeps the phrasing with the most quoted Trends interest and folds the others into it; without a
/// Trends answer the best-scored phrasing stays.
pub fn merge(judged: &mut [Judged], cfg: &DiscoveryConfig, dir: &str, errors: &mut Vec<String>) {
    let groups = groups(judged);
    let path = |g: &[usize], judged: &[Judged]| format!("{dir}/synonyms_{}.json", slug(&judged[g[0]].candidate.keyword));
    let calls: Vec<MonidCall> = groups.iter().map(|g| MonidCall { provider: "dataforseo".into(), endpoint: "/keywords/google-trends-explore".into(),
        out: path(g, judged), body: json!({"keywords": g.iter().map(|&i| format!("\"{}\"", judged[i].candidate.keyword)).collect::<Vec<_>>(),
            "location_code": cfg.location_code, "time_range": "past_12_months", "item_types": ["google_trends_graph"]}) }).collect();
    if let Err(e) = run_all(calls, 2) { errors.push(format!("synonyms: {e}")); }
    for g in groups {
        let interest: Vec<f64> = match super::files::read_json(&path(&g, judged)) {
            Ok(json) => (0..g.len()).map(|n| curve(&json, n).iter().rev().take(WEEKS).map(|p| p.1).sum::<f64>()).collect(),
            Err(_) => vec![0.0; g.len()],
        };
        fold(judged, &g, &interest);
    }
}

/// Folds every phrasing of group `g` but the one with the most `interest` into that one.
pub fn fold(judged: &mut [Judged], g: &[usize], interest: &[f64]) {
    let main_at = (0..g.len()).max_by(|&a, &b| interest[a].total_cmp(&interest[b]).then(b.cmp(&a))).unwrap_or(0);
    let top = interest[main_at].max(1.0);
    let main = judged[g[main_at]].candidate.keyword.clone();
    for (n, &i) in g.iter().enumerate().filter(|(n, _)| *n != main_at) {
        let share = (interest[n] / top * 100.0).round();
        judged[i].verdict = WATCH.into();
        judged[i].objection = format!("与「{main}」同一需求，并入它的 section（Trends 热度为其 {share}%）");
        let word = judged[i].candidate.keyword.clone();
        judged[g[main_at]].synonyms.push(format!("{word}（{share}%）"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::watch_discover::Candidate;

    fn j(keyword: &str, volume: f64) -> Judged {
        Judged { verdict: DO.into(), candidate: Candidate { keyword: keyword.into(), page: "/ai-relationship-coach".into(), volume, ..Default::default() }, ..Default::default() }
    }

    #[test]
    fn phrasings_of_one_need_fold_into_the_one_trends_prefers() {
        assert_eq!(need("relationship advice AI"), need("ai relationship advices"));
        assert_ne!(need("red flags in a man"), need("red flags in a guy"));
        let serp = |ds: &[&str]| ds.iter().map(|d| (d.to_string(), 30.0)).collect::<Vec<_>>();
        let page = serp(&["a.com", "b.com", "c.com", "d.com", "e.com", "f.com"]);
        let mut judged = vec![j("ai relationship advice", 900.0), j("relationship advice ai", 700.0), j("ai dating coach", 500.0), j("advice ai relationship", 400.0)];
        (judged[0].top, judged[1].top, judged[3].top) = (page.clone(), page, serp(&["a.com", "x.com", "y.com", "z.com", "w.com"]));
        assert_eq!(groups(&judged), [vec![0, 1]], "the third phrasing has its own first page (1 shared domain), so it stays a word");
        // Quoted Trends, 2026-11-10: "ai relationship advice" null, "relationship advice ai" 100.
        fold(&mut judged, &[0, 1], &[0.0, 400.0]);
        assert_eq!((judged[0].verdict.as_str(), judged[1].verdict.as_str()), (WATCH, DO));
        assert_eq!(judged[1].synonyms, ["ai relationship advice（0%）"]);
        assert!(judged[0].objection.contains("并入"));
    }
}
