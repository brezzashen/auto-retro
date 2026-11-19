//! One run's numbers and the decision to open a session.
//!
//! Thresholds (week over week, against the previous stored capture):
//! - GROWTH_CHARS: prompt-dedup's per-change gate lets a change add 300 tokens (≈ 900 characters)
//!   without a reason; 3,000 characters (≈ 1,000 tokens, 1.2 % of the 2026-11-09 prefix) is more
//!   than three such changes in one week. 2026-10-08 → 11-09 averaged about 16,000 a week.
//! - Analyzer gate: a new structural repeat the per-change gate fails (subtree ≥ 200, description
//!   ≥ 100 characters, a repeated system sentence) reached mastra anyway.
//! - NEW_GROUP_WASTE / NEW_WASTE_TOTAL: a paraphrased copy of a rule in the standing prefix is what
//!   the analyzer cannot see; 150 characters (≈ 50 tokens on every call) is the paraphrase analogue
//!   of the gate's 100-character repeated description, 300 in total catches several small copies.
//! - SEMANTIC_GROWTH: standing semantic waste grew by 1,000 characters, so copies accumulate even
//!   when each one is small.
use serde_json::{json, Value};

pub const GROWTH_CHARS: i64 = 3_000;
pub const NEW_GROUP_WASTE: usize = 150;
pub const NEW_WASTE_TOTAL: usize = 300;
pub const SEMANTIC_GROWTH: i64 = 1_000;
/// prompt-dedup's recorded reference point (SKILL.md, 2026-10-08 at 0.1.1.4390).
pub const BASELINE: (&str, i64) = ("2026-10-08", 168_526);

fn sum(list: &Value, key: &str) -> i64 {
    list.as_array().into_iter().flatten().filter_map(|r| r[key].as_i64()).sum()
}

/// The numbers of one analyzer report (`analyze-prefix --json`, `.report`).
pub fn of_report(report: &Value) -> Value {
    let size = &report["size"];
    let tool = |name: &str| size["byTool"].as_array().into_iter().flatten().find(|t| t["name"] == name).and_then(|t| t["chars"].as_i64());
    json!({
        "prefix": size["total"], "system": size["system"], "tools": size["tools"], "create": tool("create"),
        "tool_count": size["byTool"].as_array().map(Vec::len),
        "structural_waste": sum(&report["subtrees"], "waste"), "description_waste": sum(&report["descriptions"], "waste"),
        "constraint_chars": report["constraints"]["chars"],
        "system_internal_tokens": report["system"]["internal"]["tokens"], "system_cross_tokens": report["system"]["cross"]["tokens"],
        "system_tokens": report["system"]["tokens"], "repeated_sentences": report["system"]["repeatedSentences"].as_array().map(Vec::len),
    })
}

pub fn delta(cur: &Value, prev: &Value, key: &str) -> i64 {
    cur[key].as_i64().unwrap_or(0) - prev[key].as_i64().unwrap_or(0)
}

/// Reasons to open a session; empty means none.
pub fn reasons(cur: &Value, prev: &Value, gate_failures: &[String], semantic: &Value, prev_standing: i64) -> Vec<Value> {
    let mut out = Vec::new();
    let growth = delta(cur, prev, "prefix");
    if growth >= GROWTH_CHARS {
        out.push(json!({"code": "growth", "value": growth, "limit": GROWTH_CHARS, "text": format!("前缀一周增长 {growth} 字符（阈值 {GROWTH_CHARS}）")}));
    }
    if !gate_failures.is_empty() {
        out.push(json!({"code": "gate", "value": gate_failures.len(), "failures": gate_failures, "text": format!("prompt-dedup 门禁对上周失败 {} 项", gate_failures.len())}));
    }
    let groups = semantic["groups"].as_array().cloned().unwrap_or_default();
    let new_waste = |g: &Value| g["new_standing_waste"].as_u64().unwrap_or(0) as usize;
    let big: Vec<&Value> = groups.iter().filter(|g| new_waste(g) >= NEW_GROUP_WASTE).collect();
    let total: usize = groups.iter().map(new_waste).sum();
    if !big.is_empty() || total >= NEW_WASTE_TOTAL {
        out.push(json!({"code": "semantic_new", "value": total, "groups": big.iter().map(|g| &g["id"]).collect::<Vec<_>>(),
            "text": format!("新增语义重复 {total} 字符（单组阈值 {NEW_GROUP_WASTE}，合计阈值 {NEW_WASTE_TOTAL}）")}));
    }
    let semantic_growth = semantic["standing_waste"].as_i64().unwrap_or(0) - prev_standing;
    if semantic_growth >= SEMANTIC_GROWTH {
        out.push(json!({"code": "semantic_growth", "value": semantic_growth, "limit": SEMANTIC_GROWTH, "text": format!("常驻语义重复增长 {semantic_growth} 字符（阈值 {SEMANTIC_GROWTH}）")}));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_fire_on_growth_gate_and_new_copies() {
        let (cur, prev) = (json!({"prefix": 250_000}), json!({"prefix": 246_000}));
        let sem = json!({"standing_waste": 2_000, "groups": [{"id": "S1", "new_standing_waste": 160}]});
        let codes: Vec<String> = reasons(&cur, &prev, &["x".into()], &sem, 500).iter().map(|r| r["code"].as_str().unwrap().into()).collect();
        assert_eq!(codes, ["growth", "gate", "semantic_new", "semantic_growth"]);
        let quiet = json!({"standing_waste": 600, "groups": [{"id": "S1", "new_standing_waste": 100}]});
        assert!(reasons(&json!({"prefix": 247_000}), &prev, &[], &quiet, 500).is_empty());
    }
}
