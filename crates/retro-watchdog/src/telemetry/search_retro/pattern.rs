//! The pattern text of one rg/grep/find simple command: what the sweep looked for, without the
//! files it looked in. Search-retro 82450840685001354157: a code NONE for the query "delete
//! labelled test threads … demo account" (c82d6abb…jsonl:697) was counted false because the next
//! `grep -n "case '\|command ===\|function \|^export" demo-session.ts` (c82d6abb…jsonl:708) named
//! `demo` in its file argument; its pattern held no query word and read one known file.

/// Short flags of rg/grep that take the next word as their value.
const SHORT_VALUE: &[char] = &['A', 'B', 'C', 'm', 'e', 'f', 'g', 't', 'T', 'd', 'D', 'M'];
/// Long flags of rg/grep that take the next word as their value when written without `=`.
const LONG_VALUE: &[&str] = &["--regexp", "--file", "--glob", "--iglob", "--type", "--type-not", "--include", "--exclude", "--exclude-dir", "--max-count", "--context", "--after-context", "--before-context", "--max-depth", "--max-columns"];
/// find predicates whose value is a name pattern.
const FIND_NAME: &[&str] = &["-name", "-iname", "-path", "-ipath", "-regex", "-iregex", "-wholename", "-iwholename"];

/// The patterns of `segment` (one simple command whose program is rg, grep, egrep, ugrep or find),
/// joined by newlines; `None` when no pattern could be read, so the caller keeps the whole segment.
/// rg/grep: every `-e`/`--regexp` value, else the first positional word. find: every name-predicate value.
pub(super) fn of(segment: &str) -> Option<String> {
    let words = words(segment);
    let program = words.iter().position(|w| !w.contains('=') || w.starts_with('-'))?;
    let name = words[program].rsplit('/').next().unwrap_or_default();
    let args = &words[program + 1..];
    let found: Vec<String> = if name == "find" {
        args.windows(2).filter(|w| FIND_NAME.contains(&w[0].as_str())).map(|w| w[1].clone()).collect()
    } else {
        grep_operands(args).0
    };
    (!found.is_empty()).then(|| found.join("\n"))
}

/// What a non-grep search (session-status, agent-memory recall) asked for: its words after the
/// program word. Search-retro 814125901426638775162: `~/.local/bin/session-status fc3856e7`
/// (04a9629b…jsonl:1142) "re-searched" the `status` of the query "deliver status" by its own name.
pub(super) fn arguments(segment: &str) -> String {
    let words = words(segment);
    let program = words.iter().position(|w| !w.contains('=') || w.starts_with('-')).unwrap_or(words.len());
    words.get(program + 1..).unwrap_or_default().join(" ")
}

