//! A non-search command that reads a returned file by its bare name, the shell already inside its
//! directory from an earlier call, uses the hit.
//!
//! Search-retro 813279914114122985562: 4937110f…jsonl:2843 searched `fn ads_keyword`, the hit printed
//! `seo_competitor/watch_trends_surge.rs:115`, and the next turn (:2857, 3 s later) ran
//! `sed -n 105,130p watch_trends_surge.rs` and printed the function: Claude Code had kept the `cd …/seo_competitor`
//! of :2735. The hit counted unadopted: the path rule needs `seo_competitor/watch_trends_surge.rs`,
//! `names_from_cd` a `cd` in the same command, and `names_stem` rejects a stem joined to `.rs`.
use super::{quality, values};

/// Shortest stem (name before its first `.`) whose bare file name may stand for the file: a kebab or
/// snake name this long is distinctive (the bar of `adopt::MIN_STEM`), so `mod.rs` or `main.rs` never
/// match another directory's copy.
const MIN_STEM: usize = 6;

/// True when a non-search segment of `command` holds the file's whole name as one word.
pub(super) fn names(command: &str, file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    let stem = name.split('.').next().unwrap_or("");
    let shaped = stem.chars().count() >= MIN_STEM && stem.contains(['-', '_']) && stem.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if !shaped || name == stem {
        return false;
    }
    quality::commands(command).iter().any(|(_, segment)| {
        !values::searches(segment) && segment.split_whitespace().any(|w| w.trim_matches(|c| c == '"' || c == '\'') == name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_distinctive_file_name_read_in_place_uses_the_hit() {
        let hit = "Mars_CRM/rust/crates/mars-skill-tools/src/skills/seo_competitor/watch_trends_surge.rs";
        // 4937110f…jsonl:2857.
        let cmd = "sed -n 105,130p watch_trends_surge.rs; jq -r '.judged[]|.keyword' round.json | grep -nP '[^\\x00-\\x7F]' | head";
        assert!(names(cmd, hit));
        assert!(names("cat 'watch_trends_surge.rs'", hit));
        // Another path, a longer name, a search, or a common short name is no use.
        assert!(!names("cat other/watch_trends_surge.rs", hit));
        assert!(!names("cat watch_trends_surge.rs.bak", hit));
        assert!(!names("grep -n ads_keyword watch_trends_surge.rs", hit));
        assert!(!names("sed -n 1,9p main.rs", "crates/x/src/main.rs"));
        assert!(!names("sed -n 1,9p report.rs", "src/report.rs"));
    }
}
