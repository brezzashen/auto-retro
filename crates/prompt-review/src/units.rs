//! Text units of the Mars CRM main agent's prompt surfaces: one rule-sized piece of text each.
//!
//! Surfaces: `system` (system instruction, by Markdown heading), `tool` (each tool's description and
//! every schema field description), `catalog` (one skill's entry in the `<available_skills>` block of
//! the session exchange), `session` (other host-authored session text) and `skill` (bundled skill
//! bodies and references, read from source; loaded on demand, so not in the standing prefix).
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Shorter pieces are labels or enum glosses, not rules.
pub const MIN_CHARS: usize = 20;
/// Longer lines are split into sentences so one unit carries one rule.
const SPLIT_CHARS: usize = 280;
/// Opening exchange fillers and the measured user turn.
const FILLERS: [&str; 4] = ["hi", "Understood.", "Noted.", "ok"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unit {
    pub surface: String,
    /// Where the unit sits: `system:<heading>`, `tool:<name>`, `catalog:<skill>`, `skill:<file>`.
    pub place: String,
    /// JSON path inside a tool, or the line number of a skill source.
    pub path: String,
    pub text: String,
    /// In the request every main-agent call sends.
    pub standing: bool,
    /// `file:line` copies in source, filled for units that end up in a duplicate group.
    #[serde(default)]
    pub src: Vec<String>,
}

fn strip_marker(line: &str) -> &str {
    let t = line.trim().trim_start_matches(['-', '*', '•', '>', '|']).trim();
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && t[digits..].starts_with(". ") { t[digits + 2..].trim() } else { t }
}

/// Sentences of a long line; Chinese and English terminators, terminator kept.
fn sentences(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = line.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        cur.push(*c);
        let end = matches!(c, '。' | '！' | '？' | '；') || (matches!(c, '.' | '!' | '?' | ';') && chars.get(i + 1).is_none_or(|n| *n == ' '));
        if end {
            out.push(std::mem::take(&mut cur).trim().to_string());
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Rule-sized pieces of a block of text, with the 0-based line each came from.
pub fn pieces(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = strip_marker(raw);
        if line.starts_with("constraints:") || line.chars().all(|c| matches!(c, '-' | '|' | ':' | ' ')) {
            continue;
        }
        let parts = if line.chars().count() > SPLIT_CHARS { sentences(line) } else { vec![line.to_string()] };
        out.extend(parts.into_iter().filter(|p| p.chars().count() >= MIN_CHARS).map(|p| (n, p)));
    }
    out
}

fn push(out: &mut Vec<Unit>, surface: &str, place: String, path: String, text: &str, standing: bool) {
    for (_, piece) in pieces(text) {
        if !out.iter().any(|u| u.place == place && u.text == piece) {
            out.push(Unit { surface: surface.into(), place: place.clone(), path: path.clone(), text: piece, standing, src: vec![] });
        }
    }
}

fn system(out: &mut Vec<Unit>, text: &str, part: usize) {
    let mut heading = format!("part{part}");
    let mut block = String::new();
    let flush = |out: &mut Vec<Unit>, heading: &str, block: &mut String| {
        push(out, "system", format!("system:{heading}"), String::new(), block, true);
        block.clear();
    };
    for line in text.lines() {
        if let Some(h) = line.trim_start().strip_prefix('#') {
            flush(out, &heading, &mut block);
            heading = h.trim_start_matches('#').trim().chars().take(60).collect();
        } else {
            block.push_str(line);
            block.push('\n');
        }
    }
    flush(out, &heading, &mut block);
}

fn schema(out: &mut Vec<Unit>, tool: &str, value: &Value, path: &str) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "description" {
                    if let Some(text) = v.as_str() {
                        push(out, "tool", format!("tool:{tool}"), path.to_string(), text, true);
                    }
                } else {
                    schema(out, tool, v, &format!("{path}.{k}"));
                }
            }
        }
        Value::Array(items) => items.iter().enumerate().for_each(|(i, v)| schema(out, tool, v, &format!("{path}[{i}]"))),
        _ => {}
    }
}

