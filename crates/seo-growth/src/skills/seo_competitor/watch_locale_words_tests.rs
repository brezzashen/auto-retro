use super::*;

fn market() -> RankMarket {
    serde_json::from_value(json!({"prefix": "/ko", "locationCode": 2410, "languageCode": "ko"})).unwrap()
}

fn word(w: &str, status: &str) -> Word {
    Word { locale: "ko".into(), page: "/text-message-analyzer".into(), word: w.into(), meaning: "m".into(), status: status.into(), ..Default::default() }
}

const COPY: &str = "export const keywordPagesKo = {\n  'text-message-analyzer': {\n    keywords: ['카톡 분석', '대화 분석 ai'],\n    title: '카톡 한 줄 뒤의 속뜻',\n  },\n  'relationship-red-flags': {\n    keywords: ['레드플래그', '사진 용량 줄이기'],\n  },\n}\n";

#[test]
fn the_local_check_uses_the_target_country_for_both_the_serp_and_one_shared_trends_chart() {
    // Gefei's example (2026-09-14): two Korean phrasings of "compress an image".
    let words = vec!["이미지 압축".to_string(), "사진 용량 줄이기".to_string()];
    let chart = trends_call(&words, 2410, "t.json".into());
    assert_eq!(chart.body, json!({"keywords": ["이미지 압축", "사진 용량 줄이기"], "location_code": 2410, "time_range": "past_12_months", "item_types": ["google_trends_graph"]}),
        "both are search terms in one call, same region and range");
    let serp = serp_call(&words[1], &market(), "s.json".into());
    assert_eq!((serp.body["location_code"].as_u64(), serp.body["language_code"].as_str()), (Some(2410), Some("ko")), "region plus language filter");
    let page = json!([{"items": [{"type": "organic", "title": "사진 용량 줄이기 - 무료 온라인"}, {"type": "organic", "title": "이미지 압축기"}, {"type": "people_also_ask", "title": "사진 용량 줄이기"}]}]);
    assert_eq!((title_hits(&page, "사진 용량 줄이기"), title_hits(&page, "사진용량 줄이기"), title_hits(&page, "동영상 압축")), (1, 1, 0));
}

#[test]
fn a_word_with_a_curve_is_confirmed_and_one_without_a_curve_stays_pending() {
    let e = |word: &str, mean: f64, coverage: f64| Evidence { word: word.into(), mean, coverage, title_hits: 0 };
    let got = judge(&[e("이미지 압축", 21.0, 0.9), e("사진 용량 줄이기", 42.0, 1.0), e("사진 크기 줄이는 법", 0.0, 0.0)]);
    assert_eq!(got, [(CONFIRMED, 0.5, false), (CONFIRMED, 1.0, true), (PENDING, 0.0, false)], "the higher curve is the title word; no curve is never a drop");
    assert_eq!(judge(&[e("x", 0.0, 0.0)]), [(PENDING, 0.0, false)]);
    // Korea, 2026-11-10: "카톡 답장" filled 44 of 52 weeks; "카톡 분석" only 7, a few blips and no curve.
    assert_eq!(judge(&[e("카톡 분석", 3.71, 7.0 / 52.0), e("카톡 답장", 40.27, 44.0 / 52.0)]), [(PENDING, 0.09, false), (CONFIRMED, 1.0, true)]);
    let chart = json!([{"items": [{"type": "google_trends_graph", "data": [{"date_from": "2026-10-04", "values": [null, 80]}, {"date_from": "2026-10-11", "values": [12, 100]},
        {"date_from": "2026-10-18", "values": [null, null]}, {"date_from": "2026-10-25", "values": [null, 60]}]}]}]);
    assert_eq!((measure(&chart, 0), measure(&chart, 1)), ((3.0, 0.25), (60.0, 0.75)), "a week with no value for any word still counts as a week without searches");
    let mut ledger = vec![word("카톡 분석", CONFIRMED)];
    upsert(&mut ledger, Word { meaning: String::new(), ..word("카톡  분석", PENDING) });
    assert_eq!((ledger.len(), ledger[0].status.as_str(), ledger[0].meaning.as_str()), (1, CONFIRMED, "m"), "an empty curve later never unconfirms a word");
    upsert(&mut ledger, word("대화 분석 ai", PENDING));
    upsert(&mut ledger, Word { share: 0.4, meaning: String::new(), ..word("대화 분석 ai", CONFIRMED) });
    assert_eq!((ledger.len(), ledger[1].status.as_str(), ledger[1].share, ledger[1].meaning.as_str()), (2, CONFIRMED, 0.4, "m"), "a pending word is confirmed once it has a curve");
}

#[test]
fn a_confirmed_local_word_an_english_rewrite_removed_is_reported() {
    let copy = |locale: &str| (locale == "ko").then(|| COPY.to_string());
    let used = |w: &str| Word { adopted: true, ..word(w, CONFIRMED) };
    let ledger = vec![used("카톡 분석"), used("사진 용량 줄이기"), word("카톡 답장", CONFIRMED), word("읽씹 심리", PENDING)];
    let lost = missing(&ledger, copy);
    let words: Vec<&str> = lost.iter().map(|w| w.word.as_str()).collect();
    assert_eq!(words, ["사진 용량 줄이기"], "it now sits on another page only; a confirmed phrasing the page never used and a pending word are no loss");
    assert!(page_block(COPY, "/text-message-analyzer").unwrap().ends_with("  },") && page_block(COPY, "/relationship-red-flags").unwrap().contains("레드플래그"));
    assert_eq!(page_block(COPY, "/personal-crm"), None);
    assert_eq!(missing(&ledger, |_| None).len(), 2, "an unreadable copy file loses every adopted word");
    assert_eq!(kept_note(&ledger, "/text-message-analyzer"),
        "；该页已确认的本地词（英文改稿不得覆盖）：ko「카톡 분석」、ko「사진 용량 줄이기」；已确认但页面还没用的本地词：ko「카톡 답장」；待验证的本地词（保留，不删）：ko「읽씹 심리」");
    assert_eq!(kept_note(&ledger, "/personal-crm"), "");
    // A confirmed word becomes guarded once the page uses it, and stays guarded through a later check.
    let mut fresh = vec![word("카톡 분석", CONFIRMED), word("카톡 답장", CONFIRMED), word("대화 분석 ai", PENDING)];
    assert_eq!((adopt(&mut fresh, copy), fresh[0].adopted, fresh[1].adopted, fresh[2].adopted), (1, true, false, false));
    upsert(&mut fresh, word("카톡 분석", CONFIRMED));
    assert!(fresh[0].adopted && missing(&fresh, |_| Some("export const keywordPagesKo = {\n  'text-message-analyzer': {\n    keywords: ['chat analyzer'],\n  },\n}\n".into())).len() == 1);
}

#[test]
fn the_ledger_in_the_repository_matches_the_live_locale_copy() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../");
    let lost = findings(&format!("{root}{LEDGER}"), &format!("{root}{COPY_DIR}"));
    assert!(lost.is_empty(), "{:?}", lost.iter().map(|f| &f.title).collect::<Vec<_>>());
}
