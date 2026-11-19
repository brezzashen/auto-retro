use super::watch_hour_digest::digest;
use serde_json::json;

fn write(path: &str, text: &str) {
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn analysis(first_incomplete: &str) -> serde_json::Value {
    json!({
        "at": "2026-11-18T13:17:00+08:00",
        "errors": [],
        "traffic": {
            "segments": [
                {"window_hours": 1, "offset_hours": 0, "source": "google", "segment": "新访客", "visitors": 3},
                {"window_hours": 1, "offset_hours": 24, "source": "google", "segment": "新访客", "visitors": 1},
                {"window_hours": 24, "offset_hours": 24, "source": "google", "segment": "新访客", "visitors": 40}
            ],
            "sources": [], "batches": [], "payments": [],
            "hour": {"visitors": [{"source": "google", "os": "iOS"}], "db": {"counts": {"signups_h0": 1}, "orders": [], "subscription_changes": [],
                "followup": {"signups": 2, "chatted": 1, "paid": 0, "accounts": [{"user8": "abcd1234"}]}}}
        },
        "gsc": {"hourly": {"metadata": {"firstIncompleteHour": first_incomplete}, "rows": [
            {"keys": ["2026-11-17T12:00:00-08:00"], "impressions": 5},
            {"keys": ["2026-11-17T13:00:00-08:00"], "impressions": 7},
            {"keys": ["2026-11-17T14:00:00-08:00"], "impressions": 2}
        ]}},
        "anomalies": [{"key": "anomaly:x"}],
        "findings": [{"severity": "P1", "key": "k1", "title": "t1", "action": "a"}, {"severity": "P2", "key": "k2", "title": "t2", "action": "a"}],
        "geo": {"hour": "2026-11-18T13", "records": [], "unmeasured": [{"engine": "doubao"}]},
        "aisaSeo": {"skipped": null, "aiModeRun": [], "links": {}, "props": {"big": 1}},
        "creatorPosts": null,
        "googleAds": {"source": "ui", "tabs": {"campaigns_today": [{"name": "c1"}]}}
    })
}

#[test]
fn digest_prints_the_hour_inputs_in_one_call() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    let prev = format!("{state}/runs/2026-11-18T12");
    let run = format!("{state}/runs/2026-11-18T13");
    write(&format!("{prev}/analysis.json"), &analysis("2026-11-17T12:00:00-08:00").to_string());
    write(&format!("{run}/analysis.json"), &analysis("2026-11-17T14:00:00-08:00").to_string());
    write(&format!("{run}/report.md"), "# x\n## 访客类型分布（近 24 小时 / 近 1 小时）\nrow\n## 付费与订单\n");
    write(&format!("{state}/retro/2026-11-18T12.md"), "报告：https://r\n本小时 3 人\n");
    write(&format!("{state}/trends/latest.json"), &json!({"at": "2026-11-18T10:00", "heat": [], "noise": 0, "errors": [], "rising": [
        {"query": "nano banana", "value": 1.0, "source": "seed:x", "kind": "unknown", "page": ""},
        {"query": "Judged Word", "value": 1.0, "source": "seed:x", "kind": "unknown", "page": ""},
        {"query": "mars crm", "value": 1.0, "source": "seed:x", "kind": "brand", "page": ""}
    ]}).to_string());
    write(&format!("{state}/trend-labels.jsonl"), "{\"query\":\"judged word\",\"label\":\"噪声\"}\n");

    let text = digest(&run, &state).unwrap();
    assert!(text.contains("segments 本小时（window_hours 1, offset_hours 0，1 行）") && text.contains("segments 昨天同一小时（window_hours 1, offset_hours 24，1 行）"));
    assert!(!text.contains("\"visitors\":40"), "the previous-day 24-hour window is not printed");
    assert!(text.contains("hour.visitors (1):") && text.contains("\"signups_h0\":1") && text.contains("\"chatted\":1") && text.contains("abcd1234"));
    assert!(text.contains("新完成的小时（2 行") && text.contains("\"impressions\":5") && text.contains("\"impressions\":7") && !text.contains("\"impressions\":2"),
        "only the Search Console hours completed since the previous run");
    assert!(text.contains("\"key\":\"k1\"") && !text.contains("\"key\":\"k2\"") && text.contains("P2: 1 条"));
    assert!(text.contains("geo.unmeasured: [{\"engine\":\"doubao\"}]") && !text.contains("\"big\""), "aisaSeo.props stays out of the print");
    assert!(text.contains("campaigns_today (1):"));
    assert!(text.contains("待判断 1 个") && text.contains("nano banana") && !text.contains("Judged Word") && !text.contains("mars crm"));
    assert!(text.contains("本小时 3 人") && text.contains(&format!("sed -n '2,3p' {run}/report.md")));
}

#[test]
fn digest_says_when_search_console_did_not_advance() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().to_string();
    for hour in ["2026-11-18T12", "2026-11-18T13"] {
        write(&format!("{state}/runs/{hour}/analysis.json"), &analysis("2026-11-17T14:00:00-08:00").to_string());
    }
    let mut a = analysis("2026-11-17T14:00:00-08:00");
    a["traffic"] = serde_json::Value::Null;
    write(&format!("{state}/runs/2026-11-18T13/analysis.json"), &a.to_string());
    let text = digest(&format!("{state}/runs/2026-11-18T13"), &state).unwrap();
    assert!(text.contains("GSC 本小时无新数据") && text.contains("traffic: null"));
}
