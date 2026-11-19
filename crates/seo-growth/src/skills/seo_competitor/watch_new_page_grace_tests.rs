use super::*;

const SITE: &str = "https://example.com";

fn finding(key: &str) -> Finding {
    Finding { key: key.into(), severity: "P1".into(), title: "t".into(), action: "a".into() }
}

fn target(keyword: &str, page: &str) -> TargetKeyword {
    TargetKeyword { keyword: keyword.into(), page: page.into() }
}

fn keys(findings: &[Finding]) -> Vec<(&str, &str)> {
    findings.iter().map(|f| (f.key.as_str(), f.severity.as_str())).collect()
}

#[test]
fn absent_and_deep_words_of_a_new_page_are_held_and_old_pages_pass() {
    let now = DateTime::parse_from_rfc3339("2026-11-12T22:17:00+08:00").unwrap();
    let mut state = WatchState::default();
    state.first_seen.insert(format!("{SITE}/how-to-find-clients"), "2026-11-12T16:17:05+08:00".into());
    state.first_seen.insert(format!("{SITE}/personal-crm"), "2026-11-02T06:45:19+08:00".into());
    let targets = [target("find clients", "/how-to-find-clients"), target("how to find clients", "/how-to-find-clients"), target("personal crm", "/personal-crm")];
    let mut findings = [finding("rank:absent:find clients"), finding("rank:deep:how to find clients"), finding("rank:absent:personal crm"), finding("stall:find clients:1")];
    apply(&mut findings, &targets, SITE, &mut state, now);
    assert_eq!(keys(&findings), [("rank:absent:find clients:new", "P2"), ("rank:deep:how to find clients:new", "P2"), ("rank:absent:personal crm", "P1"), ("stall:find clients:1", "P1")]);
    assert!(findings[0].title.contains("上线第 1 天"));
}

#[test]
fn an_unlisted_page_is_held_with_the_same_key_until_the_grace_runs_out() {
    let first = DateTime::parse_from_rfc3339("2026-11-15T15:17:00+08:00").unwrap();
    let mut state = WatchState::default();
    let targets = [target("managing up", "/managing-up"), target("向上管理", "/zh-TW/managing-up")];
    let mut findings = [finding("rank:absent:managing up"), finding("rank:absent:向上管理")];
    apply(&mut findings, &targets, SITE, &mut state, first);
    assert_eq!(keys(&findings), [("rank:absent:managing up:new", "P2"), ("rank:absent:向上管理:new", "P2")]);
    assert!(findings[1].title.contains("/zh-TW/managing-up 尚未进入 sitemap 第 1 天"));
    assert_eq!(state.unlisted_since.len(), 2);

    let later = first + Duration::days(INDEX_GRACE_DAYS);
    let mut again = [finding("rank:absent:managing up")];
    apply(&mut again, &targets, SITE, &mut state, later);
    assert_eq!(keys(&again), [("rank:absent:managing up", "P1")]);
}

#[test]
fn a_listed_page_drops_its_unlisted_mark_and_counts_from_its_sitemap_sighting() {
    let now = DateTime::parse_from_rfc3339("2026-11-25T15:17:00+08:00").unwrap();
    let mut state = WatchState::default();
    state.unlisted_since.insert(format!("{SITE}/managing-up"), "2026-11-15T15:17:00+08:00".into());
    state.first_seen.insert(format!("{SITE}/managing-up"), "2026-11-24T10:17:00+08:00".into());
    let mut findings = [finding("rank:absent:managing up")];
    apply(&mut findings, &[target("managing up", "/managing-up")], SITE, &mut state, now);
    assert_eq!(keys(&findings), [("rank:absent:managing up:new", "P2")]);
    assert!(state.unlisted_since.is_empty());
}
