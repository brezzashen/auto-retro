//! A shell glob path that expands to a returned file reads it. Search-retro 816476999821716281017
//! (3ea38b02…jsonl:797): `query` returned `MarsCRM/Shell/AppRouter.swift:3-50`, and the next turn
//! ran `grep -n "func open(path" -A 30 MarsCRM/Shell/AppRouter*.swift MarsCRM/Shell/*.swift` (:805),
//! reading the hit through a glob, yet the hit counted unadopted.
//!
//! A word counts when its last component is a glob (`*`, `?`) that matches the file name and its
//! directory part ends with the file's directory: with two components (`MarsCRM/Shell/*.swift`),
//! or with one when the glob keeps a literal stem prefix of six characters or more
//! (`Shell/AppRouter*.swift`). A bare `*.swift` or `src/*.ts` names no file.

/// Shortest literal prefix that makes a one-directory glob specific to the file.
const MIN_PREFIX: usize = 6;

/// True when a word of `command` is a glob path that expands to `file`.
pub(super) fn names(command: &str, file: &str) -> bool {
    let Some((dir, name)) = file.rsplit_once('/') else { return false };
    let dirs: Vec<&str> = dir.rsplit('/').take(2).collect();
    command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')'))
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .filter_map(|w| w.rsplit_once('/'))
        .filter(|(_, last)| last.contains(['*', '?']) && !last.contains('/'))
        .any(|(word_dir, last)| {
            let word_dirs: Vec<&str> = word_dir.rsplit('/').take(2).collect();
            let literal = last.find(['*', '?']).unwrap_or(last.len());
            let in_dir = match word_dirs.as_slice() {
                [parent, grand, ..] => dirs.len() == 2 && *parent == dirs[0] && *grand == dirs[1],
                _ => false,
            } || word_dirs.first() == dirs.first() && literal >= MIN_PREFIX;
            in_dir && matches(last.as_bytes(), name.as_bytes())
        })
}

/// Shell glob match of one path component: `*` any run, `?` one character, else literal.
fn matches(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => matches(&pattern[1..], name) || (!name.is_empty() && matches(pattern, &name[1..])),
        (Some(b'?'), Some(_)) => matches(&pattern[1..], &name[1..]),
        (Some(p), Some(n)) if p == n => matches(&pattern[1..], &name[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::names;

    const FILE: &str = "/path/to/workspace/Mars_CRM/ios/MarsCRM/Shell/AppRouter.swift";

    #[test]
    fn a_glob_in_the_files_directory_reads_it() {
        // 3ea38b02…jsonl:805, the read of the :797 hit.
        let cmd = r#"LOCAL_SEARCH_MISS=regex grep -n "func open(path" -A 30 MarsCRM/Shell/AppRouter*.swift MarsCRM/Shell/*.swift 2>/dev/null | head"#;
        assert!(names(cmd, FILE));
        assert!(names("sed -n 1,40p Shell/AppRouter*.swift", FILE));
        assert!(names("cat 'MarsCRM/Shell/App?outer.swift'", FILE));
    }

    #[test]
    fn a_generic_or_foreign_glob_names_no_file() {
        assert!(!names("grep -n open *.swift", FILE));
        assert!(!names("grep -n open Shell/*.swift", FILE));
        assert!(!names("grep -n open Other/Shell/*.swift", FILE));
        assert!(!names("grep -n open MarsCRM/Shell/*.ts", FILE));
        assert!(!names("grep -n open MarsCRM/Shell/AppRouter.swift", FILE));
        assert!(!names("grep -n open MarsCRM/Shell/App*", "AppRouter.swift"));
    }
}