/// A three-character ASCII word (`ICP`, `278`, `man` of `/man/`): the whole of a one-word query, it is
/// the name looked for, re-searched only as a whole word (`answered.rs` holds). Search-retro
/// 814125901426638775162: `search "ICP"` (31b7923a…jsonl:152) answered from its printed lines, but a
/// query without a name of four or more characters could never count as answered; upper-case
/// acronyms only until founder decision 1A on search-retro 85874229037822047064 (2026-11-09) extended
/// it to any three-character word (`search "278"` b7876efb…jsonl:254, `search "/man/"` 044c53fa…jsonl:803).
pub(super) fn short_word(word: &str) -> bool {
    word.chars().count() == 3 && word.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The files an rg/grep simple command reads: its positional words after the pattern, without
/// redirections (`2>/dev/null`). Empty for other programs.
pub(super) fn files(segment: &str) -> Vec<String> {
    let words = words(segment);
    let Some(program) = words.iter().position(|w| !w.contains('=') || w.starts_with('-')) else { return Vec::new() };
    if !matches!(words[program].rsplit('/').next().unwrap_or_default(), "rg" | "grep" | "egrep" | "ugrep") {
        return Vec::new();
    }
    grep_operands(&words[program + 1..]).1.into_iter().filter(|w| !w.is_empty() && !w.contains('>') && !w.starts_with('<')).collect()
}

/// rg/grep operands as (patterns, files): every `-e`/`--regexp` value, else the first positional
/// word, is a pattern; the other positional words are files.
fn grep_operands(args: &[String]) -> (Vec<String>, Vec<String>) {
    let (mut explicit, mut positional, mut i) = (Vec::new(), Vec::new(), 0);
    while i < args.len() {
        let w = &args[i];
        if w == "--" {
            positional.extend(args[i + 1..].iter().cloned());
            break;
        }
        if let Some(v) = w.strip_prefix("--regexp=") {
            explicit.push(v.to_string());
        } else if w.starts_with("--") {
            if !w.contains('=') && LONG_VALUE.contains(&w.as_str()) {
                if w == "--regexp" { explicit.extend(args.get(i + 1).cloned()); }
                i += 1;
            }
        } else if w.starts_with('-') && w.len() > 1 {
            // `-rn`, `-A3`, `-e pat`, `-epat`: the first value-taking letter takes the rest or the next word.
            let flags: Vec<char> = w[1..].chars().collect();
            if let Some(at) = flags.iter().position(|c| SHORT_VALUE.contains(c)) {
                let rest: String = flags[at + 1..].iter().collect();
                let value = if rest.is_empty() { i += 1; args.get(i).cloned() } else { Some(rest) };
                if flags[at] == 'e' { explicit.extend(value); }
            }
        } else {
            positional.push(w.clone());
        }
        i += 1;
    }
    if explicit.is_empty() && !positional.is_empty() {
        let first = positional.remove(0);
        (vec![first], positional)
    } else {
        (explicit, positional)
    }
}

/// Shell words with quotes removed; a backslash inside double quotes escapes the next character.
pub(super) fn words(segment: &str) -> Vec<String> {
    let (mut out, mut current, mut quote, mut started) = (Vec::new(), String::new(), None::<char>, false);
    let mut chars = segment.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some('"'), '\\') => current.extend(chars.next()),
            (Some(_), _) => current.push(c),
            (None, '"' | '\'') => { quote = Some(c); started = true; }
            (None, '\\') => { current.extend(chars.next()); started = true; }
            (None, c) if c.is_whitespace() => {
                if started || !current.is_empty() { out.push(std::mem::take(&mut current)); }
                started = false;
            }
            (None, c) => current.push(c),
        }
    }
    if started || !current.is_empty() { out.push(current); }
    out
}

#[cfg(test)]
mod tests {
    use super::of;

    #[test]
    fn a_file_argument_is_not_the_pattern() {
        // c82d6abb…jsonl:708.
        let p = of(r#"grep -n "case '\|command ===\|function \|^export\|^import" demo-session.ts"#).unwrap();
        assert!(p.contains("command ===") && !p.contains("demo"));
        assert_eq!(of("rg -n parityWatch web").as_deref(), Some("parityWatch"));
        assert_eq!(of("grep -rn -A 3 --include '*.ts' fetchDossier src").as_deref(), Some("fetchDossier"));
        assert_eq!(of("rg -g '*.rs' -C2 'fn score|grade' .").as_deref(), Some("fn score|grade"));
    }

    #[test]
    fn explicit_patterns_and_find_names_are_read() {
        assert_eq!(of("grep -n -e alpha -e beta f.ts").as_deref(), Some("alpha\nbeta"));
        assert_eq!(of("rg --regexp=alpha src").as_deref(), Some("alpha"));
        assert_eq!(of("find . -name '*Purge*.ts' -type f").as_deref(), Some("*Purge*.ts"));
        assert_eq!(of("LOCAL_SEARCH_MISS=1 rg -n x -- -dash").as_deref(), Some("x"));
        assert_eq!(of("find src -type f"), None);
        assert_eq!(of("grep -rn"), None);
    }
}
