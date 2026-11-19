//! Verdicts that earlier acknowledged retros gave the same clarity candidates. Conversation retros
//! of different scopes share most of their census window, so the last eight scoped retros re-judged
//! 95 of their 132 required candidates, 56 of 60 to the same verdict and 4 to the opposite one
//! (conversation retro 87553225377583142572: 28 of its 30 were judged before). A candidate an
//! acknowledged retro already judged moves to `judged_before[]` with that verdict and incident, and
//! `required[]` refills with candidates no retro has judged yet.
use crate::monitor::root;
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, time::SystemTime};

pub(crate) struct Judged { verdict: String, incident: String, at: SystemTime }

/// `at` -> the latest acknowledged verdict, from every retro but `skip` that ran retro-clarity.
pub(crate) fn judged(home: &Path, skip: &str) -> HashMap<String, Judged> {
    let (analysis, incidents) = (root(home).join("analysis"), root(home).join("incidents"));
    let mut map: HashMap<String, Judged> = HashMap::new();
    for dir in std::fs::read_dir(&analysis).into_iter().flatten().flatten() {
        let id = dir.file_name().to_string_lossy().to_string();
        let Ok(acked) = std::fs::metadata(incidents.join(format!("{id}.ack"))).and_then(|m| m.modified()) else { continue };
        if id == skip || !dir.path().join("clarity.json").exists() { continue; }
        let Some(report) = std::fs::read(dir.path().join("report.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        for entry in report["instruction_clarity"]["reviewed"].as_array().into_iter().flatten() {
            let (Some(at), Some(verdict)) = (entry["at"].as_str(), entry["verdict"].as_str()) else { continue };
            if map.get(at).is_some_and(|j| j.at >= acked) { continue; }
            map.insert(at.to_string(), Judged { verdict: verdict.to_string(), incident: id.clone(), at: acked });
        }
    }
    map
}

/// Required `at`s among `candidates` (score order) with no earlier verdict, at most `max`, and the
/// judged ones as `{at, verdict, incident}`.
pub(crate) fn split(candidates: &[Value], judged: &HashMap<String, Judged>, min_score: u64, max: usize) -> (Vec<Value>, Vec<Value>) {
    let (mut required, mut before) = (Vec::new(), Vec::new());
    for at in candidates.iter().filter(|c| c["score"].as_u64() >= Some(min_score)).filter_map(|c| c["at"].as_str()) {
        match judged.get(at) {
            Some(j) => before.push(json!({"at": at, "verdict": j.verdict, "incident": j.incident})),
            None if required.len() < max => required.push(json!(at)),
            None => {}
        }
    }
    (required, before)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_candidate_judged_before_leaves_required_and_the_next_one_fills_in() {
        let candidates: Vec<Value> = [("a", 5), ("b", 3), ("c", 2), ("d", 1)].iter().map(|(at, s)| json!({"at": at, "score": s})).collect();
        let judged = HashMap::from([("a".to_string(), Judged { verdict: "agent".into(), incident: "i1".into(), at: SystemTime::UNIX_EPOCH })]);
        let (required, before) = split(&candidates, &judged, 2, 2);
        assert_eq!(required, vec![json!("b"), json!("c")], "c refills the slot a left; d scores too low");
        assert_eq!(before, vec![json!({"at": "a", "verdict": "agent", "incident": "i1"})]);
        let (required, before) = split(&candidates, &HashMap::new(), 2, 2);
        assert_eq!((required.len(), before.len()), (2, 0), "without earlier verdicts the cap still holds");
    }

    #[test]
    fn only_acknowledged_retros_with_a_clarity_receipt_count_and_the_latest_wins() {
        let home = std::env::temp_dir().join(format!("clarity-carried-{}", std::process::id()));
        let (analysis, incidents) = (root(&home).join("analysis"), root(&home).join("incidents"));
        let retro = |id: &str, verdict: &str, ack: bool| {
            std::fs::create_dir_all(analysis.join(id)).unwrap();
            std::fs::create_dir_all(&incidents).unwrap();
            std::fs::write(analysis.join(id).join("clarity.json"), "{}").unwrap();
            let report = json!({"instruction_clarity": {"reviewed": [{"at": "x:1", "verdict": verdict}]}});
            std::fs::write(analysis.join(id).join("report.json"), report.to_string()).unwrap();
            if ack { std::fs::write(incidents.join(format!("{id}.ack")), "{}").unwrap(); }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        retro("old", "instruction", true);
        retro("new", "agent", true);
        retro("open", "not_misunderstanding", false);
        retro("self", "not_misunderstanding", true);
        let map = judged(&home, "self");
        let j = map.get("x:1").unwrap();
        assert_eq!((j.verdict.as_str(), j.incident.as_str()), ("agent", "new"));
        std::fs::remove_dir_all(&home).ok();
    }
}
