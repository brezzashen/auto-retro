//! Page-head fingerprints for IndexNow: a title, description or canonical edit leaves the sitemap
//! `lastmod` unchanged (verified 2026-11-05: Yahoo and Bing kept the old `| Example AI` titles), so each
//! round fetches every sitemap URL and re-sends an accepted URL whose head changed since its last push.

use super::push_config::EngineState;
use super::watch_probe::{TITLE, futures_join};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::LazyLock;

static DESCRIPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<meta name="description" content="([^"]*)""#).expect("regex"));
static CANONICAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<link rel="canonical" href="([^"]*)""#).expect("regex"));

/// Short hash of the title, meta description and canonical; `None` when the page has no title.
pub fn head_signature(html: &str) -> Option<String> {
    let field = |re: &Regex| re.captures(html).map(|c| c[1].to_string()).unwrap_or_default();
    let title = field(&TITLE);
    if title.trim().is_empty() { return None; }
    let digest = Sha256::digest(format!("{title}\n{}\n{}", field(&DESCRIPTION), field(&CANONICAL)));
    Some(digest.iter().take(8).map(|b| format!("{b:02x}")).collect())
}

/// Head signature of every URL that answered 200 with a title, fetched six at a time.
pub async fn page_heads(client: &reqwest::Client, urls: &[String]) -> BTreeMap<String, String> {
    let mut heads = BTreeMap::new();
    for chunk in urls.chunks(6) {
        for (url, (status, _, html)) in chunk.iter().zip(futures_join(client, chunk).await) {
            if status != 200 { continue; }
            if let Some(signature) = head_signature(&html) { heads.insert(url.clone(), signature); }
        }
    }
    heads
}

/// Accepted URLs whose head differs from the one recorded at their last push, in sitemap order.
/// A URL without a recorded head only gets its baseline recorded.
pub fn head_changed(urls: &[String], heads: &BTreeMap<String, String>, state: &EngineState) -> Vec<String> {
    urls.iter().filter(|url| {
        state.pushed.contains_key(*url) && matches!((heads.get(*url), state.pushed_head.get(*url)), (Some(now), Some(seen)) if now != seen)
    }).cloned().collect()
}

/// Records the head of every accepted URL, so the next round re-sends only a later edit.
pub fn remember_heads(heads: &BTreeMap<String, String>, state: &mut EngineState) {
    for (url, signature) in heads {
        if state.pushed.contains_key(url) { state.pushed_head.insert(url.clone(), signature.clone()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_follows_title_description_and_canonical() {
        let page = |title: &str, desc: &str| format!(r#"<title>{title}</title><meta name="description" content="{desc}"/><link rel="canonical" href="https://example.com/pricing"/>"#);
        let base = head_signature(&page("Pricing | Example AI", "Plans")).unwrap();
        assert_eq!(head_signature(&page("Pricing | Example AI", "Plans")).unwrap(), base);
        assert_ne!(head_signature(&page("Pricing | ExampleAI", "Plans")).unwrap(), base);
        assert_ne!(head_signature(&page("Pricing | Example AI", "All plans")).unwrap(), base);
        assert_eq!(head_signature("<html></html>"), None);
    }

    #[test]
    fn only_accepted_urls_with_a_recorded_different_head_are_resent() {
        let urls: Vec<String> = ["edited", "same", "baseline", "never-pushed"].map(String::from).into();
        let heads: BTreeMap<String, String> = [("edited", "b"), ("same", "a"), ("baseline", "a"), ("never-pushed", "a")].map(|(u, h)| (u.to_string(), h.to_string())).into_iter().collect();
        let mut state = EngineState::default();
        for url in ["edited", "same", "baseline"] { state.pushed.insert(url.into(), "t".into()); }
        state.pushed_head.insert("edited".into(), "a".into());
        state.pushed_head.insert("same".into(), "a".into());
        assert_eq!(head_changed(&urls, &heads, &state), ["edited"]);
        remember_heads(&heads, &mut state);
        assert_eq!(state.pushed_head.get("baseline").map(String::as_str), Some("a"));
        assert!(!state.pushed_head.contains_key("never-pushed"));
    }
}
