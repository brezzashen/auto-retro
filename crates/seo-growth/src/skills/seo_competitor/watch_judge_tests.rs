use super::*;

fn cfg() -> DiscoveryConfig {
    serde_json::from_value(json!({
        "hourSgt": 8, "locationCode": 2840, "languageCode": "en", "domain": "example.com", "registry": "r.json",
        "seeds": [{"keyword": "ai relationship coach", "page": "/ai-relationship-coach", "fit": ["relationship", "advice", "coach"]},
                  {"keyword": "text message analyzer", "page": "/text-message-analyzer", "fit": ["text", "message"]}],
        "extraBrands": ["read ai"], "blocked": ["girlfriend"],
        "minVolume": 50, "maxKd": 30, "dropKd": 40, "minWinRate": 0.45, "serpPerDay": 15, "maxPerDay": 3, "maxNewPagesPerDay": 3
    })).unwrap()
}

fn kw(keyword: &str, volume: f64, kd: f64, yearly: f64, page: &str) -> Candidate {
    Candidate { keyword: keyword.into(), page: page.into(), volume, kd: Some(kd), yearly, intent: "commercial".into(), ..Default::default() }
}

#[test]
fn the_pilot_keywords_get_the_pilot_verdicts() {
    let brands = vec!["dex".to_string(), "read ai".to_string()];
    let coach = "/ai-relationship-coach";
    let v = |c: Candidate| prejudge(&c, &cfg(), &brands).verdict;
    assert_eq!(v(kw("best ai for relationship advice", 390.0, 13.0, 182.0, coach)), DO);
    assert_eq!(v(kw("ai dating app", 3600.0, 22.0, 24.0, coach)), DROP, "no relationship stem: off-topic");
    assert_eq!(v(kw("ai girlfriend simulator", 8100.0, 34.0, 662.0, coach)), DROP);
    assert_eq!(v(kw("read ai free", 140.0, 22.0, -33.0, "")), DROP);
    assert_eq!(v(kw("dex crm", 590.0, 2.0, 50.0, "")), DROP);
    assert_eq!(v(kw("analyze text messages ai", 40.0, 9.0, 250.0, "")), WATCH, "40 searches is below the minimum");
}

#[test]
fn a_competitor_keyword_maps_to_the_page_sharing_most_stems() {
    let c = kw("analyze text message conversation", 30.0, 6.0, 200.0, "");
    assert_eq!(fit_page(&c, &cfg().seeds).map(|s| s.page.as_str()), Some("/text-message-analyzer"));
}

#[test]
fn phrase_stems_route_high_value_man_words_and_nothing_else() {
    let mut cfg = cfg();
    cfg.seeds.push(serde_json::from_value(json!({"keyword": "high value man", "page": "/high-value-man",
        "fit": ["high value man", "high value men", "good man", "quality man", "man is serious", "serious about you"]})).unwrap());
    let page = |k: &str| fit_page(&kw(k, 100.0, 5.0, 0.0, ""), &cfg.seeds).map(|s| s.page.clone());
    assert_eq!(page("signs of a high value man").as_deref(), Some("/high-value-man"));
    assert_eq!(page("qualities of a good man").as_deref(), Some("/high-value-man"));
    assert_eq!(page("customer lifetime value"), None);
    assert_eq!(page("good crm for small business"), None);
    assert_eq!(page("text message from a high value man").as_deref(), Some("/high-value-man"), "a 3-word phrase outweighs 2 single stems");
}

#[test]
fn the_devil_vetoes_a_low_win_rate_and_names_giants() {
    let mut j = Judged { candidate: kw("x y", 390.0, 13.0, 10.0, "/p"), verdict: DO.into(), win_rate: Some(0.3), ..Default::default() };
    devil(&mut j, 0.45);
    assert_eq!(j.verdict, WATCH);
    let mut big = Judged { candidate: kw("x y", 390.0, 13.0, 10.0, "/p"), verdict: DO.into(), win_rate: Some(0.6),
        top: vec![("forbes.com".into(), 100.0), ("hubspot.com".into(), 95.0), ("a.ai".into(), 30.0)], ..Default::default() };
    devil(&mut big, 0.45);
    assert_eq!((big.verdict.as_str(), big.objection.contains("大站")), (WATCH, true));
}

#[test]
fn forum_posts_are_not_giants_and_mark_an_unserved_query() {
    // "red flags in a guy", 2026-11-05: YouTube and Quora posts were read as google.com at rank 100.
    let top = vec![("youtube.com".into(), 100.0), ("quora.com".into(), 95.0), ("openup.com".into(), 45.0), ("scienceofpeople.com".into(), 50.0)];
    let mut j = Judged { candidate: kw("red flags in a guy", 8100.0, 2.0, 0.0, "/p"), verdict: DO.into(), win_rate: Some(0.82), top, ..Default::default() };
    devil(&mut j, 0.45);
    assert_eq!(j.verdict, DO, "{}", j.objection);
    assert!(j.objection.contains("2 个论坛帖"), "{}", j.objection);
}

