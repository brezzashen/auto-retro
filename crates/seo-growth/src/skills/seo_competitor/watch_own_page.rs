//! G8, founder's choice C in report `gefei-keyword-room-update-20261110` (2026-11-10): a `做` word with at
//! least `ownPageVolume` monthly searches whose need no existing page or target covers gets its own landing
//! page, at most `maxNewPagesPerDay` (3); the other picks and its same-need phrasings ride in sections.
//! Gefei builds one site per word (c8z1ixs1md); example.com keeps one product site, so a strong distinct
//! need becomes one inner page there ("老站靠内页", 1xldegdtlt).

use super::watch_config::{DiscoveryConfig, TargetKeyword};
use super::watch_judge::{DO, Judged};
use super::watch_signals::demand;
use super::watch_synonyms::need;

/// Words too common to tie two needs together.
const WEAK: [&str; 8] = ["ai", "app", "how", "what", "free", "best", "online", "tool"];

fn words(phrase: &str) -> Vec<String> { need(phrase).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect() }

/// The page or target phrase that already covers `keyword`'s need: it holds at least half of its content
/// words, common words not counted ("pocket ai recorder" → `/ai-voice-recorder`; "breakup text generator"
/// shares only "text" with `/text-message-analyzer`).
pub fn covered_by(keyword: &str, cfg: &DiscoveryConfig, targets: &[TargetKeyword]) -> Option<String> {
    let mine: Vec<String> = words(keyword).into_iter().filter(|w| !WEAK.contains(&w.as_str())).collect();
    let needed = mine.len().div_ceil(2).max(1);
    let pages = cfg.seeds.iter().flat_map(|s| [s.page.clone(), s.keyword.clone()]).chain(targets.iter().flat_map(|t| [t.page.clone(), t.keyword.clone()]));
    pages.into_iter().find(|p| words(&p.replace(['/', '-'], " ")).iter().filter(|w| mine.contains(w)).count() >= needed)
}

