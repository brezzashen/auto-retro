//! A hit on a skill's `SKILL.md` is adopted when a Claude Skill tool call loads that skill: the
//! tool's input names the skill (`{"skill":"swipe-arbitration"}`), never the file, and the harness
//! then injects that SKILL.md whole. Search-retro 813866010669096751911: 1d859b8b…jsonl:1207
//! (`local-search query … --corpus docs`) returned `swipe-arbitration/SKILL.md` first, the next turn
//! (:1209) loaded it with `Skill {"skill":"swipe-arbitration"}`, and the hit counted unadopted.
use serde_json::Value;

/// True when `text` is a Skill tool input that loads the skill whose `SKILL.md` is `file` (its last
/// two path components, `swipe-arbitration/SKILL.md`). A plugin or directory-scoped name
/// (`codex:rescue`, `apps/web:deploy`) loads the skill named after its last `:`. A reference file
/// under the skill (`references/x.md`) is not loaded by the Skill tool and stays unmatched.
pub(super) fn names(text: &str, file: &str) -> bool {
    let Some(dir) = file.strip_suffix("/SKILL.md") else { return false };
    let dir = dir.rsplit('/').next().unwrap_or(dir);
    if dir.is_empty() || !text.contains("\"skill\"") {
        return false;
    }
    let Ok(input) = serde_json::from_str::<Value>(text) else { return false };
    input.get("skill").and_then(Value::as_str).is_some_and(|s| s.rsplit(':').next() == Some(dir))
}

#[cfg(test)]
mod tests {
    use super::names;

    #[test]
    fn the_skill_tool_loads_the_returned_skill_md() {
        let hit = "swipe-arbitration/SKILL.md";
        assert!(names(r#"{"skill":"swipe-arbitration"}"#, hit));
        assert!(names(r#"{"skill":"swipe-arbitration","args":"drawer"}"#, hit));
        assert!(names(r#"{"skill":"codex:rescue"}"#, "rescue/SKILL.md"));
        assert!(names(r#"{"skill":"apps/web:deploy"}"#, "deploy/SKILL.md"));
        // Another skill, a longer name, a reference file or a non-Skill input is no use.
        assert!(!names(r#"{"skill":"swipe"}"#, hit));
        assert!(!names(r#"{"skill":"swipe-arbitration-v2"}"#, hit));
        assert!(!names(r#"{"skill":"swipe-arbitration"}"#, "references/rules.md"));
        assert!(!names(r#"{"file_path":"/x/swipe-arbitration/README.md"}"#, hit));
        assert!(!names("cat swipe-arbitration", hit));
    }
}
