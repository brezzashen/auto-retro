//! A member chain in a query (`AppStorePurchases.shared.configure`) names its type, not the plain
//! words of its members. Search-retro 816805219412517074086: `search "AppStorePurchases.shared.configure"`
//! (f1cbfdbf…jsonl:3217) printed the call site `MarsCRMApp.swift:99`, which answered where the store
//! is wired. Two turns later the agent grepped `static let shared` in APIClient.swift and
//! AccountStore.swift (:3253), looking for another type's singleton; the member word `shared` made
//! that a re-search of the same names, so the answered hit counted as unadopted.
//!
//! The type head stays a name, and so does any identifier-shaped member (camelCase, a digit or `_`
//! inside, as `refreshUsage`); a plain lowercase member (`shared`, `configure`, `swift`) does not.
//! Only a distinctive head counts (UpperCamelCase of `member::MIN_TYPE` or more characters, as in
//! `member.rs`): in `Self.glyphs` the member is the subject and both words stay names.
//!
//! A plain lowercase head (`process`, `self`, `os`) is an access path, not a type: only its
//! identifier-shaped members of four or more characters are names (`lower_chain`). Search-retro
//! 812181474823629636591 (f7ae8eda…jsonl:519): `search "process.env.AGENT_"` answered NONE, which was
//! right (no `process.env.AGENT_` in Mars_CRM/agent/src); the next step listed every env reader with
//! `rg -n "process\.env" src -l`, and the head word `process` made that file list a false NONE.

/// The names of the member chains in `query` and the query with those chains blanked out.
pub(super) fn split(query: &str) -> (Vec<String>, String) {
    let mut found = Vec::new();
    let mut rest = String::with_capacity(query.len());
    let mut run = String::new();
    let flush = |run: &mut String, rest: &mut String, found: &mut Vec<String>| {
        match names(run) {
            Some(names) => {
                found.extend(names);
                rest.push_str(&" ".repeat(run.len()));
            }
            None => rest.push_str(run),
        }
        run.clear();
    };
    for c in query.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_') {
            run.push(c);
        } else {
            flush(&mut run, &mut rest, &mut found);
            rest.push(c);
        }
    }
    flush(&mut run, &mut rest, &mut found);
    (found, rest)
}

/// `Type.member…` as its type and identifier-shaped members; None when `word` is no such chain.
fn names(word: &str) -> Option<Vec<String>> {
    let mut parts = word.split('.');
    let head = parts.next()?;
    let members: Vec<&str> = parts.collect();
    let member_shaped = |m: &&str| m.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !distinctive_type(head) {
        return lower_chain(head, &members);
    }
    if members.is_empty() || !members.iter().all(member_shaped) {
        return None;
    }
    Some(std::iter::once(head).chain(members.iter().filter(|m| identifier(m)).copied()).map(str::to_string).collect())
}

/// A camelCase, digit or `_` inside: an identifier rather than a plain word.
fn identifier(word: &str) -> bool {
    word.chars().any(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// `process.env.AGENT_` as its identifier members (`AGENT_`); None unless the head is a plain
/// lowercase word and some member is an identifier of four or more characters.
fn lower_chain(head: &str, members: &[&str]) -> Option<Vec<String>> {
    if head.is_empty() || !head.chars().all(|c| c.is_ascii_lowercase()) || members.iter().any(|m| m.is_empty()) {
        return None;
    }
    let names: Vec<String> = members.iter().filter(|m| identifier(m) && m.chars().count() >= 4).map(|m| m.to_string()).collect();
    (!names.is_empty()).then_some(names)
}

/// UpperCamelCase with a lower-to-upper hump and at least `member::MIN_TYPE` characters.
fn distinctive_type(head: &str) -> bool {
    let chars: Vec<char> = head.chars().collect();
    chars.len() >= super::member::MIN_TYPE
        && chars[0].is_ascii_uppercase()
        && chars.iter().all(|c| c.is_ascii_alphanumeric())
        && chars.windows(2).any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::split;

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_chain_names_its_type_and_identifier_members() {
        // f1cbfdbf…jsonl:3217.
        let (found, rest) = split("AppStorePurchases.shared.configure");
        assert_eq!(found, strings(&["AppStorePurchases"]));
        assert!(rest.trim().is_empty());
        assert_eq!(split("AccountStore.refreshUsage").0, strings(&["AccountStore", "refreshUsage"]));
        assert_eq!(split("HeadlineText.fit(|fit_headline(").0, strings(&["HeadlineText"]));
        assert!(split("HeadlineText.fit(|fit_headline(").1.contains("fit_headline"));
        assert_eq!(split("SettingsScreen.swift").0, strings(&["SettingsScreen"]));
        // A bundle keeps the names it had: `app` is under four letters either way (domain.rs).
        assert_eq!(split("UpdateWatch.app").0, strings(&["UpdateWatch"]));
    }

    #[test]
    fn a_short_head_a_host_or_a_plain_word_is_no_chain() {
        assert!(split("Self.glyphs").0.is_empty());
        assert!(split("route.fulfill").0.is_empty());
        assert!(split("apps.apple.com").0.is_empty());
        assert!(split("AppStorePurchases").0.is_empty());
        assert!(split("AppStorePurchases.Shared").0.is_empty());
        assert!(split("window.location.href").0.is_empty());
        assert!(split("api.v1").0.is_empty());
    }

    #[test]
    fn a_lowercase_access_path_names_only_its_identifier_members() {
        // f7ae8eda…jsonl:519: `process` is no name of `process.env.AGENT_`.
        let (found, rest) = split("process.env.AGENT_");
        assert_eq!(found, strings(&["AGENT_"]));
        assert!(rest.trim().is_empty());
        assert_eq!(split("self.refreshUsage").0, strings(&["refreshUsage"]));
        assert_eq!(split("os.environ.HOME|crm.credit_usage_rollup").0, strings(&["HOME", "credit_usage_rollup"]));
    }
}