#[test]
fn a_surging_keyword_counts_its_latest_month() {
    let page = "/text-message-analyzer";
    let flat = Candidate { latest: 140.0, base: 27.0, ..kw("long text replies", 40.0, 0.0, 0.0, page) };
    let judged = prejudge(&flat, &cfg(), &[]);
    assert_eq!(judged.verdict, DO, "40 a month on average, 140 last month (×5.2): {}", judged.reason);
    assert!(judged.reason.contains("×5.2"));
    let still = Candidate { latest: 60.0, base: 50.0, ..kw("text me back", 55.0, 0.0, 0.0, page) };
    assert_eq!(prejudge(&still, &cfg(), &[]).verdict, WATCH, "no trend and no surge");
    assert!(score(&flat) > score(&kw("text helper", 120.0, 0.0, 0.0, page)));
}

#[test]
fn one_word_brands_match_whole_words_only() {
    let brands = vec!["dex".to_string(), "clay".to_string()];
    assert!(is_brand("dex personal crm", &brands));
    assert!(!is_brand("index of contacts", &brands));
}

#[test]
fn an_evergreen_seed_does_steady_keywords_without_a_rising_trend() {
    let mut c = cfg();
    c.seeds.push(serde_json::from_value(json!({"keyword": "how to get your ex back", "page": "/win-him-back",
        "fit": ["him", "chase", "miss"], "evergreen": {"minVolume": 200, "maxKd": 20}})).unwrap());
    let page = "/win-him-back";
    let v = |k: Candidate| prejudge(&k, &c, &[]);
    let done = v(kw("how to make him chase you", 1600.0, 2.0, -16.0, page));
    assert_eq!(done.verdict, DO, "{}", done.reason);
    assert!(done.reason.starts_with("常青需求"));
    assert_eq!(v(kw("how to make him miss you badly", 150.0, 0.0, -10.0, page)).verdict, WATCH, "below the evergreen volume");
    assert_eq!(v(kw("why does he miss me", 900.0, 25.0, -10.0, page)).verdict, WATCH, "KD above the evergreen limit");
    assert_eq!(v(kw("ai relationship advice app", 900.0, 10.0, -5.0, "/ai-relationship-coach")).verdict, WATCH, "other seeds keep the trend rule");
}

fn checked(top: &[(&str, f64)]) -> Judged {
    Judged { candidate: kw("decode his text", 900.0, 12.0, 20.0, "/text-message-analyzer"), verdict: DO.into(), win_rate: Some(0.7),
        top: top.iter().map(|(d, r)| (d.to_string(), *r)).collect(), ..Default::default() }
}

#[test]
fn typo_and_direct_answer_words_are_vetoed() {
    let open = [("a.app", 20.0), ("b.io", 15.0), ("c.ai", 25.0), ("d.com", 30.0)];
    let mut typo = Judged { respelled: "decode this text".into(), ..checked(&open) };
    devil(&mut typo, 0.45);
    assert_eq!((typo.verdict.as_str(), typo.objection.contains("typo")), (WATCH, true));
    let mut answered = Judged { direct: "answer_box".into(), ..checked(&open) };
    devil(&mut answered, 0.45);
    assert_eq!((answered.verdict.as_str(), answered.objection.contains("直接给答案")), (WATCH, true));
}

#[test]
fn homepages_filling_the_first_page_veto_and_inner_top3_is_noted() {
    let open = [("a.app", 20.0), ("b.io", 15.0), ("c.ai", 25.0), ("d.com", 30.0)];
    let mut homes = Judged { homepages: 2, ..checked(&open) };
    devil(&mut homes, 0.45);
    assert_eq!((homes.verdict.as_str(), homes.objection.contains("2 个首页")), (WATCH, true));
    let mut inner = Judged { inner_top3: true, ..checked(&open) };
    devil(&mut inner, 0.45);
    assert_eq!((inner.verdict.as_str(), inner.objection.contains("前 3 全是内页")), (DO, true));
}

#[test]
fn huge_volume_with_near_zero_kd_needs_inner_pages_on_top() {
    let open = [("a.app", 20.0), ("b.io", 15.0), ("c.ai", 25.0), ("d.com", 30.0)];
    let odd = |inner_top3: bool| { let mut j = Judged { inner_top3, candidate: kw("free text", 40_000.0, 2.0, 0.0, "/p"), ..checked(&open) }; devil(&mut j, 0.45); j };
    assert_eq!(odd(false).verdict, WATCH, "{}", odd(false).objection);
    assert_eq!(odd(true).verdict, DO, "red flags in a guy (8,100, KD 2) style: inner pages on top make it real");
}

