use super::*;
use crate::skills::seo_competitor::watch_config::{Inspection, read_config};

const SITE: &str = "https://example.com";
const LIVE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/seo_competitor/watch.config.json");

/// The live config with the founder's cap of 3 and two seed pages: one from before the room, one it built.
fn config() -> WatchConfig {
    let mut c = read_config(LIVE).unwrap();
    let d = c.discovery.as_mut().unwrap();
    d.seeds = ["/ai-voice-recorder", "/win-him-back"].iter().map(|p| serde_json::from_value(serde_json::json!({"keyword": p.trim_start_matches('/'), "page": p, "fit": []})).unwrap()).collect();
    (d.max_new_pages_per_day, d.new_page_gate) = (3, GateConfig::default());
    c
}

fn now() -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339("2026-11-20T08:17:00+08:00").unwrap() }

/// A state directory whose first round ran on 2026-11-11 and reserved three new pages over two days.
fn rounds() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |day: &str, own: serde_json::Value| {
        std::fs::create_dir_all(dir.path().join(format!("discover/{day}"))).unwrap();
        std::fs::write(dir.path().join(format!("discover/{day}/round.json")), serde_json::json!({"day": day, "ownPages": own}).to_string()).unwrap();
    };
    write("2026-11-11", serde_json::json!([["breakup text generator", "/breakup-text-generator"]]));
    write("2026-11-12", serde_json::json!([["transcribe voice notes", "/transcribe-voice-notes"], ["ai dating profile writer", "/ai-dating-profile-writer"]]));
    std::fs::create_dir_all(dir.path().join("discover/week-2026-W46")).unwrap();
    dir
}

/// `(path, first seen, coverage, last crawl)`; `/ai-voice-recorder` entered the sitemap the day before the room's first round.
fn state(pages: &[(&str, &str, &str, &str)]) -> WatchState {
    let mut s = WatchState::default();
    s.first_seen.insert(format!("{SITE}/ai-voice-recorder"), "2026-11-10T16:17:00+08:00".into());
    s.inspections.insert(format!("{SITE}/ai-voice-recorder"), Inspection { coverage: INDEXED.into(), last_crawl: "2026-11-10T10:00:00Z".into(), ..Default::default() });
    for (path, seen, coverage, crawl) in pages {
        s.first_seen.insert(format!("{SITE}{path}"), format!("{seen}T09:17:00+08:00"));
        if !coverage.is_empty() { s.inspections.insert(format!("{SITE}{path}"), Inspection { coverage: coverage.to_string(), last_crawl: crawl.to_string(), ..Default::default() }); }
    }
    s
}

fn report(clicks: &[(&str, f64)]) -> Value {
    serde_json::json!({"rows": clicks.iter().map(|(p, c)| serde_json::json!({"keys": [format!("{SITE}{p}")], "clicks": c, "impressions": 40})).collect::<Vec<_>>()})
}

fn run(s: &WatchState, clicks: &Value, at: DateTime<FixedOffset>) -> Gate { gate(&config(), s, rounds().path().to_str().unwrap(), clicks, at) }

#[test]
fn the_cap_stays_at_three_while_new_pages_are_indexed_and_every_one_has_clicks() {
    let s = state(&[("/breakup-text-generator", "2026-11-11", INDEXED, "2026-11-11T10:09:32Z"), ("/transcribe-voice-notes", "2026-11-12", INDEXED, "2026-11-12T03:00:00Z"),
        ("/win-him-back", "2026-11-13", INDEXED, "2026-11-13T03:00:00Z"), ("/ja/breakup-text-generator", "2026-11-12", "URL is unknown to Google", "")]);
    let g = run(&s, &report(&[("/breakup-text-generator", 2.0), ("/transcribe-voice-notes", 1.0), ("/win-him-back", 1.0)]), now());
    assert_eq!((g.cap, g.open), (3, true));
    let paths: Vec<&str> = g.pages.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(paths, ["/breakup-text-generator", "/transcribe-voice-notes", "/win-him-back"],
        "a page from before the room's first round has no clicks and is not the room's; a locale copy is no page");
    assert!(g.note.contains("收录 3/3（100%，门槛 80%），有点击 3/3 → 达标，今天最多新建 3 页"), "{}", g.note);
}

#[test]
fn a_page_without_clicks_or_a_low_index_rate_falls_back_to_one_page_a_day() {
    let indexed = [("/breakup-text-generator", "2026-11-11", INDEXED, "2026-11-11T10:09:32Z"), ("/transcribe-voice-notes", "2026-11-12", INDEXED, "2026-11-12T03:00:00Z")];
    let silent = run(&state(&indexed), &report(&[("/breakup-text-generator", 2.0)]), now());
    assert_eq!((silent.cap, silent.open), (1, false), "indexed 2/2 but one page has no click");
    assert!(silent.note.contains("有点击 1/2 → 不达标，回落，今天最多新建 1 页"), "{}", silent.note);
    let half = state(&[indexed[0], ("/transcribe-voice-notes", "2026-11-12", "Crawled - currently not indexed", "2026-11-12T03:00:00Z")]);
    let low = run(&half, &report(&[("/breakup-text-generator", 2.0), ("/transcribe-voice-notes", 1.0)]), now());
    assert_eq!((low.cap, low.pages[1].status.as_str()), (1, NOT_INDEXED), "50% is under the 80% threshold");
    let unread = run(&state(&indexed), &Value::Null, now());
    assert_eq!((unread.cap, unread.open), (1, false), "a failed click read never keeps the cap at three");
}

