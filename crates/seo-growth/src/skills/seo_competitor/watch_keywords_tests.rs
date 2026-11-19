use super::*;
use super::super::watch_config::read_config;

fn config() -> WatchConfig {
    let mut c = read_config(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/seo_competitor/watch.config.json")).unwrap();
    c.targets.push(super::super::watch_config::TargetKeyword { keyword: "best ai for relationship advice".into(), page: "/ai-relationship-coach".into() });
    c
}

#[test]
fn a_dropped_keyword_waits_four_weeks_and_a_handed_one_never_returns() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
    let dropped = |day: &str| Entry { verdict: DROP.into(), last_judged: day.into(), ..Default::default() };
    assert!(settled(Some(&dropped("2026-11-10")), today));
    assert!(!settled(Some(&dropped("2026-11-01")), today));
    assert!(settled(Some(&Entry { verdict: DO.into(), dispatched: Some("2026-11-29".into()), ..Default::default() }), today));
    assert!(!settled(None, today));
    let checked = Entry { verdict: "观察".into(), win_rate: Some(0.3), last_judged: "2026-11-27".into(), ..Default::default() };
    assert!(settled(Some(&checked), today), "a SERP-checked keyword waits a week before another SERP");
}

#[test]
fn a_handed_keyword_ships_when_it_appears_in_targets() {
    let mut ledger = Ledger::new();
    ledger.insert("best ai for relationship advice".into(), Entry { dispatched: Some("2026-11-04".into()), ..Default::default() });
    let dir = std::env::temp_dir().join("seo-watch-follow-test");
    std::fs::create_dir_all(&dir).unwrap();
    follow(&mut ledger, &config(), dir.to_str().unwrap(), "2026-11-05");
    assert_eq!(ledger["best ai for relationship advice"].shipped.as_deref(), Some("2026-11-05"));
}

#[test]
fn the_round_is_due_once_a_day_at_its_hour_or_when_forced() {
    let c = config();
    let at = |t: &str| DateTime::parse_from_rfc3339(t).unwrap();
    let state = WatchState::default();
    assert!(due(&c, &state, at("2026-11-05T08:17:00+08:00"), false));
    assert!(!due(&c, &state, at("2026-11-05T09:17:00+08:00"), false));
    assert!(due(&c, &state, at("2026-11-05T09:17:00+08:00"), true));
    let done = WatchState { last_discover_day: Some("2026-11-05".into()), ..Default::default() };
    assert!(!due(&c, &done, at("2026-11-05T08:17:00+08:00"), true));
}

#[test]
fn a_strong_uncovered_pick_is_handed_over_as_the_day_s_new_page() {
    let c = config();
    let cfg = c.discovery.as_ref().unwrap();
    let pick = |k: &str, page: &str| Judged { candidate: super::super::watch_discover::Candidate { keyword: k.into(), page: page.into(), volume: 2400.0,
        ..Default::default() }, verdict: DO.into(), win_rate: Some(0.6), ..Default::default() };
    // The 2026-11-10 dry run's picks, all on pages that exist, plus one need no page covers.
    let picks = [pick("pocket ai recorder", "/ai-voice-recorder"), pick("red flags in a guy", "/relationship-red-flags"),
        pick("how to get him back", "/win-him-back"), pick("breakup text generator", "/text-message-analyzer")];
    let own = super::super::watch_own_page::choose(&picks, cfg, &c.targets);
    assert_eq!(own, [("breakup text generator".to_string(), "/breakup-text-generator".to_string())]);
    let gate = super::super::watch_page_gate::Gate { cap: 1, open: false, note: "新页闸门：不达标，回落".into(), ..Default::default() };
    let local = [super::super::watch_locale_words::Word { locale: "ko".into(), page: "/ai-voice-recorder".into(), word: "녹음 요약".into(), status: "confirmed".into(), adopted: true, ..Default::default() }];
    let f = findings(&picks, "/d", &gate, &own, &local);
    assert!(f[0].action.starts_with("最接近页面 /ai-voice-recorder；该页已确认的本地词（英文改稿不得覆盖）：ko「녹음 요약」；规则唱反调"), "{}", f[0].action);
    assert!(f[3].action.starts_with("新建落地页 /breakup-text-generator（G8") && f[0].action.starts_with("最接近页面 /ai-voice-recorder"));
    assert!(f[4].action.contains("新页名额给「breakup text generator」") && f[4].action.contains("；新页闸门：不达标，回落；ja/ko/th 页面的词先跑 localeWords verify"));
    assert!(f[4].title.contains("最多新建 1 个落地页"), "the gate's cap, not the configured 3");
}
