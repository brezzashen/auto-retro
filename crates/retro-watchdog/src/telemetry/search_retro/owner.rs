//! The program that owns a snake name is one piece, not the name's leading words (search-retro
//! 813280046605134040490).
//!
//! `answered::accepted` treats a later search holding two pieces of a name as a search for that name
//! again. A kebab-case program token whose words open a snake name broke that rule: after
//! `search "CPU_BUILD_GATE_PRIORITY"` printed every setter of the variable, the agent asked
//! `query "who launches cpu-build-gate tool-prebuild after a push to origin/mastra"`
//! (29474b26…jsonl:277, then :340), a question about the program. `cpu-build-gate` gave `gate`
//! and `tool-prebuild` gave `build` (pieces match inside words), two pieces of the variable, and the
//! answered hit read as unadopted. The owning program now counts as a single piece and stands for
//! its words: `cpu-build-gate priority`, or `BUILD_GATE` in the variable's own snake form, still
//! re-searches it.

/// `text` (lowercased) with each kebab-case token whose words are a proper leading run of the snake
/// `whole` (lowercased) blanked, and the words those tokens cover (empty when none was blanked).
pub(super) fn without_owners(text: &str, whole: Option<&str>) -> (String, Vec<String>) {
    let Some(whole) = whole.filter(|w| w.contains('_')) else { return (text.to_string(), Vec::new()) };
    let words: Vec<&str> = whole.split('_').collect();
    let (mut out, mut owned) = (text.to_string(), Vec::new());
    for token in text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_') {
        let own: Vec<&str> = token.split('-').collect();
        if token.contains('_') || own.len() < 2 || own.len() >= words.len() || own.iter().any(|w| w.is_empty()) {
            continue;
        }
        if words.starts_with(&own) {
            out = out.replace(token, " ");
            for w in own {
                if !owned.iter().any(|o| o == w) {
                    owned.push(w.to_string());
                }
            }
        }
    }
    (out, owned)
}

#[cfg(test)]
mod tests {
    use super::super::answered::accepted;
    use super::super::steps::{Kind, Step};
    use super::*;

    fn shell(text: &str) -> Step {
        Step { at_ms: 1.0, id: "1".into(), kind: Kind::Shell, text: text.into(), line: 1, turn: "1".into() }
    }

    #[test]
    fn a_leading_kebab_program_is_blanked() {
        let (text, owned) = without_owners("who launches cpu-build-gate tool-prebuild", Some("cpu_build_gate_priority"));
        assert_eq!(owned, ["cpu", "build", "gate"]);
        assert_eq!(text, "who launches   tool-prebuild");
        assert!(without_owners("cpu-build-gate-priority", Some("cpu_build_gate_priority")).1.is_empty());
        assert!(without_owners("build-gate", Some("cpu_build_gate_priority")).1.is_empty());
        assert!(without_owners("cpu-build-gate", Some("cpubuildgate")).1.is_empty());
    }

    #[test]
    fn asking_about_the_owning_program_keeps_the_answer() {
        let names = vec!["CPU_BUILD_GATE_PRIORITY".to_string()];
        // 29474b26…jsonl:277 then :340.
        let later = "~/.local/bin/local-search query \"who launches cpu-build-gate tool-prebuild after a push to origin/mastra\" --corpus code 2>&1 | head -25";
        assert!(accepted(&[], &[shell(later)], 0.0, &names, false, 8, 9e5));
    }

    #[test]
    fn the_variable_asked_again_still_refuses_the_answer() {
        let names = vec!["CPU_BUILD_GATE_PRIORITY".to_string()];
        for later in ["local-search query \"cpu-build-gate priority env\" --corpus code", "rg -n 'BUILD_GATE' src", "rg -n 'CPU_BUILD_GATE_PRIORITY' src"] {
            assert!(!accepted(&[], &[shell(later)], 0.0, &names, false, 8, 9e5), "{later}");
        }
    }
}
