//! Site health of the public pages: every sitemap URL is fetched and its status, `<html lang>`,
//! title, canonical, `hreflang` links and `noindex` are compared with what the URL promises.

use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;
use std::time::Instant;

static LOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<loc>([^<]+)</loc>").expect("regex"));
static URL_ENTRY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<url>(.*?)</url>").expect("regex"));
static LASTMOD_DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<lastmod>\s*(\d{4}-\d{2}-\d{2})\s*</lastmod>").expect("regex"));
static LANG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<html lang="([^"]*)""#).expect("regex"));
pub(super) static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<title>([^<]*)</title>").expect("regex"));
static CANONICAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<link rel="canonical" href="([^"]*)""#).expect("regex"));
static HREFLANG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<link rel="alternate" hreflang="[^"]+""#).expect("regex"));
static NOINDEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<meta name="robots" content="[^"]*noindex"#).expect("regex"));

/// Locales a sitemap URL can carry, as `(path prefix, html lang)`; English has no prefix.
const PREFIXES: [(&str, &str); 8] =
    [("/zh-CN", "zh-CN"), ("/zh-TW", "zh-TW"), ("/ja", "ja"), ("/ko", "ko"), ("/th", "th"), ("/fr", "fr"), ("/it", "it"), ("/es", "es")];

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PageProbe {
    pub url: String,
    pub status: u16,
    pub millis: u128,
    pub lang: String,
    pub expected_lang: String,
    pub title: String,
    pub canonical: String,
    pub hreflang_links: usize,
    pub noindex: bool,
    /// Every way the page differs from its promise; empty when healthy.
    pub problems: Vec<String>,
}

pub fn sitemap_urls(xml: &str) -> Vec<String> {
    LOC.captures_iter(xml).map(|c| c[1].trim().to_string()).collect()
}

/// `loc → lastmod` of every sitemap entry whose `lastmod` is a plain date (YYYY-MM-DD), the date of its
/// content. A full timestamp is the build time of a page without a dated source and changes on every
/// deploy, so it is left out.
pub fn sitemap_content_dates(xml: &str) -> std::collections::BTreeMap<String, String> {
    URL_ENTRY.captures_iter(xml).filter_map(|entry| {
        let loc = LOC.captures(&entry[1])?[1].trim().to_string();
        let date = LASTMOD_DATE.captures(&entry[1])?[1].to_string();
        Some((loc, date))
    }).collect()
}

/// The `<html lang>` a URL must declare: `zh-CN`, `zh-TW`, `ja`, `ko` or `th` under their prefix, else `en`.
pub fn expected_lang(url: &str, site: &str) -> String {
    let path = url.strip_prefix(site).unwrap_or(url);
    PREFIXES.iter()
        .find(|(prefix, _)| path == *prefix || path.starts_with(&format!("{prefix}/")))
        .map_or("en", |(_, lang)| lang)
        .to_string()
}

fn first(re: &Regex, html: &str) -> String {
    re.captures(html).map(|c| c[1].to_string()).unwrap_or_default()
}

/// Judges one fetched page against its URL.
pub fn judge(url: &str, site: &str, status: u16, millis: u128, html: &str) -> PageProbe {
    let expected = expected_lang(url, site);
    let mut page = PageProbe {
        url: url.to_string(), status, millis,
        lang: first(&LANG, html), expected_lang: expected,
        title: first(&TITLE, html), canonical: first(&CANONICAL, html),
        hreflang_links: HREFLANG.find_iter(html).count(), noindex: NOINDEX.is_match(html),
        problems: Vec::new(),
    };
    if status != 200 { page.problems.push(format!("HTTP {status}")); }
    if page.lang != page.expected_lang { page.problems.push(format!("lang {} ≠ {}", page.lang, page.expected_lang)); }
    if page.title.trim().is_empty() { page.problems.push("empty title".into()); }
    if page.canonical != url { page.problems.push(format!("canonical {}", page.canonical)); }
    if page.hreflang_links < 4 { page.problems.push(format!("{} hreflang links (< 4)", page.hreflang_links)); }
    if page.noindex { page.problems.push("noindex".into()); }
    if millis > 3000 { page.problems.push(format!("slow {millis} ms")); }
    page
}

/// Status `0` means no complete response: the request failed, or the body could not be read in time.
async fn fetch(client: &reqwest::Client, url: &str) -> (u16, u128, String) {
    let started = Instant::now();
    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.text().await {
                Ok(body) => (status, started.elapsed().as_millis(), body),
                Err(_) => (0, started.elapsed().as_millis(), String::new()),
            }
        }
        Err(_) => (0, started.elapsed().as_millis(), String::new()),
    }
}

