//! Scores one window of search events against the local-search policy (AGENTS.md, plan B of the
//! 2026-10-23 three-model test): history and reports only through local-search, broad searches
//! start with local-search, rg/grep confirm inside a located file.
//!
//! Components, each 0..1 (1 when it has no events):
//!   history_report = local-search calls on history/report ÷ (those + blocked, escaped and unguarded sweeps there)
//!   no_tree_grep   = local-search calls ÷ (local-search calls + rg/grep tree sweeps of an indexed tree that ran)
//!   phrase_routing = local-search code/docs/all calls ÷ (those + phrase tree sweeps over code)
//! A tree sweep has `scope` "tree"; "file" and "files" (a one-level glob or an explicit file list)
//! are confirmations inside located files. Unindexed trees (target "other": /etc, scratchpads, and
//! ~/automation config/ and host/) are rg territory under the policy and never count (search-retro
//! 84070750632218527984); the rest of ~/automation is code since search-retro 810449725933314328503.
//! A `LOCAL_SEARCH_MISS=1` tree sweep counts as a sweep when script-guard found no local-search call
//! of the session in the ten minutes before it (`escape_basis` "none"); escapes with a basis, and
//! older events without the field, stay free (search-retro 813708664581992389150: the free escape
//! carried tree sweeps from 1 to 80 a window while the counted ones fell, so 84 read as about 76).
//! Agents without a PreToolUse hook (Antigravity) are metered from their bridged transcripts
//! (`unhooked.rs`, founder decision 1A on search-retro 89311962361037056406): a search the hook
//! would have refused ran anyway and has verdict "unguarded", which counts as run.
//! score = 100 × (0.4 history_report + 0.4 no_tree_grep + 0.2 phrase_routing); grade A ≥ 90,
//! B ≥ 75, C ≥ 60, D ≥ 40, else F.

use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) fn score(events: &[Value]) -> Value {
    let s = |e: &Value, k: &str| e[k].as_str().unwrap_or("").to_string();
    let is_ls = |e: &Value| s(e, "kind") == "local_search";
    // script-guard logs each segment of one Bash command with the same session and ts_ms; when one
    // segment is blocked the whole command never ran (search-retro 8486853407474113856: an
    // "allowed" regex tree sweep sharing 7473551a…jsonl:451 with a blocked one counted as run).
    let call = |e: &Value| (s(e, "session"), s(e, "profile"), e["ts_ms"].as_f64().unwrap_or(-1.0).to_bits());
    let refused: std::collections::HashSet<_> = events.iter().filter(|e| s(e, "verdict") == "blocked").map(call).collect();
    let ran = |e: &Value| matches!(s(e, "verdict").as_str(), "allowed" | "hint" | "unguarded" | "fallback") && !refused.contains(&call(e));
    let hr = |e: &Value| matches!(s(e, "target").as_str(), "history" | "report");
    let indexed = |e: &Value| matches!(s(e, "target").as_str(), "code" | "docs");
    let unverified = |e: &Value| s(e, "verdict") == "escape" && s(e, "escape_basis") == "none";
    let tree_grep = |e: &Value| s(e, "kind") == "grep" && s(e, "scope") == "tree" && indexed(e) && (ran(e) || unverified(e));

    let ls_hr = events.iter().filter(|e| is_ls(e) && hr(e)).count();
    let blocked = events.iter().filter(|e| s(e, "verdict") == "blocked" && hr(e)).count();
    let escaped = events.iter().filter(|e| s(e, "verdict") == "escape" && hr(e)).count();
    let unguarded = events.iter().filter(|e| s(e, "verdict") == "unguarded" && hr(e)).count();
    let ls_all = events.iter().filter(|e| is_ls(e)).count();
    let trees = events.iter().filter(|e| tree_grep(e)).count();
    let ls_code = events.iter().filter(|e| is_ls(e) && matches!(s(e, "target").as_str(), "code" | "docs" | "all")).count();
    let phrase_code = events.iter().filter(|e| tree_grep(e) && s(e, "target") == "code" && s(e, "pattern") == "phrase").count();

    let ratio = |good: usize, bad: usize| if good + bad == 0 { 1.0 } else { good as f64 / (good + bad) as f64 };
    let (h, t, p) = (ratio(ls_hr, blocked + escaped + unguarded), ratio(ls_all, trees), ratio(ls_code, phrase_code));
    let points = (100.0 * (0.4 * h + 0.4 * t + 0.2 * p)).round();
    let grade = match points as i64 { 90.. => "A", 75..=89 => "B", 60..=74 => "C", 40..=59 => "D", _ => "F" };

    json!({
        "score": points, "grade": grade,
        "components": {"history_report": round(h), "no_tree_grep": round(t), "phrase_routing": round(p)},
        "counts": {"events": events.len(), "local_search": ls_all, "local_search_history_report": ls_hr, "local_search_code_docs": ls_code,
            "grep_tree_ran": trees, "grep_file": events.iter().filter(|e| s(e, "kind") == "grep" && matches!(s(e, "scope").as_str(), "file" | "files")).count(),
            "escaped_code": events.iter().filter(|e| s(e, "verdict") == "escape" && !hr(e)).count(),
            // No local-search call of the session in the ten minutes before (script-guard `escape_basis`).
            "escaped_code_unverified": events.iter().filter(|e| s(e, "verdict") == "escape" && s(e, "escape_basis") == "none").count(),
            "blocked": blocked, "escaped": escaped, "unguarded_history_report": unguarded,
            "unguarded": events.iter().filter(|e| s(e, "verdict") == "unguarded").count(), "phrase_tree_code": phrase_code},
        // script-guard fallback mode (2026-11-05): sweeps that ran after a local-search call, and why.
        "fallback": {"count": events.iter().filter(|e| s(e, "verdict") == "fallback").count(),
            "reasons": top(events.iter().filter(|e| s(e, "kind") == "grep" && !s(e, "fallback_reason").is_empty()).map(|e| s(e, "fallback_reason")), 10)},
        "by_agent": tally(events, |e| format!("{}/{}", s(e, "agent"), s(e, "profile")), &is_ls, &tree_grep),
        "top_tree_greps": top(events.iter().filter(|e| tree_grep(e)).map(|e| normalize(&s(e, "cmd"))), 20),
        "top_blocked": top(events.iter().filter(|e| s(e, "verdict") == "blocked").map(|e| normalize(&s(e, "cmd"))), 10),
        "sessions_without_local_search": sessions_without(events, &is_ls, &tree_grep),
    })
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Commands with quoted patterns and long paths folded, so repeats of one habit count together.
fn normalize(cmd: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => { quote = None; out.push_str("<pat>"); }
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None => out.push(c),
        }
    }
    out.split_whitespace().map(|w| if w.matches('/').count() >= 3 { "<path>" } else { w }).take(12).collect::<Vec<_>>().join(" ")
}

