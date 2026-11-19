//! A local-search call whose hit lines the agent's own excluding line filter (`| grep -v …`) removed.
//! Search-retro 82856125487639539959 (25d92dc6…jsonl:1050): `search "event_extraction::|
//! extract_event_details\(" … | grep -v "tests/\|mars-contracts/src/event_extraction.rs"` printed
//! nothing, yet the engine logged 10 hits for it (queries.jsonl ts 1794761365345): every hit lay in
//! the test files and the definition the filter named. The `rg` that followed printed that same
//! definition line, so the call counted as a false NONE. An excluding filter passes a NONE line, so
//! a call that printed no file and no NONE while its engine row holds hits is the filter's empty
//! answer, not the engine's: it is unobserved, as a selecting filter's empty output is
//! (`returned::filtered`). A call whose engine row has no hits, or that has no row, counts as before.
use super::quality::query_of;
use super::ran::normalize;
use serde_json::Value;
use std::path::Path;

/// Engine rows are written when the search ends; transcript clocks may differ by a little.
const SLACK_MS: f64 = 5_000.0;
/// Results arrive after the window's last call; keep their rows too.
const TAIL_MS: f64 = 15.0 * 60_000.0;

/// Agent engine rows with hits `(ts_ms, normalized query)` near the window.
pub(super) fn hit_rows(home: &Path, from_ms: f64, to_ms: f64) -> Vec<(f64, String)> {
    let text = std::fs::read_to_string(home.join(".local/share/local-search/queries.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|r| r.get("caller").is_none() && r["hits"].as_u64().is_some_and(|h| h > 0))
        .filter_map(|r| Some((r["ts_ms"].as_f64()?, normalize(r["query"].as_str()?))))
        .filter(|(t, _)| *t >= from_ms - SLACK_MS && *t <= to_ms + TAIL_MS)
        .collect()
}

/// True when the call's stdout goes through an excluding line filter, its output holds no NONE
/// line, and an engine row with hits for its query falls between the call and its result.
pub(super) fn emptied(command: &str, output: &str, rows: &[(f64, String)], call_ms: f64, result_ms: f64) -> bool {
    if rows.is_empty() || !super::returned::excluded(command) || output.lines().any(|l| l.trim_start().starts_with("NONE")) {
        return false;
    }
    let Some(query) = query_of(command) else { return false };
    let query = normalize(&query);
    rows.iter().any(|(t, q)| *q == query && *t >= call_ms - SLACK_MS && *t <= result_ms + SLACK_MS)
}
