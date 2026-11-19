//! Report-only rule for target words with no copy, index or internal-link action left (founder
//! 2026-11-13, report `seo-daily-retro-20261113` item 2): a `rank:absent:` word whose page is indexed,
//! carries every word stem in its title or main text and is linked from its home page lacks referring
//! domains, not copy. It is shown at P2 and handed to a tab again only after the product-listing ledger
//! holds more live listings that link the page than when it was held. Bing's site-wide inbound-link count
//! was rejected as the signal: it grew by 5 about every 3 weeks whatever page the links pointed at.

use super::watch_analyze::Finding;
use super::watch_config::{TargetKeyword, WatchState};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LinkGap {
    /// Directory of the product-listing ledger files, relative to the repository root the watchdog runs in.
    pub ledger: String,
    /// Live listings linking the page that must be added above the held baseline before the word is raised again.
    pub min_new_links: i64,
}

/// Key suffix of a held finding; a raise appends the baseline it rose from, so each raise is new work once.
const HELD: &str = ":links";
const INDEXED: &str = "Submitted and indexed";

fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn between<'a>(html: &'a str, open: &str, close: &str) -> &'a str {
    let Some(start) = html.find(open) else { return "" };
    let rest = &html[start..];
    rest.find(close).map_or(rest, |end| &rest[..end])
}

/// The text a reader of the page sees first: its `<title>` and its first `<main>`, tags removed.
fn page_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in format!("{} {}", between(html, "<title", "</title>"), between(html, "<main", "</main>")).chars() {
        match c {
            '<' => { in_tag = true; text.push(' '); }
            '>' => in_tag = false,
            _ if !in_tag => text.extend(c.to_lowercase()),
            _ => {}
        }
    }
    text
}

/// Every word of the keyword appears on the page: a short Latin word exactly, a longer one by its first
/// five letters, any other script as a substring. Latin words of one or two letters other than `ai` are skipped.
fn covered(keyword: &str, text: &str) -> bool {
    let page = words(text);
    words(keyword).iter().all(|word| {
        if !word.is_ascii() { return text.contains(word.as_str()); }
        if word.len() <= 2 && word != "ai" { return true; }
        if word.len() <= 3 { return page.iter().any(|w| w == word); }
        page.iter().any(|w| w.starts_with(&word[..word.len().min(5)]))
    })
}

/// `(home URL, path it should link)` pairs that prove an internal link: the site root for the page itself, and for
/// a `/xx/page` locale path its locale root or the site root linking the unprefixed page (`/ja`, `/ko` and `/th`
/// have no home; their pages hang off the English page's hreflang cluster).
fn homes(site: &str, page: &str) -> Vec<(String, String)> {
    let mut parts = page.trim_matches('/').split('/');
    match (parts.next(), parts.next()) {
        (Some(locale), Some(base)) => vec![(format!("{site}/{locale}"), page.to_string()), (format!("{site}/"), format!("/{base}"))],
        _ => vec![(format!("{site}/"), page.to_string())],
    }
}

/// Live ledger listings whose rendered link, or submitted URL when no link was recorded, is this page.
pub fn ledger_links(dir: &str, url: &str) -> i64 {
    let same = |value: &serde_json::Value| value.as_str().is_some_and(|href| href.trim_end_matches('/') == url.trim_end_matches('/'));
    let links = |file: std::fs::DirEntry| super::files::read_json(&file.path().to_string_lossy()).ok()
        .filter(|entry| entry["status"] == "live" && (same(&entry["link"]["href"]) || (entry["link"]["href"].is_null() && same(&entry["productUrl"]))));
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(links).count() as i64
}

pub fn fetch(url: &str) -> Option<String> {
    let resp = crate::http::send(reqwest::Method::GET, url, &[], None, Some(std::time::Duration::from_secs(20))).ok()?;
    (resp.status == 200).then(|| String::from_utf8_lossy(&resp.body).into_owned())
}

/// Whether the word is still held at this count of live listings; moves the baseline when it is.
fn still_held(word: &str, now: i64, gap: &LinkGap, state: &mut WatchState) -> Result<(), (i64, i64)> {
    let base = *state.link_gap.entry(word.to_string()).or_insert(now);
    let gained = now >= base + gap.min_new_links;
    if gained && !state.handled.contains(&format!("rank:absent:{word}{HELD}:{base}")) { return Err((base, now)); }
    // A handed-over raise and a lower count both restart the baseline; a smaller gain keeps it, so slow growth still adds up.
    if gained || now < base { state.link_gap.insert(word.to_string(), now); }
    Ok(())
}

