//! A CLI whose subcommands are modules runs a returned file when a command names its directory and
//! stem as two consecutive words: `mars-skill-tools conversion-analytics query "select …"` runs
//! `skills/conversion_analytics/query.rs`.
//!
//! Search-retro 89200397237788824863: 68a63afd…jsonl:500 searched `posthog query|hogql|…`, the hit
//! printed `conversion_analytics/query.rs:23: fn hogql(…)`, and the next three tool calls ran
//! `mars-skill-tools conversion-analytics query "<HogQL>"`; the hit counted unadopted because
//! `names_stem` (`adopt.rs`) needs a stem of six or more characters with `-` or `_` inside, and
//! `query` alone is too common a word to name the file.
use super::values;

/// Shortest directory name that may name the module: a kebab or snake name this long is distinctive
/// (the same bar as `adopt::MIN_STEM`).
const MIN_DIR: usize = 6;

/// Shortest file stem accepted after the directory (`run`, `add`, `query`).
const MIN_STEM: usize = 3;

/// True when a non-search segment of `command` holds the file's directory and stem as consecutive
/// words, either in kebab or snake form. The directory must be a kebab or snake name of MIN_DIR+
/// characters, so plain words such as `src lib` never match.
pub(super) fn names(command: &str, file: &str) -> bool {
    let mut parts = file.rsplit('/');
    let name = parts.next().unwrap_or("");
    let Some(dir) = parts.next() else { return false };
    let stem = name.split('.').next().unwrap_or("");
    let ident = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    let dir_shaped = dir.chars().count() >= MIN_DIR && dir.contains(['-', '_']) && ident(dir);
    let stem_shaped = stem.chars().count() >= MIN_STEM && ident(stem) && stem != "mod" && stem != "lib" && stem != "main";
    if !dir_shaped || !stem_shaped || name == stem {
        return false;
    }
    let norm = |s: &str| s.trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase().replace('_', "-");
    let (dir, stem) = (norm(dir), norm(stem));
    super::quality::commands(command).iter().any(|(_, segment)| {
        if values::searches(segment) {
            return false;
        }
        let words: Vec<String> = segment.split_whitespace().map(norm).collect();
        words.windows(2).any(|w| w[0] == dir && w[1] == stem)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIT: &str = "conversion_analytics/query.rs";

    #[test]
    fn a_cli_subcommand_names_its_module_file() {
        // 68a63afd…jsonl:500 → the next tool call.
        assert!(names("cd /r; mars-skill-tools conversion-analytics query \"select 1 from events\"", HIT));
        assert!(names("tool conversion_analytics query --json", HIT));
    }

    #[test]
    fn other_words_searches_and_plain_directories_do_not() {
        // The directory alone, the stem alone, another order or another stem is no use.
        assert!(!names("mars-skill-tools conversion-analytics collect", HIT));
        assert!(!names("mars-skill-tools query conversion-analytics", HIT));
        assert!(!names("echo query", HIT));
        // A search for the pair is a re-search, not a use.
        assert!(!names("rg -n 'conversion-analytics query' docs", HIT));
        // A short or unseparated directory is too common a word.
        assert!(!names("cargo src query", "src/query.rs"));
        assert!(!names("tool helpers query", "helpers/query.rs"));
        // mod.rs, lib.rs and main.rs stems are no subcommand.
        assert!(!names("tool seo-competitor mod", "seo_competitor/mod.rs"));
    }
}
