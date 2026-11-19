use super::*;

fn hosts() -> Vec<String> {
    BaiduConfig::default().hosts
}

fn answer() -> Value {
    json!({"organic_results": [
        {"position": 1, "link": "https://lianai520.cn/"},
        {"position": 2, "link": "https://apps.apple.com/cn/app/id6753884796"},
        {"position": 3, "link": "https://www.love.xj.cn/"},
        {"position": 7, "link": "https://www.example.cn/zh-CN/ai-relationship-coach"},
        {"position": 9, "link": "https://example.com/ai-relationship-coach"}]})
}

fn at(text: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(text).unwrap()
}

#[test]
fn our_rank_is_the_first_result_on_either_host_and_leaders_are_the_top_three_hosts() {
    let (ours, leaders) = read_serp(&answer(), &hosts());
    assert_eq!(ours, Some((7, "https://www.example.cn/zh-CN/ai-relationship-coach".into())));
    assert_eq!(leaders, vec!["lianai520.cn", "apps.apple.com", "love.xj.cn"]);
    let (none, _) = read_serp(&json!({"organic_results": [{"position": 1, "link": "https://lianai520.cn/"}]}), &hosts());
    assert_eq!(none, None, "a page without our hosts leaves the word unranked");
}

#[test]
fn a_run_is_due_on_the_first_run_and_after_every_days() {
    let now = at("2026-11-22T11:17:00+08:00");
    assert!(due(None, now, 7));
    assert!(due(Some("2026-11-15T11:17:00+08:00"), now, 7));
    assert!(!due(Some("2026-11-15T12:17:00+08:00"), now, 7));
    assert!(due(Some("not a time"), now, 7), "an unreadable stamp reruns rather than stalls");
}

#[test]
fn the_next_rank_carries_the_earlier_measurement_and_the_first_run_has_none() {
    let first = next_rank(None, "/zh-CN/ai-relationship-coach", &json!({"organic_results": []}), &hosts(), "2026-11-15T11:17:00+08:00");
    assert_eq!((first.position, first.previous, first.previous_at.as_deref()), (None, None, None));
    let second = next_rank(Some(&first), "/zh-CN/ai-relationship-coach", &answer(), &hosts(), "2026-11-22T11:17:00+08:00");
    assert_eq!(second.position, Some(7));
    assert_eq!((second.previous, second.previous_at.as_deref()), (None, Some("2026-11-15T11:17:00+08:00")));
}

#[test]
fn the_section_shows_a_dash_before_the_first_comparison_and_outside_the_depth_after_it() {
    let first = next_rank(None, "/zh-CN/ai-relationship-coach", &json!({"organic_results": []}), &hosts(), "2026-11-15T11:17:00+08:00");
    let second = next_rank(Some(&first), "/zh-CN/ai-relationship-coach", &answer(), &hosts(), "2026-11-22T11:17:00+08:00");
    let view = BaiduView { depth: 50, checked_now: false, next_run: Some("2026-11-29 11:17".into()),
        ranks: BTreeMap::from([("恋爱教练".to_string(), first), ("AI恋爱教练".to_string(), second)]) };
    let md = section(Some(&view));
    assert!(md.contains("下次检查 2026-11-29 11:17"));
    assert!(md.contains("| AI恋爱教练 | /zh-CN/ai-relationship-coach | 7 | 前 50 名外 | lianai520.cn、apps.apple.com、love.xj.cn | 2026-11-22 11:17 |"));
    assert!(md.contains("| 恋爱教练 | /zh-CN/ai-relationship-coach | 前 50 名外 | — |"));
    assert!(md.find("AI恋爱教练").unwrap() < md.find("| 恋爱教练").unwrap(), "ranked words sort first");
    assert_eq!(section(None), "");
}

#[test]
fn no_words_turns_the_watch_off() {
    let mut errors = Vec::new();
    assert!(collect(&BaiduConfig::default(), "/nonexistent", at("2026-11-15T11:17:00+08:00"), true, &mut errors).is_none());
    assert!(errors.is_empty());
}

#[test]
fn chinese_words_keep_their_own_serp_files() {
    let words = ["AI恋爱教练", "AI恋爱军师", "AI恋爱助手", "AI情感咨询", "恋爱教练", "恋爱军师", "聊天记录分析", "ExampleAI"];
    let keys: std::collections::BTreeSet<String> = words.iter().map(|w| super::super::watch_trends_market::file_key(w)).collect();
    assert_eq!(keys.len(), words.len(), "{keys:?}");
}