#[test]
fn a_page_google_has_not_crawled_yet_waits_outside_the_rate() {
    let s = state(&[("/breakup-text-generator", "2026-11-11", INDEXED, "2026-11-11T10:09:32Z"),
        ("/transcribe-voice-notes", "2026-11-18", INDEXED, "2026-11-18T03:00:00Z"),
        ("/ai-dating-profile-writer", "2026-11-15", "URL is unknown to Google", "")]);
    let g = run(&s, &report(&[("/breakup-text-generator", 2.0)]), now());
    let status: Vec<&str> = g.pages.iter().map(|p| p.status.as_str()).collect();
    assert_eq!(status, [IN_INDEX, PENDING, PENDING], "5 days old and not crawled, and 2 days old, both wait");
    assert_eq!((g.cap, g.open), (3, true));
    assert!(g.note.contains("新建关键词页 3 个，其中 2 个待定"));
    // Seven days after its first sighting an uncrawled page counts as not indexed.
    let stuck = run(&s, &report(&[("/breakup-text-generator", 2.0), ("/transcribe-voice-notes", 1.0)]), now() + Duration::days(3));
    assert_eq!((stuck.pages[1].status.as_str(), stuck.cap), (NOT_INDEXED, 1));
    let empty = run(&state(&[]), &report(&[]), now());
    assert_eq!((empty.cap, empty.pages.len()), (3, 0), "no new page yet: the first batch runs at the full cap");
    let old = run(&state(&[("/breakup-text-generator", "2026-11-11", "Crawled - currently not indexed", "2026-11-12T00:00:00Z")]), &report(&[]), now() + Duration::days(6));
    assert_eq!((old.cap, old.pages.len()), (3, 0), "15 days old is outside the 14-day window");
}

#[test]
fn a_click_on_a_locale_copy_counts_for_its_page() {
    let indexed = [("/breakup-text-generator", "2026-11-11", INDEXED, "2026-11-11T10:09:32Z"), ("/transcribe-voice-notes", "2026-11-12", INDEXED, "2026-11-12T03:00:00Z")];
    // The 2026-11-10 shape: the English URL has impressions only, its zh-TW copy the clicks.
    let g = run(&state(&indexed), &report(&[("/breakup-text-generator", 0.0), ("/zh-TW/breakup-text-generator", 11.0), ("/ja/transcribe-voice-notes", 1.0), ("/wiki/transcribe-voice-notes", 5.0)]), now());
    assert_eq!((g.cap, g.open, g.pages[0].clicks, g.pages[1].clicks), (3, true, 11.0, 1.0), "{}", g.note);
    let prefixes = ["/ja".to_string(), "/zh-TW".to_string()];
    let clicks: BTreeMap<String, f64> = [(format!("{SITE}/zh-TW/personal-crm"), 1.0), (format!("{SITE}/personal-crm"), 2.0), (format!("{SITE}/"), 9.0),
        (format!("{SITE}/jazz-notes"), 4.0), (format!("{SITE}/wiki/personal-crm"), 5.0), ("https://other.example/personal-crm".to_string(), 7.0)].into();
    let sums = by_page(&clicks, SITE, &prefixes);
    assert_eq!((sums["/personal-crm"], sums["/"], sums["/jazz-notes"], sums["/wiki/personal-crm"]), (3.0, 9.0, 4.0, 5.0), "only a configured locale prefix is a copy");
    assert_eq!(sums.len(), 4, "another site's row is ignored");
}

#[test]
fn the_room_s_pages_are_its_seed_pages_and_the_new_pages_its_rounds_reserved() {
    let dir = rounds();
    let (paths, start) = room_pages(&config(), dir.path().to_str().unwrap());
    assert_eq!(start, NaiveDate::from_ymd_opt(2026, 11, 11), "the first round on disk; a week directory is no round");
    assert_eq!(paths.into_iter().collect::<Vec<_>>(), ["/ai-dating-profile-writer", "/ai-voice-recorder", "/breakup-text-generator", "/transcribe-voice-notes", "/win-him-back"]);
    let none = tempfile::tempdir().unwrap();
    let g = gate(&config(), &state(&[("/win-him-back", "2026-11-13", "Crawled - currently not indexed", "2026-11-13T03:00:00Z")]), none.path().to_str().unwrap(), &report(&[]), now());
    assert_eq!((g.cap, g.pages.len()), (3, 0), "no round on disk yet: nothing is the room's");
}

#[test]
fn the_three_gate_keys_read_from_the_config_and_default_when_absent() {
    let cfg: GateConfig = serde_json::from_value(serde_json::json!({"windowDays": 7, "minIndexRate": 0.5, "fallbackCap": 2})).unwrap();
    assert_eq!((cfg.window_days, cfg.min_index_rate, cfg.fallback_cap, cfg.grace_days), (7, 0.5, 2, 3));
    let d: GateConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!((d.window_days, d.min_index_rate, d.fallback_cap), (14, 0.8, 1));
    let g = read_config(LIVE).unwrap().discovery.unwrap().new_page_gate;
    assert_eq!((g.window_days, g.min_index_rate, g.fallback_cap), (14, 0.8, 1), "founder's choice B, 2026-11-10");
}
