//! Whether a local-search call written as the `||` fallback of a shell line ran at all.
//! Search-retro 816932372930225207160 (85e097d8…jsonl:296): `ls dir 2>/dev/null || local-search
//! search "FAILURES" …` printed the listing, so the search never ran and `queries.jsonl` holds no
//! row for it; it was still counted as a no-hit call, and the grep that followed made it the
//! window's only false NONE. Only two shapes are checked: a call after `||`, and (since 2026-11-05)
//! a call whose result is a PreToolUse hook block, which refuses the whole line before any segment
//! runs (search-retro 82791454137862182601, d4b75593…jsonl:534: script-guard refused a line holding
//! `local-search search "api/me/session"`; no engine row exists for it, yet it was the window's only
//! false NONE and one fallback). Since 2026-11-18 a third shape: the result carries an argument
//! error line (`Error: …` from anyhow, `error: …` from clap), so local-search refused its arguments
//! before searching (search-retro 811000023466649112492, agent-a23f3765…jsonl:167: `--path .skills`
//! printed `Error: --path .skills: no such file or directory …`, and the `grep` after the step's
//! `echo` marker printed a memory note line that the meter read as an unadopted hit). Such a call
//! whose query has no agent engine row between its call and its result did not run and stays
//! outside every rate (`not_run_calls`). Every other call counts as before, and a window without
//! engine rows checks nothing.
use super::quality::query_of;
use serde_json::Value;
use std::path::Path;

/// Engine rows are written when the search ends; transcript clocks may differ by a little.
const SLACK_MS: f64 = 5_000.0;
/// Results arrive after the window's last call; keep their rows too.
const TAIL_MS: f64 = 15.0 * 60_000.0;

