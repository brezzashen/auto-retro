//! A later call naming a returned file's type through one of its members follows the hit: Swift,
//! TypeScript and Kotlin files carry the name of the type they declare, so `HeadlineText.fit(` names
//! `People/HeadlineText.swift`. Search-retro 811147635907382047180 (df9a4534…jsonl:3997): a query
//! returned `People/HeadlineText.swift:124-156` (`static func fit`), and two turns later (:4015) the
//! agent searched that type's callers, `HeadlineText.fit(`; the hit counted unadopted.

/// Shortest type name (file stem) a call may name through a member; shorter ones (`View`, `Model`)
/// are too common to point at one file.
pub(super) const MIN_TYPE: usize = 8;

/// True when `text` names `<Stem>.<member>` for the file's UpperCamelCase stem, not inside a longer
/// name or after a `/` (another directory's file).
pub(super) fn names(text: &str, file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    let stem = name.split('.').next().unwrap_or("");
    let chars: Vec<char> = stem.chars().collect();
    let camel = chars.len() >= MIN_TYPE
        && chars[0].is_ascii_uppercase()
        && chars.iter().all(|c| c.is_ascii_alphanumeric())
        && chars.windows(2).any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase());
    if !camel || name == stem {
        return false;
    }
    text.match_indices(stem).any(|(i, _)| {
        let joined = text[..i].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/'));
        let member = text[i + stem.len()..].strip_prefix('.').is_some_and(|r| r.starts_with(|c: char| c.is_ascii_lowercase()));
        !joined && member
    })
}

#[cfg(test)]
mod tests {
    use super::names;

    #[test]
    fn a_member_of_the_returned_type_names_its_file() {
        let hit = "People/HeadlineText.swift";
        let call = r#"~/.local/bin/local-search search "headlineFit(|HeadlineText.fit(|fit_headline(" --corpus code --lines"#;
        assert!(names(call, hit));
        // A longer type, another directory's file, the bare type, a short stem or a snake stem is no use.
        assert!(!names("MyHeadlineText.fit(", hit));
        assert!(!names("cat Other/HeadlineText.swift", hit));
        assert!(!names("rg HeadlineText web", hit));
        assert!(!names("Model.load(", "Shared/Model.swift"));
        assert!(!names("release_watch.run(", "src/release_watch.rs"));
    }
}
