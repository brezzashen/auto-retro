//! A query that is a shell loop variable (`for v in '1.64.1' '10.6.5'; do local-search search "$v"`)
//! asks for the loop's words. Search-retro 816857502053349117708 (bfff573f…jsonl:474): five dependency
//! pins searched in one loop printed every pin line, the agent wrote "Pin 已全部覆盖" and installed;
//! the query read as `$v`, which has no name, so the answered hit could never count as adopted.

/// `query`, or the words of the `for <var> in …; do` loop of `command` when `query` is only `$var`
/// or `${var}`, joined by `|` as one alternation.
pub(super) fn resolve(command: &str, query: &str) -> String {
    let var = query.strip_prefix("${").and_then(|v| v.strip_suffix('}')).or_else(|| query.strip_prefix('$'));
    let Some(var) = var.filter(|v| !v.is_empty() && v.chars().all(|c| c.is_alphanumeric() || c == '_')) else {
        return query.to_string();
    };
    let head = format!("for {var} in ");
    let Some(start) = command.find(&head) else { return query.to_string() };
    let list = &command[start + head.len()..];
    let end = list.find([';', '\n']).unwrap_or(list.len());
    let words: Vec<String> = shell_words(&list[..end]);
    if words.is_empty() { query.to_string() } else { words.join("|") }
}

/// Words of a loop list with their quotes removed.
fn shell_words(list: &str) -> Vec<String> {
    let (mut out, mut current, mut quote) = (Vec::new(), String::new(), None::<char>);
    for c in list.chars() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => current.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() { out.push(std::mem::take(&mut current)); }
            }
            (None, c) => current.push(c),
        }
    }
    if !current.is_empty() { out.push(current); }
    out
}

/// A dotted version of three or more all-digit parts (`1.64.1`, `18.7.0`) is one name: its parts
/// are too short to be names, and a later search for the same version is a re-search.
pub(super) fn is_version(word: &str) -> bool {
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() >= 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loop_variable_query_asks_for_the_loop_words() {
        let command = "cd /r; for v in '1.64.1' '10.6.5' 'mcr.microsoft.com/playwright'; do echo \"== $v\"; ~/.local/bin/local-search search \"$v\" --corpus code; done";
        assert_eq!(resolve(command, "$v"), "1.64.1|10.6.5|mcr.microsoft.com/playwright");
        assert_eq!(resolve("for name in a b\ndo local-search search \"${name}\"; done", "${name}"), "a|b");
        assert_eq!(resolve(command, "$other"), "$other");
        assert_eq!(resolve(command, "ShipGateway"), "ShipGateway");
    }

    #[test]
    fn a_dotted_version_is_one_name() {
        assert!(is_version("1.64.1") && is_version("18.7.0"));
        assert!(!is_version("0.5") && !is_version("v1.2.3") && !is_version("a.b.c"));
    }
}