fn session(out: &mut Vec<Unit>, text: &str, index: usize) {
    let catalog = regex::Regex::new(r"(?s)<skill>\s*<name>(.*?)</name>\s*<description>(.*?)</description>").expect("regex");
    for cap in catalog.captures_iter(text) {
        push(out, "catalog", format!("catalog:{}", cap[1].trim()), String::new(), &cap[2], true);
    }
    let rest = regex::Regex::new(r"(?s)<available_skills>.*?</available_skills>").expect("regex").replace_all(text, "");
    push(out, "session", format!("session:{index}"), String::new(), &rest, true);
}

/// Units of one captured Gemini request body (`measure-prefix.ts --body`).
pub fn from_body(body: &Value) -> Vec<Unit> {
    let mut out = Vec::new();
    for (i, part) in body["systemInstruction"]["parts"].as_array().into_iter().flatten().enumerate() {
        system(&mut out, part["text"].as_str().unwrap_or_default(), i);
    }
    for decl in body["tools"].as_array().into_iter().flatten().flat_map(|t| t["functionDeclarations"].as_array().into_iter().flatten()) {
        let name = decl["name"].as_str().unwrap_or("?");
        push(&mut out, "tool", format!("tool:{name}"), "description".into(), decl["description"].as_str().unwrap_or_default(), true);
        schema(&mut out, name, &decl["parameters"], "parameters");
    }
    for (i, content) in body["contents"].as_array().into_iter().flatten().enumerate() {
        for part in content["parts"].as_array().into_iter().flatten() {
            let text = part["text"].as_str().unwrap_or_default();
            if content["role"] == "user" && !FILLERS.contains(&text.trim()) {
                session(&mut out, text, i);
            }
        }
    }
    out
}

/// Units of a bundled skill source file: every template literal of SKILL_LITERAL chars or more.
pub fn from_skill_source(rel: &str, source: &str) -> Vec<Unit> {
    let stem = rel.rsplit('/').next().unwrap_or(rel).split('.').next().unwrap_or(rel).to_string();
    let mut out = Vec::new();
    for (start_line, literal) in crate::literals::templates(source) {
        for (n, piece) in pieces(&literal) {
            let line = start_line + n;
            if !out.iter().any(|u: &Unit| u.text == piece) {
                out.push(Unit { surface: "skill".into(), place: format!("skill:{stem}"), path: line.to_string(), text: piece, standing: false, src: vec![format!("{rel}:{line}")] });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body_units_cover_every_surface() {
        let body = json!({
            "systemInstruction": {"parts": [{"text": "# Delete\n- Deleting a person cannot be undone; always confirm first.\n## Short\nok"}]},
            "tools": [{"functionDeclarations": [{"name": "delete-resource", "description": "Deletes one resource. Always ask the user to confirm before deleting.",
                "parameters": {"properties": {"path": {"description": "constraints: input must match this regex ^/.*\nThe resource path to delete from the CRM."}}}}]}],
            "contents": [{"role": "user", "parts": [{"text": "<available_skills>\n<skill>\n<name>photos</name>\n<description>Use for questions about the user's own iPhone photo library.</description>\n</skill>\n</available_skills>"}]},
                {"role": "user", "parts": [{"text": "hi"}]}]
        });
        let units = from_body(&body);
        let places: Vec<&str> = units.iter().map(|u| u.place.as_str()).collect();
        assert_eq!(places, ["system:Delete", "tool:delete-resource", "tool:delete-resource", "catalog:photos"]);
        assert!(units.iter().all(|u| !u.text.starts_with("constraints:")));
    }

    #[test]
    fn long_lines_split_into_sentences() {
        let line = "第一条规则写在这里，它足够长可以单独成为一个单元。".repeat(12) + " Second rule is written in English here. Third one ends here.";
        assert!(pieces(&line).len() > 2);
    }
}