/// Holds the P1 absent-rank findings whose page leaves only a link gap, and raises them once per link gain.
pub fn apply(findings: &mut [Finding], targets: &[TargetKeyword], site: &str, state: &mut WatchState, gap: &LinkGap, fetch: &dyn Fn(&str) -> Option<String>, links: &dyn Fn(&str) -> i64) {
    let mut pages: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut get = |url: &str| pages.entry(url.to_string()).or_insert_with(|| fetch(url)).clone();
    for finding in findings.iter_mut().filter(|f| f.severity == "P1") {
        let Some(word) = finding.key.strip_prefix("rank:absent:").map(str::to_string) else { continue };
        let Some(target) = targets.iter().find(|t| t.keyword == word) else { continue };
        let url = format!("{site}{}", target.page);
        if state.inspections.get(&url).map(|i| i.coverage.as_str()) != Some(INDEXED) { continue; }
        if !get(&url).is_some_and(|html| covered(&word, &page_text(&html))) { continue; }
        if !homes(site, &target.page).iter().any(|(home, path)| get(home).is_some_and(|html| html.contains(&format!("href=\"{path}\"")))) { continue; }
        match still_held(&word, links(&url), gap, state) {
            Ok(()) => {
                finding.key.push_str(HELD);
                finding.severity = "P2".into();
                finding.title = format!("{}（{} 已收录、页面已有这个词、首页有内链：外链缺口，只报告）", finding.title, target.page);
                finding.action = format!("不改文案：缺的是外部引用，由 product-listing 补外链（创始人 2026-11-13）；台账里链接到该页的已上线收录比降级时多 {} 条后本项会重新报出", gap.min_new_links);
            }
            Err((base, now)) => {
                finding.key = format!("rank:absent:{word}{HELD}:{base}");
                finding.title = format!("{}（链接到该页的已上线收录 {base} → {now}，外链增加后复查）", finding.title);
                finding.action = format!("外链增加后复查 {}：先看当天 SERP 与 Search Console 是否有了排名或展示，再决定是否需要改标题或 H2", target.page);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::watch_config::Inspection;

    const SITE: &str = "https://example.com";
    const PAGE: &str = "<title>AI Email Reply Drafts | ExampleAI</title><header>The AI assistant</header><main><h1>AI email reply drafts</h1><h2>Help me reply: any message</h2></main>";
    const HOME: &str = "<a href=\"/ai-email-reply\">AI email reply</a>";

    fn finding(key: &str) -> Finding {
        Finding { key: key.into(), severity: "P1".into(), title: "t".into(), action: "a".into() }
    }

    fn state(urls: &[&str]) -> WatchState {
        let mut state = WatchState::default();
        for url in urls { state.inspections.insert((*url).into(), Inspection { coverage: INDEXED.into(), ..Inspection::default() }); }
        state
    }

    fn site(url: &str) -> Option<String> {
        match url { "https://example.com/ai-email-reply" | "https://example.com/orphan" => Some(PAGE.into()), "https://example.com/" => Some(HOME.into()), _ => None }
    }

    #[test]
    fn a_word_is_held_only_when_indexed_on_the_page_and_linked_from_home() {
        let targets = ["ai email", "help me reply", "ai email assistant", "ai email reply"].map(|k| TargetKeyword { keyword: k.into(), page: "/ai-email-reply".into() })
            .into_iter().chain([TargetKeyword { keyword: "email reply".into(), page: "/orphan".into() }, TargetKeyword { keyword: "ai email".into(), page: "/unindexed".into() }]).collect::<Vec<_>>();
        let mut findings = [finding("rank:absent:ai email"), finding("rank:absent:help me reply"), finding("rank:absent:ai email assistant"), finding("rank:absent:email reply"), finding("rank:deep:ai email reply")];
        let mut state = state(&["https://example.com/ai-email-reply", "https://example.com/orphan"]);
        apply(&mut findings, &targets, SITE, &mut state, &LinkGap { ledger: String::new(), min_new_links: 1 }, &site, &|_| 0);
        let keys: Vec<(&str, &str)> = findings.iter().map(|f| (f.key.as_str(), f.severity.as_str())).collect();
        assert_eq!(keys, [("rank:absent:ai email:links", "P2"), ("rank:absent:help me reply:links", "P2"), ("rank:absent:ai email assistant", "P1"), ("rank:absent:email reply", "P1"), ("rank:deep:ai email reply", "P1")]);
        assert_eq!(state.link_gap.get("ai email"), Some(&0));
        assert!(findings[0].title.contains("外链缺口") && findings[0].action.contains("多 1 条"));
    }

    #[test]
    fn a_link_gain_raises_the_word_once_and_holds_it_again_after_the_hand_over() {
        let targets = [TargetKeyword { keyword: "ai email".into(), page: "/ai-email-reply".into() }];
        let gap = LinkGap { ledger: String::new(), min_new_links: 5 };
        let mut state = state(&["https://example.com/ai-email-reply"]);
        state.link_gap.insert("ai email".into(), 106);
        let run = |state: &mut WatchState, links: i64| { let mut f = [finding("rank:absent:ai email")]; apply(&mut f, &targets, SITE, state, &gap, &site, &|_| links); (f[0].key.clone(), f[0].severity.clone()) };
        assert_eq!(run(&mut state, 110), ("rank:absent:ai email:links".into(), "P2".into()));
        assert_eq!(run(&mut state, 112), ("rank:absent:ai email:links:106".into(), "P1".into()));
        assert_eq!(state.link_gap["ai email"], 106);
        state.handled.push("rank:absent:ai email:links:106".into());
        assert_eq!(run(&mut state, 113), ("rank:absent:ai email:links".into(), "P2".into()));
        assert_eq!(state.link_gap["ai email"], 113);
        assert_eq!(run(&mut state, 90), ("rank:absent:ai email:links".into(), "P2".into()));
        assert_eq!(state.link_gap["ai email"], 90);
    }

    #[test]
    fn a_locale_page_without_its_own_home_is_linked_through_the_unprefixed_page() {
        let targets = [TargetKeyword { keyword: "復縁".into(), page: "/ja/win-him-back".into() }];
        let mut state = state(&["https://example.com/ja/win-him-back"]);
        let pages = |url: &str| match url {
            "https://example.com/ja/win-him-back" => Some("<title>復縁したい人へ</title><main>復縁のサイン</main>".to_string()),
            "https://example.com/" => Some("<a href=\"/win-him-back\">Win him back</a>".to_string()),
            _ => None,
        };
        let mut findings = [finding("rank:absent:復縁")];
        apply(&mut findings, &targets, SITE, &mut state, &LinkGap { ledger: String::new(), min_new_links: 1 }, &pages, &|_| 0);
        assert_eq!((findings[0].key.as_str(), findings[0].severity.as_str()), ("rank:absent:復縁:links", "P2"));
    }

    #[test]
    fn the_ledger_counts_live_listings_that_link_the_page() {
        let dir = std::env::temp_dir().join(format!("mars-link-gap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rows = [
            ("a.json", r#"{"status":"live","productUrl":"https://example.com","link":{"href":"https://example.com/ai-email-reply/","rel":null}}"#),
            ("b.json", r#"{"status":"live","productUrl":"https://example.com/ai-email-reply"}"#),
            ("c.json", r#"{"status":"pending","productUrl":"https://example.com/ai-email-reply"}"#),
            ("d.json", r#"{"status":"live","productUrl":"https://example.com/ai-email-reply","link":{"href":"https://example.com"}}"#),
        ];
        for (name, body) in rows { std::fs::write(dir.join(name), body).unwrap(); }
        let count = ledger_links(&dir.to_string_lossy(), "https://example.com/ai-email-reply");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(count, 2, "a pending listing and a listing whose rendered link goes elsewhere do not count");
        assert_eq!(ledger_links("/nonexistent-ledger", "https://example.com/ai-email-reply"), 0);
    }

    #[test]
    fn coverage_reads_the_title_and_main_text_by_word_stem() {
        let text = page_text(PAGE);
        assert!(covered("ai email reply", &text) && covered("help me reply to this message", &format!("{text} this message")));
        assert!(!covered("ai email assistant", &text), "the header is outside <main>");
        assert!(covered("email reply drafting", &text) && !covered("ai mail", &text));
        assert!(covered("復縁 line", "復縁したいときの line") && !covered("復縁", "恋愛相談"));
        assert_eq!(homes(SITE, "/ja/win-him-back"), [("https://example.com/ja".to_string(), "/ja/win-him-back".to_string()), ("https://example.com/".to_string(), "/win-him-back".to_string())]);
        assert_eq!(homes(SITE, "/ai-email-reply"), [("https://example.com/".to_string(), "/ai-email-reply".to_string())]);
    }
}