/// The path a new page for `keyword` takes.
pub fn path(keyword: &str) -> String {
    format!("/{}", keyword.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-"))
}

/// `(keyword, path)` of the SERP-checked `做` words, best first, that get their own page today.
pub fn choose(judged: &[Judged], cfg: &DiscoveryConfig, targets: &[TargetKeyword]) -> Vec<(String, String)> {
    judged.iter().filter(|j| j.verdict == DO && j.win_rate.is_some() && demand(&j.candidate) >= cfg.own_page_volume)
        .filter(|j| covered_by(&j.candidate.keyword, cfg, targets).is_none())
        .take(cfg.max_new_pages_per_day).map(|j| (j.candidate.keyword.clone(), path(&j.candidate.keyword))).collect()
}

/// The day's new pages from all `做` words (best first): one outside the picks takes the slot of the last
/// pick that is no new page,
/// since the picks lean to words existing pages already cover (2026-11-10 dry run: the top 3 were covered,
/// "transcribe voice notes" with 1,300 searches was fourth).
pub fn reserve(judged: &[Judged], picks: &mut Vec<Judged>, cfg: &DiscoveryConfig, targets: &[TargetKeyword]) -> Vec<(String, String)> {
    let own = choose(judged, cfg, targets);
    for (k, _) in &own {
        if picks.iter().any(|p| p.candidate.keyword == *k) { continue; }
        let Some(j) = judged.iter().find(|j| j.candidate.keyword == *k) else { continue };
        let mine = |p: &Judged| own.iter().any(|(o, _)| *o == p.candidate.keyword);
        if picks.len() >= cfg.max_per_day.max(1) {
            let Some(last) = picks.iter().rposition(|p| !mine(p)) else { break };
            picks.remove(last);
        }
        picks.push(j.clone());
    }
    own
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::seo_competitor::watch_discover::Candidate;

    fn cfg() -> DiscoveryConfig { capped(1) }

    fn capped(new_pages: usize) -> DiscoveryConfig {
        serde_json::from_value(serde_json::json!({"hourSgt": 8, "locationCode": 2840, "languageCode": "en", "domain": "example.com", "registry": "r.json",
            "seeds": [{"keyword": "red flags", "page": "/relationship-red-flags", "fit": ["flag"]},
                {"keyword": "ai voice recorder", "page": "/ai-voice-recorder", "fit": ["recorder"]}],
            "extraBrands": [], "blocked": [], "minVolume": 50, "maxKd": 30, "dropKd": 40, "minWinRate": 0.45, "serpPerDay": 15,
            "maxPerDay": 3, "maxNewPagesPerDay": new_pages})).unwrap()
    }

    fn pick(keyword: &str, volume: f64) -> Judged {
        Judged { candidate: Candidate { keyword: keyword.into(), volume, ..Default::default() }, verdict: DO.into(), win_rate: Some(0.6), ..Default::default() }
    }

    #[test]
    fn a_strong_word_with_a_need_no_page_covers_gets_one_new_page() {
        let targets = [TargetKeyword { keyword: "text message analyzer".into(), page: "/text-message-analyzer".into() }];
        // Shapes of the 2026-11-10 dry run: two words a page already covers, then two uncovered ones.
        let picks = [pick("red flags in a guy", 8100.0), pick("pocket ai recorder", 2400.0), pick("breakup text generator", 5400.0),
            pick("ai dating profile writer", 1300.0), pick("ai breakup letter", 400.0)];
        assert_eq!(covered_by("red flags in a guy", &cfg(), &targets).as_deref(), Some("/relationship-red-flags"));
        assert_eq!(covered_by("pocket ai recorder", &cfg(), &targets).as_deref(), Some("/ai-voice-recorder"));
        assert_eq!(covered_by("ai text message reply", &cfg(), &targets).as_deref(), Some("/text-message-analyzer"));
        assert_eq!(covered_by("ai dating profile writer", &cfg(), &targets), None, "\"ai\" alone ties no needs");
        assert_eq!(choose(&picks, &cfg(), &targets), [("breakup text generator".to_string(), "/breakup-text-generator".to_string())], "at most one a day");
        assert!(choose(&picks[4..], &cfg(), &targets).is_empty(), "under 1,000 searches stays a section");
        let unchecked = Judged { win_rate: None, ..pick("breakup text generator", 5400.0) };
        assert!(choose(&[unchecked], &cfg(), &targets).is_empty(), "no new page without a first-page check");
    }

    #[test]
    fn the_day_s_new_page_word_takes_the_last_pick_slot() {
        let judged = [pick("red flags in a guy", 8100.0), pick("pocket ai recorder", 8100.0), pick("green flags in a relationship", 2900.0), pick("breakup text generator", 1300.0)];
        let mut picks = judged[..3].to_vec();
        let own = reserve(&judged, &mut picks, &cfg(), &[]);
        let kept: Vec<&str> = picks.iter().map(|p| p.candidate.keyword.as_str()).collect();
        assert_eq!((own.len(), kept), (1, vec!["red flags in a guy", "pocket ai recorder", "breakup text generator"]));
    }

    #[test]
    fn up_to_three_new_page_words_take_slots_without_pushing_each_other_out() {
        // Founder's choice C: `maxNewPagesPerDay` 3.
        let judged = [pick("red flags in a guy", 8100.0), pick("pocket ai recorder", 8100.0), pick("green flags in a relationship", 2900.0),
            pick("breakup text generator", 1300.0), pick("transcribe voice notes", 1300.0), pick("ai dating profile writer", 1200.0), pick("ai breakup letter", 1100.0)];
        let mut picks = judged[..3].to_vec();
        let own = reserve(&judged, &mut picks, &capped(3), &[]);
        let kept: Vec<&str> = picks.iter().map(|p| p.candidate.keyword.as_str()).collect();
        assert_eq!(own.len(), 3, "the fourth uncovered word waits");
        assert_eq!(kept, ["breakup text generator", "transcribe voice notes", "ai dating profile writer"]);
    }
}
