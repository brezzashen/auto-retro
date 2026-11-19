use super::*;
use crate::skills::seo_competitor::watch_signals;
use serde_json::json;

fn word(volume: f64, kd: f64, cpc: f64) -> Candidate {
    Candidate { keyword: "w".into(), volume, kd: Some(kd), cpc, ..Default::default() }
}

#[test]
fn the_beginner_filter_orders_words_by_cpc_up_to_the_priced_click_bonus() {
    // Gefei 2023-10-15, top rows of his table: "neb tm calculator" 5400 / KD 19 / $5.79, "construction loan calculator" 6600 / KD 14 / $1.71.
    let (dear, cheap) = (word(5400.0, 19.0, 5.79), word(6600.0, 14.0, 1.71));
    assert!(near_money(&dear) && near_money(&cheap));
    assert_eq!(weight(&dear), 1.5, "a CPC over $3 earns the whole bonus");
    assert!((weight(&cheap) - 1.285).abs() < 1e-9, "$1.71 earns 1.71 / 3 of it");
    assert!(watch_signals::weight(&dear) > watch_signals::weight(&cheap), "the order follows CPC");
    assert!(notes(&cheap).iter().any(|n| n.contains("离钱近")));
}

#[test]
fn a_word_outside_the_filter_gets_no_cpc_bonus_and_is_never_demoted() {
    assert!(!near_money(&word(590.0, 10.0, 9.0)), "under 600 searches");
    assert!(!near_money(&word(900.0, 29.0, 9.0)), "KD must be under 29");
    assert!(!near_money(&word(900.0, 10.0, 0.05)), "CPC under $0.1");
    assert!(!near_money(&Candidate { kd: None, ..word(900.0, 0.0, 9.0) }), "no KD, no filter");
    assert_eq!(weight(&word(900.0, 10.0, 0.0)), 1.0, "a free click is no penalty");
    assert_eq!(weight(&word(70.0, 0.0, 51.92)), 1.5, "the priced click (CPC >= 3, KD <= 15) keeps its bonus under 600 searches");
}

#[test]
fn each_paid_demand_signal_adds_a_tenth() {
    let ads = Candidate { competition: 0.67, ..word(100.0, 40.0, 0.0) };
    assert!(advertised(&ads) && !advertised(&Candidate { competition: 0.66, ..ads.clone() }));
    assert!((weight(&ads) - 1.1).abs() < 1e-9);
    let all = Candidate { price_pages: 2, app_listings: 1, ..ads.clone() };
    assert_eq!(paid_signals(&all), 3);
    assert!((weight(&all) - 1.3).abs() < 1e-9);
    assert!(notes(&ads)[0].contains("Google Ads 竞争度高（0.67）"));
    assert_eq!(serp_note(&all), "；付费信号：首页 2 个结果带标价或价格页、首页有 1 个应用商店 App（内购金额未读）");
    assert_eq!(serp_note(&ads), "");
}

#[test]
fn prices_pricing_sitelinks_and_app_listings_are_read_from_the_first_page() {
    // Shapes of the 2026-11-10 SERP cache.
    let priced = json!({"breadcrumb": "https://www.nutshell.com › CRM & Sales Software", "price": {"current": 4.3, "regular": 13, "currency": "USD"}});
    let sitelink = json!({"breadcrumb": "https://writeme.ai", "price": null, "links": [{"title": "Use Cases"}, {"title": "Pricing"}]});
    let app = json!({"domain": "google.com", "breadcrumb": "https://play.google.com › store › apps › details › id=com...", "website_name": "Google Play", "price": null});
    let ios = json!({"domain": "google.com", "breadcrumb": "https://apps.apple.com › app › mobile-xrm", "price": null});
    let plain = json!({"breadcrumb": "https://www.scienceofpeople.com › Blog", "price": null, "links": [{"title": "Read more"}]});
    assert_eq!(serp_signals(&[&priced, &sitelink, &app, &ios, &plain]), (2, 2));
    assert_eq!(serp_signals(&[&plain]), (0, 0));
}
