//! Audit contract of a memory dream (incident `contract: 2`; founder, 2026-11-10): on top of the
//! measurements `validate.rs` checks, the dream read the sessions `dream-sessions` picked and
//! answered, per group, what the session lacked, where it got stuck, what repeated and what the
//! instructions do not say. Each answer cites transcript lines; each lesson is a note in the store
//! with the real request that should recall it, measured by `agent-memory eval` on those requests
//! (`eval-lessons.json`), so a lesson nobody can recall does not pass as learned.
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;

const ANSWERS: [&str; 4] = ["missing_info", "friction", "repetition", "instruction_gap"];
/// A `best:`, `fast:` or `praise:` group is read for what went well, not for what went wrong.
const PRAISE_ANSWERS: [&str; 2] = ["what_worked", "reusable"];
const KINDS: [&str; 5] = ["calibration", "gotcha", "gap", "instruction", "outlier"];
/// Verdicts of a `lesson:` follow-up group.
const FOLLOWUP: [&str; 5] = ["helped", "ignored", "wrong", "obsolete", "unclear"];
/// The lesson fails as written (sessions pass it by, it misleads, or the script, skill or prompt now
/// handles the case): by the ack it is rewritten or retired, in the same dream.
const FAILING: [&str; 3] = ["ignored", "wrong", "obsolete"];

fn body(text: &str) -> &str {
    text.strip_prefix("---\n").and_then(|t| t.split_once("\n---")).map_or(text, |(_, body)| body).trim()
}

/// The note is gone, or its body differs from the one the dream started with (store commit `head`).
/// A `verify` stamp changes only the frontmatter and is not an action.
fn acted_on(store: &Path, head: &str, note: &str) -> bool {
    let Ok(now) = std::fs::read_to_string(store.join(format!("{note}.md"))) else { return true };
    match std::process::Command::new("git").arg("-C").arg(store).args(["show", &format!("{head}:{note}.md")]).output() {
        Ok(then) if then.status.success() => body(&String::from_utf8_lossy(&then.stdout)) != body(&now),
        _ => true,
    }
}

fn text(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.trim().is_empty())
}

fn cited(home: &Path, v: &Value) -> bool {
    v.as_array().is_some_and(|e| !e.is_empty() && e.iter().all(|s| s.as_str().is_some_and(|s| crate::retro_recall::cited_line_exists(home, s))))
}

fn check_audit(home: &Path, store: &Path, head: &str, group: &str, entry: &Value, lessons: &[&str]) -> Result<()> {
    ensure!(cited(home, &entry["evidence"]), "audits {group}: evidence needs transcript path:line that exist");
    if let Some(note) = group.strip_prefix("lesson:") {
        let verdict = entry["verdict"].as_str().unwrap_or_default();
        ensure!(FOLLOWUP.contains(&verdict), "audits {group}: verdict must be one of {FOLLOWUP:?}");
        ensure!(text(&entry["why"]), "audits {group}: why (what the sessions did with the lesson) is empty");
        ensure!(!note.starts_with("memo-") || text(&entry["entry"]), "audits {group}: a memo holds several lessons; entry must name the one this verdict is about");
        if FAILING.contains(&verdict) {
            ensure!(text(&entry["next"]), "audits {group}: {verdict} needs next (what was rewritten, moved or retired)");
            ensure!(acted_on(store, head, note), "audits {group}: judged {verdict} but the note is unchanged since the dream began: rewrite it action first, remove the entry, or agent-memory forget it (a verify stamp is not a change)");
        }
        ensure!(verdict != "helped" || text(&entry["promoted"]), "audits {group}: helped needs promoted (where the lesson now reaches more sessions: top of memo-<skill>, a skill line, a script, or why it stays as it is)");
        return Ok(());
    }
    for key in if ["praise:", "best:", "fast:"].iter().any(|k| group.starts_with(k)) { &PRAISE_ANSWERS[..] } else { &ANSWERS[..] } {
        ensure!(text(&entry["answers"][key]), "audits {group}: answers.{key} is empty (say what the transcripts show, or \"none\" with why)");
    }
    match entry["verdict"].as_str() {
        Some("lesson") => ensure!(entry["lessons"].as_array().is_some_and(|l| !l.is_empty() && l.iter().all(|n| n.as_str().is_some_and(|n| lessons.contains(&n)))),
            "audits {group}: verdict lesson needs lessons[] naming report.json lessons[].note"),
        Some("no_lesson") => ensure!(text(&entry["why"]), "audits {group}: no_lesson needs why"),
        _ => anyhow::bail!("audits {group}: verdict must be lesson or no_lesson"),
    }
    Ok(())
}