fn top(items: impl Iterator<Item = String>, n: usize) -> Vec<Value> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for item in items {
        *counts.entry(item).or_default() += 1;
    }
    let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.into_iter().take(n).map(|(cmd, count)| json!({"pattern": cmd, "count": count})).collect()
}

fn tally(events: &[Value], key: impl Fn(&Value) -> String, is_ls: &dyn Fn(&Value) -> bool, tree: &dyn Fn(&Value) -> bool) -> Value {
    let mut out: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for e in events {
        let entry = out.entry(key(e)).or_default();
        entry.0 += 1;
        entry.1 += usize::from(is_ls(e));
        entry.2 += usize::from(tree(e));
    }
    out.into_iter().map(|(k, (n, ls, tr))| (k, json!({"events": n, "local_search": ls, "grep_tree_ran": tr}))).collect::<serde_json::Map<_, _>>().into()
}

/// Sessions with at least three tree sweeps and no local-search call, busiest first.
fn sessions_without(events: &[Value], is_ls: &dyn Fn(&Value) -> bool, tree: &dyn Fn(&Value) -> bool) -> Vec<Value> {
    let mut per: BTreeMap<String, (usize, usize, String)> = BTreeMap::new();
    for e in events {
        let session = e["session"].as_str().unwrap_or("").to_string();
        let entry = per.entry(session).or_insert((0, 0, e["profile"].as_str().unwrap_or("").to_string()));
        entry.0 += usize::from(tree(e));
        entry.1 += usize::from(is_ls(e));
    }
    let mut rows: Vec<(String, usize, String)> = per.into_iter().filter(|(_, (t, l, _))| *t >= 3 && *l == 0).map(|(s, (t, _, p))| (s, t, p)).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    rows.into_iter().take(15).map(|(session, trees, profile)| json!({"session": session, "profile": profile, "grep_tree_ran": trees})).collect()
}
