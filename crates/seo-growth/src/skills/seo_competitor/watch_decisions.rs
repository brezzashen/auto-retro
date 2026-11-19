//! Dated founder decisions the watchdog brings back on their day: `<state>/decisions.jsonl`, one
//! `{"key","dueOn","title","action"}` per line (`dueOn` a Singapore date). From `dueOn` on, each open
//! line is a P1 `decision:<key>` finding; the tab that carries it out adds `"done": true` to the line.

use super::watch_analyze::Finding;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Decision {
    key: String,
    due_on: String,
    title: String,
    action: String,
    #[serde(default)]
    done: bool,
}

fn due(text: &str, today: &str) -> Vec<Finding> {
    text.lines().filter_map(|line| serde_json::from_str::<Decision>(line).ok())
        .filter(|d| !d.done && d.due_on.as_str() <= today)
        .map(|d| Finding { key: format!("decision:{}", d.key), severity: "P1".into(), title: format!("{}（到期 {}）", d.title, d.due_on), action: d.action })
        .collect()
}

/// Open decisions due on or before `today` (`YYYY-MM-DD`).
pub fn findings(state_dir: &str, today: &str) -> Vec<Finding> {
    due(&std::fs::read_to_string(format!("{state_dir}/decisions.jsonl")).unwrap_or_default(), today)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_is_raised_from_its_day_until_it_is_done() {
        let text = concat!(
            r#"{"key":"coach-title","dueOn":"2026-11-16","title":"T","action":"A"}"#, "\n",
            r#"{"key":"closed","dueOn":"2026-11-10","title":"T","action":"A","done":true}"#, "\n",
            "not json\n",
        );
        assert!(due(text, "2026-11-15").is_empty());
        let got = due(text, "2026-11-16");
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].key.as_str(), got[0].severity.as_str(), got[0].title.as_str()), ("decision:coach-title", "P1", "T（到期 2026-11-16）"));
        assert_eq!(due(text, "2026-11-20").len(), 1);
    }
}