/// The production probes, and for each failing URL the same path on `mirror` (Canary), judged against
/// the production URL: a mirror page without problems means the fix waits for promotion. `released`
/// holds the URLs the production sitemap lists; the rest come only from the Canary sitemap.
pub struct SiteProbe {
    pub urls: Vec<String>,
    pub released: Vec<String>,
    pub probes: Vec<PageProbe>,
    pub mirror: Vec<PageProbe>,
}

/// Fetches the sitemaps of `site` and `mirror` (their URLs are production URLs) and every URL in them.
pub fn probe_site(site: &str, mirror: Option<&str>) -> Result<SiteProbe, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; MarsSeoWatch/1.0; +https://example.com)")
            .timeout(std::time::Duration::from_secs(20))
            .build().map_err(|e| e.to_string())?;
        let (status, _, xml) = fetch(&client, &format!("{site}/sitemap.xml")).await;
        if status != 200 { return Err(format!("sitemap.xml HTTP {status}")); }
        let mut urls = sitemap_urls(&xml);
        let released = urls.clone();
        if let Some(mirror) = mirror {
            let (_, _, mirror_xml) = fetch(&client, &format!("{mirror}/sitemap.xml")).await;
            for url in sitemap_urls(&mirror_xml) { if !urls.contains(&url) { urls.push(url); } }
        }
        let probes = probe_all(&client, &urls, site, |url| url.to_string()).await;
        let failing: Vec<String> = probes.iter().filter(|p| !p.problems.is_empty()).map(|p| p.url.clone()).collect();
        let mirror = match mirror {
            Some(origin) => probe_all(&client, &failing, site, |url| url.replacen(site, origin, 1)).await,
            None => Vec::new(),
        };
        Ok(SiteProbe { urls, released, probes, mirror })
    })
}

/// Fetches `fetch_url(url)` for each production `url`, six at a time, and judges it as `url`.
/// A page whose only problem is slowness is fetched alone up to [`SOLO_FETCHES`] more times and
/// judged by the first fetch that is not slow: concurrent batch fetches stall together on a saturated
/// host, and the probe host's proxy path adds random multi-second stalls that the origin never sees.
/// Only a page that is slow on every solo fetch is slow. A page without a complete response (status 0)
/// is refetched the same way after a short pause, because the probe's proxy path drops requests that
/// never reach the origin and stalls bodies the origin sent in milliseconds. When more than
/// [`MAX_TRANSPORT_REFETCHES`] pages fail that way the site itself is down, and they keep their verdict.
async fn probe_all(client: &reqwest::Client, urls: &[String], site: &str, fetch_url: impl Fn(&str) -> String) -> Vec<PageProbe> {
    let mut probes = Vec::new();
    for chunk in urls.chunks(6) {
        let targets: Vec<String> = chunk.iter().map(|url| fetch_url(url)).collect();
        let fetched = futures_join(client, &targets).await;
        for (url, (status, millis, html)) in chunk.iter().zip(fetched) { probes.push(judge(url, site, status, millis, &html)); }
    }
    for index in solo_refetch_targets(&probes) {
        let probe = &mut probes[index];
        for _ in 0..SOLO_FETCHES {
            if probe.status == 0 { tokio::time::sleep(TRANSPORT_PAUSE).await; }
            let (status, millis, html) = fetch(client, &fetch_url(&probe.url)).await;
            *probe = judge(&probe.url, site, status, millis, &html);
            if !needs_solo_refetch(probe) { break; }
        }
    }
    probes
}

const SOLO_FETCHES: usize = 3;
const MAX_TRANSPORT_REFETCHES: usize = 20;
const TRANSPORT_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

pub(super) fn only_slow(probe: &PageProbe) -> bool {
    !probe.problems.is_empty() && probe.problems.iter().all(|p| p.starts_with("slow "))
}

fn needs_solo_refetch(probe: &PageProbe) -> bool {
    only_slow(probe) || probe.status == 0
}

/// Indexes of the probes to fetch again alone: every slow-only page, and the pages without a complete
/// response unless there are more of them than [`MAX_TRANSPORT_REFETCHES`].
fn solo_refetch_targets(probes: &[PageProbe]) -> Vec<usize> {
    let transport = probes.iter().filter(|p| p.status == 0).count();
    probes.iter().enumerate()
        .filter(|(_, p)| only_slow(p) || (p.status == 0 && transport <= MAX_TRANSPORT_REFETCHES))
        .map(|(index, _)| index)
        .collect()
}