#[test]
fn a_seasonal_word_is_watched_and_a_new_word_needs_no_kd() {
    let page = "/text-message-analyzer";
    let valentine = Candidate { seasonal: 2, ..kw("valentine text message ideas", 2900.0, 8.0, 40.0, page) };
    let judged = prejudge(&valentine, &cfg(), &[]);
    assert_eq!(judged.verdict, WATCH);
    assert!(judged.reason.contains("每年 2 月高峰"));
    let newborn = Candidate { kd: None, age_days: Some(9), trends_volume: 600.0, ..kw("vibe texting", 0.0, 0.0, 0.0, page) };
    assert_eq!(prejudge(&newborn, &cfg(), &[]).verdict, DO, "a 9-day-old word has no KD yet; the SERP decides");
    let old = Candidate { kd: None, ..kw("text message ideas", 900.0, 0.0, 30.0, page) };
    assert_eq!(prejudge(&old, &cfg(), &[]).verdict, WATCH, "an old word without KD still waits");
}

#[test]
fn a_product_model_number_is_a_brand_word() {
    assert!(is_model("sony icd-ux570 digital voice recorder"), "dry run 2026-11-10 judged it 做");
    assert!(!is_model("2027 calendar") && !is_model("mp3 recorder") && !is_model("text message analyzer"));
    let model = kw("sony icd-ux570 digital voice recorder", 1900.0, 10.0, 20.0, "/text-message-analyzer");
    assert_eq!(prejudge(&model, &cfg(), &[]).verdict, DROP);
}

#[test]
fn the_live_seeds_route_the_one_and_husband_words_to_high_value_man_and_counselling_elsewhere() {
    let live = super::super::watch_config::read_config(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/seo_competitor/watch.config.json")).unwrap();
    let seeds = &live.discovery.as_ref().unwrap().seeds;
    let page = |k: &str| fit_page(&kw(k, 1000.0, 0.0, 0.0, ""), seeds).map(|s| s.page.clone()).unwrap_or_default();
    for word in ["how to know if he's the one", "is he the one", "husband material", "good qualities of a husband", "what to look for in a man", "qualities of a good man"] {
        assert_eq!(page(word), "/high-value-man", "{word}");
    }
    assert_eq!(page("marriage counselling singapore"), "/relationship-counselling-singapore");
    assert_ne!(page("one piece"), "/high-value-man");
    assert_ne!(page("customer lifetime value"), "/high-value-man");
}

#[test]
fn ai_boyfriend_companions_stay_blocked_while_judging_a_real_boyfriend_is_allowed() {
    let live = super::super::watch_config::read_config(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/seo_competitor/watch.config.json")).unwrap();
    let cfg = live.discovery.as_ref().unwrap();
    let v = |k: &str| prejudge(&kw(k, 1000.0, 5.0, 0.0, ""), cfg, &[]).reason;
    for blocked in ["ai boyfriend app", "virtual boyfriend", "boyfriend simulator", "ai girlfriend", "best girlfriend gifts", "ai彼氏 アプリ"] {
        assert_eq!(v(blocked), "品牌安全", "{blocked}");
    }
    for allowed in ["good boyfriend qualities", "how to know if your boyfriend is the one", "良い彼氏の特徴"] {
        assert_ne!(v(allowed), "品牌安全", "{allowed}");
    }
    let page = |k: &str| fit_page(&kw(k, 1000.0, 0.0, 0.0, ""), &cfg.seeds).map(|s| s.page.clone()).unwrap_or_default();
    assert_eq!(page("good boyfriend qualities"), "/high-value-man");
}

#[test]
fn a_navigational_word_survives_only_on_a_core_market_seed() {
    // 2026-11-14: "premarital counseling near me" (US 2,400, KD 0) was dropped as navigational although
    // counselling near the searcher is a core market (founder 2026-11-11).
    let mut cfg = cfg();
    cfg.seeds.push(serde_json::from_value(json!({"keyword": "relationship counseling", "page": "/relationship-counselling",
        "fit": ["marriage counseling", "premarital counseling"], "evergreen": {"minVolume": 200, "maxKd": 20}, "coreMarket": true})).unwrap());
    let near = |keyword: &str, page: &str| Candidate { intent: "navigational".into(), ..kw(keyword, 2400.0, 0.0, 0.0, page) };
    let counselling = prejudge(&near("premarital counseling near me", "/relationship-counselling"), &cfg, &[]);
    assert_eq!((counselling.verdict.as_str(), counselling.candidate.page.as_str()), (DO, "/relationship-counselling"));
    let coach = prejudge(&near("relationship coach near me", "/ai-relationship-coach"), &cfg, &[]);
    assert_eq!((coach.verdict.as_str(), coach.reason.as_str()), (DROP, "导航意图"));
    assert_eq!(prejudge(&near("betterhelp premarital counseling login", ""), &cfg, &["betterhelp".to_string()]).reason, "品牌词", "a brand is still dropped first");
}
