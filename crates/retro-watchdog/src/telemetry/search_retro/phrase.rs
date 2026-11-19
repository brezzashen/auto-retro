//! A NONE for a phrase is refuted only by a printed line that holds the phrase's words.
//! Search-retro 818262728038716071109 (2e3d12c6…jsonl:180): `local-search search "from 'three'|from 'three/"`
//! answered "no line of their files has from 'three'|from 'three/"; the next sweep
//! `rg -n "['\"]three['\"/]|img-fx|@types/three" src scripts package.json vite.config.ts` (:191)
//! printed `package.json:47: "three": "0.186.1"` and comments naming three.js, but no import of
//! `three`. The NONE was right, yet the name `three` alone counted it as false. The rule applies
//! only when every alternative of a `search` pattern has two or more words and the sweep prints
//! `N:text` lines (`-n`, no `-l`/`-c`); a file list (4aedaa8c…jsonl:168, `enum MarsSymbol`, `-l`)
//! keeps the earlier rule in `none.rs`. An alternative with a punctuation-only part (`%% series`) is
//! refuted only by a line holding its text as written (`literal`, search-retro 815124133498867247874).
use super::{none, quality, steps};

/// `Some(false)` when the sweep printed no `N:text` line holding every word of one alternative of
/// the phrase `search`, `Some(true)` when it did, `None` when the rule does not apply.
pub(super) fn refuted(output: &str, sweep: &str, command: &str) -> Option<bool> {
    let pattern = search_pattern(command)?;
    let alternatives: Vec<(Option<String>, Vec<String>)> = pattern.split('|').map(|a| (literal(a), words(a))).collect();
    if alternatives.iter().any(|(lit, w)| lit.is_none() && w.len() < 2) || !none::numbered(sweep) {
        return None;
    }
    Some(output.lines().filter_map(none::text_of).any(|text| {
        let (held, flat) = (words(text), text.split_whitespace().collect::<Vec<_>>().join(" "));
        alternatives.iter().any(|(lit, a)| match lit {
            Some(lit) => flat.contains(lit.as_str()),
            None => a.iter().all(|w| held.contains(w)),
        })
    }))
}

/// An alternative with a part that holds no word character (`%% series`) asks for its text as
/// written, which local-search reads literally (its `literal:` note): only a line holding that text,
/// spaces collapsed, refutes the NONE. Search-retro 815124133498867247874 (7b135b8c…jsonl:567):
/// `search "%% series" --path …/quota_economics/` was right (no legend there), and the next sweep
/// `rg -n "xychart|series|line \[|bar \[" …/quota_economics/` printed `Series` lines only.
fn literal(alternative: &str) -> Option<String> {
    let parts: Vec<&str> = alternative.split_whitespace().collect();
    (parts.len() >= 2 && parts.iter().any(|p| !p.chars().any(|c| c.is_alphanumeric() || c == '_'))).then(|| parts.join(" "))
}

/// Lowercase words of letters, digits and `_`.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// The pattern of the first `local-search search` in the command; `query` (vector) is not literal.
fn search_pattern(command: &str) -> Option<String> {
    quality::commands(command).into_iter().find_map(|(_, segment)| {
        let words: Vec<&str> = segment.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
        if words.first()?.rsplit('/').next() != Some("local-search") {
            return None;
        }
        let mut sub = 1;
        while words.get(sub).is_some_and(|w| *w == "--db") {
            sub += 2;
        }
        if words.get(sub) != Some(&"search") {
            return Some(None);
        }
        let rest = words[sub + 1..].join(" ");
        Some(match rest.chars().next() {
            Some(q @ ('"' | '\'')) => Some(steps::quoted(&rest[1..], q)),
            _ => rest.split_whitespace().next().map(str::to_string),
        })
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = "cd /r; ~/.local/bin/local-search search \"from 'three'|from 'three/\" --corpus code --path Mars_CRM/web --lines 2>&1 | head -30";

    #[test]
    fn a_wider_sweep_that_prints_only_the_bare_name_leaves_a_phrase_none_standing() {
        // 2e3d12c6…jsonl:191-193: the sweep printed package.json's version line and comments.
        let sweep = "cd web; LOCAL_SEARCH_MISS=regex rg -n \"['\\\"]three['\\\"/]|img-fx|@types/three\" src scripts package.json 2>&1 | head -20; sed -n 1,30p src/a.tsx";
        let out = "package.json:47:    \"three\": \"0.186.1\",\nsrc/ImageGenerating.tsx:1:/** img-fx and three.js load only for a turn that\nimport { ImageGeneration } from 'img-fx'\n *  dissolves toward the card tone, which img-fx reads from the preset";
        assert_eq!(refuted(out, sweep, SEARCH), Some(false));
        // A printed import refutes it.
        assert_eq!(refuted("src/a.ts:3:import * as THREE from 'three'", sweep, SEARCH), Some(true));
        assert_eq!(refuted("src/a.ts:3:import { Mesh } from \"three/addons\"", sweep, SEARCH), Some(true));
    }

    #[test]
    fn names_file_lists_and_vector_queries_keep_the_earlier_rule() {
        let sweep = "rg -n three src";
        assert_eq!(refuted("a.ts:1:three", sweep, "local-search search \"heroVerify|from 'three'\" --lines"), None);
        assert_eq!(refuted("a.ts:1:three", sweep, "local-search search three --lines"), None);
        assert_eq!(refuted("a.ts:1:three", sweep, "local-search query \"from three\" --corpus code"), None);
        // 4aedaa8c…jsonl:168: an `-l` file list stays a false NONE under `none.rs`.
        assert_eq!(refuted("MarsCRM/DesignSystem/MarsSymbol.swift", "rg -n \"MarsSymbol\\b\" MarsCRM -l", "local-search search \"enum MarsSymbol\" --lines"), None);
        assert_eq!(search_pattern("LOCAL_SEARCH_NO_LOG=1 local-search --db x.db search 'a b' --lines").as_deref(), Some("a b"));
    }

    #[test]
    fn a_punctuated_phrase_is_refuted_only_by_its_literal_text() {
        // 7b135b8c…jsonl:567-576: the sweep printed `series` lines but no `%% series` legend.
        let search = "cd /m; ~/.local/bin/local-search search \"%% series\" --corpus code --lines --path rust/crates/mars-ops/src/quota_economics/ 2>&1 | head -8";
        let sweep = "cd /m; LOCAL_SEARCH_MISS=none rg -n \"xychart|series|line \\[|bar \\[\" rust/crates/mars-ops/src/quota_economics/ | head -20";
        let out = "rust/crates/mars-ops/src/quota_economics/report/sections.rs:97:    let series: Vec<Series> = plans\nrust/crates/mars-ops/src/quota_economics/report.rs:83:/// One xychart series: bars or a line.";
        assert_eq!(refuted(out, sweep, search), Some(false));
        assert_eq!(refuted("a/report.rs:99:    %%  series: 现行, 建议", sweep, search), Some(true));
        assert_eq!(literal("%% series").as_deref(), Some("%% series"));
        assert_eq!(literal("from 'three'"), None);
    }
}