pub(super) async fn futures_join(client: &reqwest::Client, urls: &[String]) -> Vec<(u16, u128, String)> {
    let mut set = tokio::task::JoinSet::new();
    for (index, url) in urls.iter().enumerate() {
        let client = client.clone();
        let url = url.clone();
        set.spawn(async move { (index, fetch(&client, &url).await) });
    }
    let mut out = vec![(0, 0, String::new()); urls.len()];
    while let Some(Ok((index, result))) = set.join_next().await { out[index] = result; }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "https://example.com";

    #[test]
    fn locale_prefix_sets_the_expected_lang() {
        assert_eq!(expected_lang("https://example.com", SITE), "en");
        assert_eq!(expected_lang("https://example.com/zh-CN", SITE), "zh-CN");
        assert_eq!(expected_lang("https://example.com/zh-TW/personal-crm", SITE), "zh-TW");
        assert_eq!(expected_lang("https://example.com/zh-CNX", SITE), "en");
        assert_eq!(expected_lang("https://example.com/ja/ai-relationship-coach", SITE), "ja");
        assert_eq!(expected_lang("https://example.com/jazz", SITE), "en");
        assert_eq!(expected_lang("https://example.com/ko/ai-relationship-coach", SITE), "ko");
        assert_eq!(expected_lang("https://example.com/ko", SITE), "ko");
        assert_eq!(expected_lang("https://example.com/th/personal-ai-assistant", SITE), "th");
        assert_eq!(expected_lang("https://example.com/kotlin", SITE), "en");
        assert_eq!(expected_lang("https://example.com/thread", SITE), "en");
    }

    #[test]
    fn a_healthy_page_has_no_problems_and_a_stale_one_lists_each() {
        let url = "https://example.com/zh-CN/pricing";
        let links = r#"<link rel="alternate" hreflang="en" href="a" /><link rel="alternate" hreflang="zh-Hans" href="b" /><link rel="alternate" hreflang="zh-Hant" href="c" /><link rel="alternate" hreflang="x-default" href="d" />"#;
        let good = format!(r#"<html lang="zh-CN"><title>价格</title><link rel="canonical" href="{url}" />{links}"#);
        assert!(judge(url, SITE, 200, 120, &good).problems.is_empty());
        let stale = r#"<html lang="zh-CN"><title>x</title><link rel="canonical" href="https://example.com/pricing" /><meta name="robots" content="noindex" />"#;
        let problems = judge(url, SITE, 200, 5000, stale).problems;
        assert_eq!(problems.len(), 4, "{problems:?}");
    }

    #[test]
    fn only_a_page_that_is_merely_slow_is_refetched() {
        let url = "https://example.com/pricing";
        let links = r#"<link rel="alternate" hreflang="a" /><link rel="alternate" hreflang="b" /><link rel="alternate" hreflang="c" /><link rel="alternate" hreflang="d" />"#;
        let page = format!(r#"<html lang="en"><title>Pricing</title><link rel="canonical" href="{url}" />{links}"#);
        assert!(only_slow(&judge(url, SITE, 200, 32000, &page)));
        assert!(!only_slow(&judge(url, SITE, 200, 120, &page)));
        assert!(!only_slow(&judge(url, SITE, 404, 32000, &page)));
    }

    #[test]
    fn pages_without_a_response_are_refetched_unless_the_whole_site_is_down() {
        let url = "https://example.com/personal-crm";
        let links = r#"<link rel="alternate" hreflang="a" /><link rel="alternate" hreflang="b" /><link rel="alternate" hreflang="c" /><link rel="alternate" hreflang="d" />"#;
        let page = format!(r#"<html lang="en"><title>CRM</title><link rel="canonical" href="{url}" />{links}"#);
        let healthy = judge(url, SITE, 200, 120, &page);
        let missing = judge(url, SITE, 404, 120, &page);
        let dropped = judge(url, SITE, 0, 20002, "");
        assert_eq!(solo_refetch_targets(&[healthy.clone(), missing, dropped.clone()]), [2]);
        let outage: Vec<PageProbe> = (0..=MAX_TRANSPORT_REFETCHES).map(|_| dropped.clone()).collect();
        assert!(solo_refetch_targets(&outage).is_empty());
        assert!(!needs_solo_refetch(&healthy));
    }

    #[test]
    fn sitemap_locs_are_listed_in_order() {
        let xml = "<url><loc>https://example.com</loc></url><url><loc> https://example.com/zh-CN </loc></url>";
        assert_eq!(sitemap_urls(xml), ["https://example.com", "https://example.com/zh-CN"]);
    }

    #[test]
    fn content_dates_keep_plain_dates_and_skip_build_timestamps() {
        let xml = "<url>\n<loc>https://example.com/research/a</loc>\n<lastmod>2026-11-04</lastmod>\n</url>\
            <url><loc>https://example.com/pricing</loc><lastmod>2026-11-04T04:47:07.496Z</lastmod></url>\
            <url><loc>https://example.com/terms</loc></url>";
        let dates = sitemap_content_dates(xml);
        assert_eq!(dates.len(), 1);
        assert_eq!(dates["https://example.com/research/a"], "2026-11-04");
    }
}
