//! `--text` view of the census: one line per signature row or per drilled call. Retros had piped
//! `--show` JSON (the whole census plus 600-character calls) through their own jq projection: 79 of
//! 98 `--show` calls in 25 sessions over 2026-11-08..15, often twice for one signature after the
//! first output was persisted as too large (conversation retro 817759638105279248955).
use serde_json::Value;

const HEAD: usize = 160;

/// First `HEAD` characters of `text` with every whitespace run folded into one space.
fn head(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(HEAD).collect();
    if flat.chars().count() > HEAD { out.push('…'); }
    out
}

fn counts(row: &Value) -> String {
    let n = |k: &str| row[k].as_u64().unwrap_or(0);
    format!("{} calls in {} sessions, {} extra, {} stuck, {} failed, {} duplicate, {} retry",
        n("calls"), n("sessions"), n("extra_calls"), n("stuck_sessions"), n("failed_calls"), n("duplicate_calls"), n("retry_calls"))
}

/// Plain lines for a census or receipt `value`: with a drill-down, the signature's counts then each
/// call as `path:line [label] call => result`; otherwise `required` and one line per ranked row.
pub(super) fn render(value: &Value) -> String {
    let rows = value["top"].as_array().cloned().unwrap_or_default();
    let mut lines = Vec::new();
    if let Some(sig) = value["shown"]["signature"].as_str() {
        let row = rows.iter().find(|r| r["signature"] == sig);
        lines.push(format!("{sig}: {}", row.map(counts).unwrap_or_else(|| "not in the ranked rows (below the floor)".into())));
        for call in value["shown"]["calls"].as_array().into_iter().flatten() {
            let field = |k: &str| call[k].as_str().unwrap_or_default();
            lines.push(format!("{} [{}] {} => {}", field("at"), field("label"), head(field("call")), head(field("result"))));
        }
        return lines.join("\n");
    }
    lines.push(format!("{} calls in {} sessions", value["calls"], value["sessions"]));
    if let Some(required) = value["required"].as_array() {
        lines.push(format!("required: {}", required.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" | ")));
    }
    lines.extend(rows.iter().map(|r| format!("{}: {}", r["signature"].as_str().unwrap_or_default(), counts(r))));
    lines.join("\n")
}