fn check_lessons(home: &Path, dir: &Path, store: &Path, report: &Value, lessons: &[Value]) -> Result<()> {
    for (i, lesson) in lessons.iter().enumerate() {
        let note = lesson["note"].as_str().unwrap_or_default();
        ensure!(!note.is_empty() && store.join(format!("{note}.md")).is_file(), "lessons[{i}].note {note:?} is not a note in the store");
        ensure!(lesson["kind"].as_str().is_some_and(|k| KINDS.contains(&k)), "lessons[{i}].kind must be one of {KINDS:?}");
        ensure!(text(&lesson["query"]), "lessons[{i}].query (the real request or step that should recall it) is empty");
        ensure!(cited(home, &lesson["evidence"]), "lessons[{i}].evidence needs transcript path:line that exist");
    }
    ensure!(report["actions"]["lessons_written"].as_u64() == Some(lessons.len() as u64), "report.json actions.lessons_written must equal lessons[] ({})", lessons.len());
    if lessons.is_empty() { return Ok(()); }
    let eval: Value = serde_json::from_slice(&std::fs::read(dir.join("eval-lessons.json"))
        .context("eval-lessons.json missing: agent-memory eval --set analysis/<id>/lessons.jsonl > analysis/<id>/eval-lessons.json")?)?;
    let dream = &eval["sets"]["dream"];
    ensure!(dream["cases"].as_u64() >= Some(lessons.len() as u64), "eval-lessons.json has {} dream cases for {} lessons", dream["cases"], lessons.len());
    ensure!(report["lesson_eval"]["top5_pct"].as_f64() == dream["top5_pct"].as_f64() && report["lesson_eval"]["hook_injected_pct"].as_f64() == dream["hook_injected_pct"].as_f64(),
        "report.json lesson_eval must copy eval-lessons.json sets.dream top5_pct and hook_injected_pct");
    ensure!(dream["top5_pct"].as_f64() >= Some(100.0) || report["lesson_miss_reason"].as_str().is_some_and(|r| r.trim().len() >= 20),
        "a lesson is not recalled by its own request (eval-lessons.json top5 {}): reword the note or give lesson_miss_reason", dream["top5_pct"]);
    Ok(())
}

/// The audit part of the gate. `before`/`after` are the eval files: lessons of earlier dreams (set
/// `dream` of `golden-memory-dream.jsonl`) may not drop out of recall.
pub(super) fn validate(home: &Path, id: &str, dir: &Path, (store, head): (&Path, &str), report: &Value, before: &Value, after: &Value) -> Result<()> {
    let receipt: Value = serde_json::from_slice(&std::fs::read(dir.join("dream-sessions.json"))
        .context("run cpu-watchdog dream-sessions <id> first: analysis/<id>/dream-sessions.json is missing")?)?;
    ensure!(receipt["incident"] == id, "dream-sessions.json belongs to another incident");
    let lessons = report["lessons"].as_array().context("report.json lessons must be an array (empty when no session taught anything)")?;
    let names: Vec<&str> = lessons.iter().filter_map(|l| l["note"].as_str()).collect();
    let audits = report["audits"].as_array().context("report.json audits must be an array")?;
    for group in receipt["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let entry = audits.iter().find(|a| a["group"] == group).with_context(|| format!("report.json audits[] has no entry for {group}"))?;
        check_audit(home, store, head, group, entry, &names)?;
    }
    // A retired lesson leaves the golden set's percentage (eval counts it as `gone`), so it is listed.
    let gone = |eval: &Value| eval["sets"]["dream"]["gone"].as_u64().unwrap_or(0);
    let retired = report["retired"].as_array().cloned().unwrap_or_default();
    ensure!(gone(after) <= gone(before) + retired.len() as u64, "{} lesson(s) left the store during the dream but report.json retired[] lists {}", gone(after) - gone(before), retired.len());
    for (i, row) in retired.iter().enumerate() {
        ensure!(["note", "reason", "verdict"].iter().all(|k| text(&row[*k])), "retired[{i}] needs note, reason and the verdict it rests on");
    }
    check_lessons(home, dir, store, report, lessons)?;
    if let Some(b) = before["sets"]["dream"]["top5_pct"].as_f64() {
        let a = after["sets"]["dream"]["top5_pct"].as_f64().unwrap_or(0.0);
        ensure!(a >= b || report["regression_reason"].as_str().is_some_and(|r| r.trim().len() >= 20),
            "earlier dream lessons dropped out of recall: dream.top5_pct {b} -> {a}; restore the note or give regression_reason");
    }
    Ok(())
}
