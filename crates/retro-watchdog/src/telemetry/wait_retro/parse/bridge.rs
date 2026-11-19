//! Waits of Antigravity sessions that Orca bridges into Claude transcripts
//! (`~/.claude/projects/-Users-<user>-antigravity-orca-bridge`).
//!
//! A bridged assistant line carries the planner step's `created_at`, which is when the model began
//! the step, and each bridged result line carries the tool's start. The span between them is model
//! time, not a tool wait: wait retro 83168070974659841531 charged 106.3 min to one Gemini session
//! whose tools ran 10.0 min by their own stamps. Each bridged result starts with
//! `Created At: <rfc3339>` and, once the tool returned, `Completed At: <rfc3339>`; a step is charged
//! the envelope from its earliest start to its latest end, across the one-result-per-line records,
//! when the next model step or user turn arrives. Results without both stamps (the older bridge
//! format, background tasks) are not charged.
use super::{Cursor, MIN_MS, CAP_MS};
use super::super::super::runtime_retro::parse::timestamp_ms;
use serde_json::Value;

pub(super) const SLUG: &str = "antigravity-orca-bridge";

/// `2026-11-11T01:18:48+08:00` or `…Z` as epoch ms.
pub(super) fn rfc3339_ms(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.ends_with('Z') { return timestamp_ms(text); }
    let (base, sign, offset) = match text.len().checked_sub(6).and_then(|i| text.get(i..).map(|o| (i, o))) {
        Some((i, o)) if o.starts_with('+') || o.starts_with('-') => (&text[..i], if o.starts_with('+') { 1.0 } else { -1.0 }, &o[1..]),
        _ => return None,
    };
    let (h, m) = offset.split_once(':')?;
    let minutes = h.parse::<f64>().ok()? * 60.0 + m.parse::<f64>().ok()?;
    Some(timestamp_ms(&format!("{base}Z"))? - sign * minutes * 60_000.0)
}

/// The tool's own start and end, read from the head of a bridged result.
pub(super) fn stamps(block: &Value) -> Option<(f64, f64)> {
    let content = &block["content"];
    let text = content.as_str().map(str::to_string).or_else(|| {
        content.as_array().map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"))
    })?;
    let mut lines = text.lines();
    let created = rfc3339_ms(lines.next()?.strip_prefix("Created At: ")?)?;
    let completed = rfc3339_ms(lines.next()?.strip_prefix("Completed At: ")?)?;
    (completed >= created).then_some((created, completed))
}

impl Cursor {
    pub(super) fn feed_bridge(&mut self, line: &Value, at: f64) {
        let blocks = line["message"]["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        let results: Vec<&Value> = blocks.iter().filter(|b| b["type"] == "tool_result").collect();
        if line["type"] == "user" && !results.is_empty() {
            for (from, to) in results.into_iter().filter_map(stamps) {
                self.stamp_from = if self.stamp_from > 0.0 { self.stamp_from.min(from) } else { from };
                self.stamp_to = self.stamp_to.max(to);
            }
            return;
        }
        if line["type"] != "user" && line["type"] != "assistant" { return; }
        self.flush_bridge();
        if line["type"] == "assistant" {
            let calls = blocks.iter().filter(|b| b["type"] == "tool_use")
                .map(|b| super::Call::new(super::claude_label(b), b["input"]["command"].as_str().unwrap_or(""), b["id"].as_str().unwrap_or(""))).collect();
            self.output(at, calls);
        }
    }

    /// Charges the finished step's stamped envelope and clears it.
    fn flush_bridge(&mut self) {
        let (from, to) = (self.stamp_from, self.stamp_to);
        (self.stamp_from, self.stamp_to) = (0.0, 0.0);
        if from > 0.0 && to >= from {
            let span = to - from;
            if let Some(call) = self.waiting_call() {
                if (MIN_MS..CAP_MS).contains(&span) && to >= self.since_ms { self.charge(to, span, call); }
            }
        }
        self.last_output = 0.0;
        self.calls.clear();
    }
}
