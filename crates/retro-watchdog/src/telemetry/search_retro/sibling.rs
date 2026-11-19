//! A different identifier of the same family is not a re-search (search-retro 816559106459755584535).
//!
//! `answered::accepted` treats a later search holding two camelCase pieces of a name as a search for
//! that name again (`ComposerDraftStore` re-searched by `query "composer text draft"`). A naming
//! prefix breaks that rule: after `search "MarsDebugMemoryTab"` printed its UserDefaults line, the
//! agent surveyed other DEBUG flags with `search "MarsDebugEventsView"` and then
//! `search "MarsDebugFindQuery|find-query"` (d8746185…jsonl:399, :420, :440) while adding a new
//! `-MarsDebugNow` flag. Each later name shares only `mars` and `debug`; each side has a piece the
//! other lacks, so it names a sibling, not the same thing. A token whose pieces are a subset of the
//! name's (`TakeoverContext` for `ComposerTakeoverContext`) or a superset (`HomeGreetingTests` for
//! `HomeGreeting`) still re-searches it.
use super::answered::parts;

/// `looked_for` (original case) with every sibling identifier of the name blanked out, lowercased.
/// `whole` is the lowercased name, `pieces` its other lowercased parts.
pub(super) fn without_siblings(looked_for: &str, whole: Option<&str>, pieces: &[String]) -> String {
    let mut out = looked_for.to_string();
    for token in looked_for.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let lower = token.to_lowercase();
        if token.is_empty() || whole == Some(lower.as_str()) {
            continue;
        }
        let own: Vec<String> = parts(&[token.to_string()]).into_iter().filter(|p| *p != lower).collect();
        let shared = pieces.iter().filter(|p| own.contains(p)).count();
        let theirs = own.iter().any(|p| !pieces.contains(p));
        let mine = pieces.iter().any(|p| !own.contains(p));
        if shared >= 2 && theirs && mine {
            out = out.replace(token, " ");
        }
    }
    out.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(name: &str) -> Vec<String> {
        let whole = name.to_lowercase();
        parts(&[name.to_string()]).into_iter().filter(|p| *p != whole).collect()
    }

    #[test]
    fn a_sibling_with_its_own_piece_is_blanked() {
        let p = pieces("MarsDebugMemoryTab");
        assert_eq!(without_siblings("MarsDebugEventsView", Some("marsdebugmemorytab"), &p).trim(), "");
        assert_eq!(without_siblings("MarsDebugFindQuery|find-query", Some("marsdebugmemorytab"), &p), " |find-query");
    }

    #[test]
    fn subsets_supersets_and_plain_words_stay() {
        let p = pieces("ComposerTakeoverContext");
        assert_eq!(without_siblings("TakeoverContext", Some("composertakeovercontext"), &p), "takeovercontext");
        let p = pieces("HomeGreeting");
        assert_eq!(without_siblings("HomeGreetingTests", Some("homegreeting"), &p), "homegreetingtests");
        let p = pieces("ComposerDraftStore");
        assert_eq!(without_siblings("composer text draft", Some("composerdraftstore"), &p), "composer text draft");
    }

    #[test]
    fn a_sibling_flag_search_keeps_the_answer() {
        use super::super::answered::accepted;
        use super::super::steps::{Kind, Step};
        let shell = |text: &str| Step { at_ms: 1.0, id: "1".into(), kind: Kind::Shell, text: text.into(), line: 1, turn: "1".into() };
        // d8746185…jsonl:399 then :420.
        let names = vec!["MarsDebugMemoryTab".to_string()];
        assert!(accepted(&[], &[shell("~/.local/bin/local-search search \"MarsDebugEventsView\" --corpus code")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell("rg -n 'MarsDebugMemoryTab' ios")], 0.0, &names, false, 8, 9e5));
        assert!(!accepted(&[], &[shell("rg -n 'memory_tab|debug_memory' ios")], 0.0, &names, false, 8, 9e5));
    }
}
