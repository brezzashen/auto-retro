use super::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

fn write_sample_run(dir: &std::path::Path) {
    let analysis = json!({
        "at": "2026-11-11T00:17:00+08:00",
        "indexed": [455, 470],
        "last24h": {"impressions": 328.0, "clicks": 4.0},
        "previous24h": {"impressions": 242.0, "clicks": 4.0},
        "newPageGate": {"note": "新页闸门：达标，今天最多新建 3 页"},
        "probes": [{"url": "https://example.com/ai-relationship-coach", "status": 200}],
        "findings": [{"key": "ctr:/ai-relationship-coach", "severity": "P1", "title": "低点击率", "action": "改标题"}],
        "gsc": {
            "hourly": {"metadata": {"firstIncompleteHour": "2026-11-10T13:00:00Z"}},
            "pages": {"rows": [{"keys": ["https://example.com/ai-relationship-coach"], "impressions": 465.0, "clicks": 2.0, "ctr": 0.0043, "position": 9.46}]},
            "queryPage": {"rows": []}
        },
        "traffic": {
            "segments": [{"window_hours": 24, "offset_hours": 0, "source": "$direct", "segment": "新访客", "visitors": 26.0, "cta": 15.0, "signups": 0.0, "senders": 0.0}],
            "batches": [],
            "sources": []
        },
        "trends": {"rising": []}
    });
    std::fs::write(dir.join("analysis.json"), serde_json::to_string(&analysis).unwrap()).unwrap();
}

#[test]
fn help_prints_usage_without_side_effects() {
    assert_eq!(main(&["--help".into()]), 0);
}

#[test]
fn dry_run_packs_backfilled_anomalies_and_fable_draft() {
    let tmp = tempfile::tempdir().unwrap();
    write_sample_run(tmp.path());
    let draft = tmp.path().join("fable-draft.md");
    std::fs::write(&draft, "拟直接重写 /ai-relationship-coach 的 seoTitle").unwrap();
    let out = tmp.path().join("advisor.md");
    let code = main(&[
        "--run".into(), tmp.path().to_string_lossy().into(),
        "--draft".into(), draft.to_string_lossy().into(),
        "--out".into(), out.to_string_lossy().into(),
        "--dry-run".into(),
    ]);
    assert_eq!(code, 0);
    let prompt = std::fs::read_to_string(format!("{}.prompt.txt", out.display())).unwrap();
    assert!(prompt.contains("anomaly:page1-ctr:/ai-relationship-coach") || prompt.contains("/ai-relationship-coach"));
    assert!(prompt.contains("新访客 CTA→注册漏斗"));
    assert!(prompt.contains("拟直接重写 /ai-relationship-coach 的 seoTitle"));
    assert!(prompt.contains("双顾问分歧裁决表"));
    assert!(prompt.contains(super::super::watch_judge_rules::CORE_MARKET) && prompt.contains("不要把真人或本地咨询意图当成否决理由"));
    assert!(prompt.contains("## 6. Fable 5.2 主导会话的初判"), "the daily retro is Fable-led");
}

#[test]
fn the_prompt_names_the_tab_that_actually_leads() {
    assert_eq!(lead_name("daily"), "Fable 5.2");
    assert_eq!(lead_name("keywords"), "Opus 5.5");
    assert_eq!(lead_name("hourly"), "Opus 5.5");
    let kw = build_prompt("keywords", "pack", true);
    assert!(kw.contains("与主导会话 Opus 5.5 进行对抗性分析") && kw.contains("| Opus 5.5 倾向 |") && !kw.contains("Fable"), "founder 2026-11-17: the keyword room debate is Opus 5.5 against Neon");
}

#[test]
fn falls_back_to_low_model_and_writes_receipt_when_primary_fails() {
    let tmp = tempfile::tempdir().unwrap();
    write_sample_run(tmp.path());
    let fake_ag1 = tmp.path().join("fake-ag1.sh");
    std::fs::write(
        &fake_ag1,
        "#!/bin/sh\nif [ \"$2\" = \"gemini-4-neon-medium\" ]; then echo 'quota timeout' >&2; exit 3; fi\necho '### 1. 对 Fable 5.2 方案的反驳与盲点\n标题刚改不满 7 天，禁动。'\n",
    ).unwrap();
    std::fs::set_permissions(&fake_ag1, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = tmp.path().join("advisor.md");
    let code = main(&[
        "--run".into(), tmp.path().to_string_lossy().into(),
        "--ag1".into(), fake_ag1.to_string_lossy().into(),
        "--out".into(), out.to_string_lossy().into(),
    ]);
    assert_eq!(code, 0);
    let md = std::fs::read_to_string(&out).unwrap();
    assert!(md.contains("gemini-4-neon-low") && md.contains("标题刚改不满 7 天"));
    let receipt: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{}.json", out.display())).unwrap()).unwrap();
    assert_eq!(receipt["fallbackUsed"], true);
    assert_eq!(receipt["ok"], true);
    assert!(receipt["anomalyCount"].as_u64().unwrap() >= 2);
}

#[test]
fn pack_reports_a_failed_segments_query_as_unread_not_zero() {
    let tmp = tempfile::tempdir().unwrap();
    write_sample_run(tmp.path());
    let path = tmp.path().join("analysis.json");
    let mut analysis: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    analysis["traffic"] = json!({"segments": [], "segments_error": "PostHog query/: HTTP 504", "days": [{"day": "2026-11-12", "visitors": 318.0}, {"day": "2026-11-13", "visitors": 194.0}]});
    std::fs::write(&path, serde_json::to_string(&analysis).unwrap()).unwrap();
    let (pack, _) = crate::skills::seo_competitor::watch_advisor_pack::build_pack(&tmp.path().to_string_lossy(), None, None, "Opus 5.5").unwrap();
    assert!(pack.contains("未读到（PostHog query/: HTTP 504）") && pack.contains("2026-11-13 194 人"));
    assert!(!pack.contains("生产真实访客"));
    analysis["traffic"] = serde_json::Value::Null;
    std::fs::write(&path, serde_json::to_string(&analysis).unwrap()).unwrap();
    let (pack, _) = crate::skills::seo_competitor::watch_advisor_pack::build_pack(&tmp.path().to_string_lossy(), None, None, "Opus 5.5").unwrap();
    assert!(pack.contains("本轮没有 PostHog 数据") && pack.contains("按天真实访客：无"));
}
