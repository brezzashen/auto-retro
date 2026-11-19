use super::*;

#[test]
fn weekly_query_rows_sum_per_query_with_an_impression_weighted_position() {
    let stats: Value = serde_json::from_str(r#"{"d":[
        {"Query":"example.com","Impressions":8,"Clicks":10,"AvgImpressionPosition":2,"AvgClickPosition":-1},
        {"Query":"example.com","Impressions":2,"Clicks":0,"AvgImpressionPosition":7,"AvgClickPosition":-1},
        {"Query":"relationship agent","Impressions":4,"Clicks":0,"AvgImpressionPosition":-1},
        {"Query":"site:example.com","Impressions":21,"Clicks":0,"AvgImpressionPosition":4}]}"#).unwrap();
    let rows = query_rows(&stats, 2);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], QueryRow { query: "site:example.com".into(), impressions: 21, clicks: 0, position: 4.0 });
    assert_eq!(rows[1], QueryRow { query: "example.com".into(), impressions: 10, clicks: 10, position: 3.0 });
    assert_eq!(query_rows(&stats, 9)[2].position, 0.0, "a query with no position stays 0");
}

#[test]
fn the_serp_position_is_the_first_result_on_our_host_or_www() {
    let answer = json!({"organic_results": [
        {"position": 1, "link": "https://crm.org/crmland/personal-crm"},
        {"position": 2, "link": "https://www.example.com/personal-crm"},
        {"position": 3, "link": "https://example.com/pricing"}]});
    assert_eq!(serp_position(&answer, "example.com"), Some((2, "https://www.example.com/personal-crm".into())));
    assert_eq!(serp_position(&json!({"organic_results": [{"position": 1, "link": "https://exampleapp.co/"}]}), "example.com"), None);
    assert_eq!(serp_position(&json!({}), "example.com"), None);
}

#[test]
fn rotation_wraps_and_markets_follow_the_locale_prefix() {
    let items = vec![1, 2, 3, 4];
    assert_eq!(rotate(&items, 3, 3), (vec![4, 1, 2], 2));
    assert_eq!(rotate(&items, 0, 9), (vec![1, 2, 3, 4], 0));
    assert_eq!(rotate::<u8>(&[], 5, 3), (vec![], 0));
    assert_eq!((market_of("/ja/personal-ai-assistant"), market_of("/zh-TW/text-message-analyzer"), market_of("/personal-crm")), ("jp", "tw", "us"));
    assert_eq!((market_of("/ko/ai-relationship-coach"), market_of("/th/personal-ai-assistant"), market_of("/kotlin")), ("kr", "th", "us"));
}

#[test]
fn year_one_means_never_crawled() {
    assert_eq!(day_of(-62135596800000), None);
    assert_eq!(day_of(1794220727000).as_deref(), Some("2026-11-09"));
    assert_eq!(urlencoding("https://example.com/"), "https%3A%2F%2Fexample.com%2F");
}

#[test]
fn never_crawled_pages_raise_one_p2_and_the_section_lists_ranks_best_first() {
    let mut view = BingView { in_index: Some(8), feed: Some((48, "2026-11-03".into())), checked_words: vec!["personal crm".into()], ..Default::default() };
    view.crawled.insert("https://example.com/".into(), Some("2026-11-09".into()));
    view.crawled.insert("https://example.com/personal-crm".into(), None);
    view.ranks.insert("rolodex app".into(), WordRank { page: "/personal-crm".into(), position: None, url: None, at: "2026-11-09T23:17:00+08:00".into() });
    view.ranks.insert("exampleapp".into(), WordRank { page: "/".into(), position: Some(1), url: Some("https://example.com/".into()), at: "2026-11-09T23:17:00+08:00".into() });
    let found = findings(&view);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].severity.as_str(), found[0].key.as_str()), ("P2", "bing:uncrawled:1"));
    let md = section(Some(&view));
    assert!(md.starts_with("## Bing 排名与收录"));
    assert!(md.contains("Bing 收录 8 页") && md.contains("48 个地址（2026-11-03 读取）") && md.contains("从未抓取 1 个"));
    assert!(md.find("| exampleapp | / | 1 |").unwrap() < md.find("| rolodex app | /personal-crm | 首页外 | 2026-11-09 23:17 |").unwrap());
    assert_eq!(section(None), "");
    let quiet = BingView { queries: vec![QueryRow { query: "exampleapp ai | example.com".into(), impressions: 3, clicks: 0, position: 2.7 }], ..Default::default() };
    let md = section(Some(&quiet));
    assert!(md.contains("还没有查过目标词的 Bing 名次") && !md.contains("| 目标词 |"));
    assert!(md.contains("| exampleapp ai \\| example.com | 3 | 0 | 2.7 |"), "a pipe in a query must not split the row");
    view.crawled.clear();
    assert!(findings(&view).is_empty());
}