/// Agent engine rows `(ts_ms, normalized query)` near the window; watchdog receipts carry `caller`.
pub(super) fn rows(home: &Path, from_ms: f64, to_ms: f64) -> Vec<(f64, String)> {
    let text = std::fs::read_to_string(home.join(".local/share/local-search/queries.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|r| r.get("caller").is_none())
        .filter_map(|r| Some((r["ts_ms"].as_f64()?, normalize(r["query"].as_str()?))))
        .filter(|(t, _)| *t >= from_ms - SLACK_MS && *t <= to_ms + TAIL_MS)
        .collect()
}

/// True when the command's local-search runs only after `||`, or a PreToolUse hook refused the
/// line, and no engine row of its query falls between the call and its result. A row logged after
/// the next tool call started belongs to that call: search-retro 814194729051765393311
/// (884cd1bf…jsonl:130) had its refused `branchThread|cloneThread` line counted as run because the
/// retry at :142, 2.8 s later, logged the same query inside the slack.
pub(super) fn skipped(command: &str, result: &str, rows: &[(f64, String)], call_ms: f64, result_ms: f64, next_ms: Option<f64>) -> bool {
    if rows.is_empty() || !(after_or(command) || hook_blocked(result) || refused_args(result)) { return false; }
    let Some(query) = query_of(command) else { return false };
    let query = normalize(&query);
    let until = next_ms.map_or(result_ms + SLACK_MS, |n| n.min(result_ms + SLACK_MS));
    !rows.iter().any(|(t, q)| *t >= call_ms - SLACK_MS && *t <= until && *q == query)
}

/// The first local-search search|query segment is the right operand of `||`.
fn after_or(command: &str) -> bool {
    commands_with_ops(command).iter().find(|(_, segment)| query_of(segment).is_some()).is_some_and(|(op, _)| op == "||")
}

/// Claude Code reports a refused command as `PreToolUse:<Tool> hook error: …` and runs nothing.
pub(super) fn hook_blocked(result: &str) -> bool {
    let head = result.trim_start();
    head.starts_with("PreToolUse:") && head.lines().next().is_some_and(|l| l.contains("hook error"))
}

/// A flush-left argument error line: local-search's anyhow `Error: …` or clap's `error: …`. Only
/// a hint; `skipped` still requires that no engine row of the query was logged.
fn refused_args(result: &str) -> bool {
    result.lines().any(|l| l.starts_with("Error: ") || l.starts_with("error: "))
}

/// Shell quoting differs between the transcript text and the logged argv: drop backslashes and
/// collapse whitespace before comparing.
pub(super) fn normalize(query: &str) -> String {
    query.replace('\\', "").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Simple commands of a shell line with the operator before each: "" first, then `;`, `|`, `||`,
/// `&`, `&&`, a newline or a parenthesis, outside quotes.
fn commands_with_ops(command: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = command.chars().collect();
    let (mut out, mut current, mut quote, mut op, mut i) = (Vec::new(), String::new(), None::<char>, String::new(), 0);
    while i < chars.len() {
        let c = chars[i];
        match (quote, c) {
            (Some(q), _) if c == q => { quote = None; current.push(c); }
            (Some(_), _) => current.push(c),
            (None, '"' | '\'') => { quote = Some(c); current.push(c); }
            (None, ';' | '|' | '&' | '\n' | '(' | ')') => {
                if !current.trim().is_empty() { out.push((std::mem::take(&mut op), std::mem::take(&mut current))); }
                current.clear();
                let double = matches!(c, '|' | '&') && chars.get(i + 1) == Some(&c);
                op = if double { i += 1; format!("{c}{c}") } else { c.to_string() };
            }
            (None, _) => current.push(c),
        }
        i += 1;
    }
    if !current.trim().is_empty() { out.push((op, current)); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fallback_search_without_an_engine_row_did_not_run() {
        // 85e097d8…jsonl:296: `ls` succeeded, so the search after `||` never ran.
        let cmd = r#"cd ~/automation; ls crates/x/ 2>/dev/null || ~/.local/bin/local-search search "FAILURES" --corpus code --lines; git log --oneline -3"#;
        let rows = vec![(1_000.0, "builds upload".to_string())];
        assert!(skipped(cmd, "", &rows, 10_000.0, 11_000.0, None));
        // The same line when `ls` failed: the engine logged the search, so it counts.
        assert!(!skipped(cmd, "", &[(10_400.0, "FAILURES".to_string())], 10_000.0, 11_000.0, None));
        // Not a `||` fallback, or no engine rows at all: nothing changes.
        assert!(!skipped(r#"cd /r && local-search search "FAILURES" --lines"#, "", &rows, 10_000.0, 11_000.0, None));
        assert!(!skipped(cmd, "", &[], 10_000.0, 11_000.0, None));
        // `local-search … || echo none` runs the search first.
        assert!(!skipped(r#"local-search search "FAILURES" || echo none"#, "", &rows, 10_000.0, 11_000.0, None));
    }

    #[test]
    fn a_line_refused_by_a_pretooluse_hook_did_not_run() {
        // d4b75593…jsonl:534: script-guard refused the line for its trailing grep sweep.
        let cmd = r#"cd Mars_CRM && grep -n "x" a.rs | head; ~/.local/bin/local-search search "api/me/session" --corpus code --lines --path Mars_CRM/ios 2>&1 | head; grep -n "beforeSignOut" -r ios/MarsCRM"#;
        let blocked = "PreToolUse:Bash hook error: [\"$HOME/.local/bin/script-guard\"]: Blocked by script-guard: this grep sweeps an indexed code tree.";
        let rows = vec![(1_000.0, "builds upload".to_string())];
        assert!(skipped(cmd, blocked, &rows, 10_000.0, 10_040.0, None));
        // A hook error with an engine row of the query (the line ran after all) still counts.
        assert!(!skipped(cmd, blocked, &[(10_020.0, "api/me/session".to_string())], 10_000.0, 10_040.0, None));
        // Output that merely quotes a hook message later on is no block.
        assert!(!skipped(cmd, &format!("routes.rs:1: x\n{blocked}"), &rows, 10_000.0, 10_040.0, None));
        // 884cd1bf…jsonl:130 → :142: the retry 2.8 s later logged the same query; that row is the retry's.
        let retry = vec![(13_000.0, "api/me/session".to_string())];
        assert!(skipped(cmd, blocked, &retry, 10_000.0, 10_040.0, Some(12_840.0)));
        assert!(!skipped(cmd, blocked, &retry, 10_000.0, 10_040.0, None));
    }

    #[test]
    fn a_search_that_refused_its_arguments_did_not_run() {
        // agent-a23f3765…jsonl:167: a wrong `--path`, then a grep of two notes after an echo marker.
        let cmd = r#"~/.local/bin/local-search search "a|b c" --corpus code --lines --path .nodir 2>&1 | head -c 3000; echo '====='; grep -n 'a' ~/notes/x.md"#;
        let out = "Error: --path .nodir: no such file or directory under /r, any parent directory or ~\n=====\n/Users/u/notes/x.md:29:a line";
        let rows = vec![(1_000.0, "other".to_string())];
        assert!(skipped(cmd, out, &rows, 10_000.0, 10_300.0, None));
        // The retry 2 s later logged the same query; that row is the retry's, not this call's.
        assert!(skipped(cmd, out, &[(12_200.0, "a|b c".to_string())], 10_000.0, 10_300.0, Some(12_000.0)));
        // An engine row of the query inside the call: it ran, whatever else printed `Error:`.
        assert!(!skipped(cmd, out, &[(10_250.0, "a|b c".to_string())], 10_000.0, 10_300.0, None));
        // An indented or mid-line `Error:` is a hit's text, not an argument error.
        assert!(!skipped(cmd, "src/a.rs:3:    Error: bad\n  Error: x", &rows, 10_000.0, 10_300.0, None));
    }

    #[test]
    fn queries_compare_without_shell_escapes() {
        assert_eq!(normalize(r#"a\"b  c"#), r#"a"b c"#);
    }
}
