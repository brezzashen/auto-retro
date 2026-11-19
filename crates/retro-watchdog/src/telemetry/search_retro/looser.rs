//! A call whose own `| grep` dropped local-search's looser-retry label. Since ~/automation
//! local-search logs `looser` on a `search` row whose hits are a looser retry's suggestions
//! (`search_cmd.rs`: the exact pattern matched nothing). Search-retro 88110673498121705918
//! (8a012622…jsonl:3601): `search "海外主站|…|国内的浏览器" --corpus report --lines | grep
//! 'https://report'` kept ten report URLs and dropped `no chunk holds the exact pattern`; the agent
//! narrowed the search (:3617, which printed the label) and took the absence (:3633, :3676), yet
//! the call counted as an unadopted hit. With the row's flag the transcript output is read as if
//! the label had reached it: the URLs are suggestions, a hit only when the agent opens one.
use super::ran::normalize;
use super::returned::{line_filtered, RELAXED};
use serde_json::Value;
use std::path::Path;

/// Engine rows are written when the search ends; transcript clocks may differ by a little.
const SLACK_MS: f64 = 5_000.0;
/// Results arrive after the window's last call; keep their rows too.
const TAIL_MS: f64 = 15.0 * 60_000.0;

/// Agent `search` rows `(ts_ms, normalized query)` near the window that printed a looser retry.
pub(super) fn rows(home: &Path, from_ms: f64, to_ms: f64) -> Vec<(f64, String)> {
    let text = std::fs::read_to_string(home.join(".local/share/local-search/queries.jsonl")).unwrap_or_default();
    text.lines()
        .filter(|l| l.contains("\"looser\""))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|r| r.get("caller").is_none() && r.get("looser").is_some())
        .filter_map(|r| Some((r["ts_ms"].as_f64()?, normalize(r["query"].as_str()?))))
        .filter(|(t, _)| *t >= from_ms - SLACK_MS && *t <= to_ms + TAIL_MS)
        .collect()
}

/// The output with the looser-retry label put back on top, when the command line-filters its
/// search, the label did not reach the transcript, and a looser row of the same query was logged
/// between the call and its result.
pub(super) fn relabel(command: &str, output: &str, rows: &[(f64, String)], call_ms: f64, result_ms: f64) -> Option<String> {
    if rows.is_empty() || !line_filtered(command) || output.lines().any(|l| l.trim_start().starts_with(RELAXED)) {
        return None;
    }
    let query = normalize(&super::quality::query_of(command)?);
    rows.iter()
        .any(|(t, q)| *t >= call_ms - SLACK_MS && *t <= result_ms + SLACK_MS && *q == query)
        .then(|| format!("{RELAXED} (label dropped by the call's line filter):\n{output}"))
}

#[cfg(test)]
mod tests {
    use super::super::returned::returned_or_used;
    use super::relabel;

    const COMMAND: &str = "~/.local/bin/local-search search \"国内直连|国内 DoH\" --corpus report --per-source 1 --limit 20 --lines 2>&1 | grep 'https://report' | head";
    const OUTPUT: &str = "  https://reports.example.com/network-incident-1794408315/  [report/section]\n  https://reports.example.com/network-incident-1794390102/  [report/section]\n";

    /// 8a012622…jsonl:3601: the filtered URLs of a looser row are suggestions the agent never opened.
    #[test]
    fn a_looser_row_turns_filtered_urls_into_suggestions() {
        assert_eq!(returned_or_used(COMMAND, OUTPUT, |_| false).len(), 2);
        let rows = vec![(1_000.0, "国内直连|国内 DoH".to_string())];
        let relabelled = relabel(COMMAND, OUTPUT, &rows, 900.0, 1_100.0).unwrap();
        assert!(returned_or_used(COMMAND, &relabelled, |_| false).is_empty(), "{relabelled}");
        // A suggestion the agent then opens is still a hit it adopted.
        assert_eq!(returned_or_used(COMMAND, &relabelled, |_| true).len(), 2);
    }

    /// Without a looser row, without a line filter, or with the label printed, nothing changes.
    #[test]
    fn exact_rows_unfiltered_calls_and_printed_labels_stay_as_they_are() {
        let rows = vec![(1_000.0, "国内直连|国内 DoH".to_string())];
        assert!(relabel(COMMAND, OUTPUT, &[(1_000.0, "other".to_string())], 900.0, 1_100.0).is_none());
        assert!(relabel(COMMAND, OUTPUT, &rows, 20_000.0, 21_000.0).is_none());
        let unfiltered = COMMAND.replace(" | grep 'https://report'", "");
        assert!(relabel(&unfiltered, OUTPUT, &rows, 900.0, 1_100.0).is_none());
        let inverted = COMMAND.replace("grep 'https", "grep -v 'https");
        assert!(relabel(&inverted, OUTPUT, &rows, 900.0, 1_100.0).is_none());
        let labelled = format!("no chunk holds the exact pattern; chunks naming DoH:\n{OUTPUT}");
        assert!(relabel(COMMAND, &labelled, &rows, 900.0, 1_100.0).is_none());
    }
}
