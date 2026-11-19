//! Lines a local-search NONE inside the sweep step printed are that search's, not the sweep's
//! matches. Search-retro 82856125487639539959: the NONE for `StepCardView|StrandsOrb` under
//! `--path ios/MarsCRM/Features/Onboarding` (895a65e4…jsonl:298) was right (no Onboarding file names
//! either), and the next step (:310) ran `local-search search "StepCardView" … --path …Onboarding |
//! head -6; grep -n "StepCardView\|AssistantTurnView\|StrandsOrb" …/OnboardingChatPhone.swift`. The
//! grep printed three `AssistantTurnView` lines; the four `StepCardView.swift:N:` lines under the
//! search's own `-- outside --path: 7 file(s) hold it on one line, first 3 --` label made it a false
//! NONE. A block starts at a `NONE` line and holds its `  -- … --` labels, near lines `(words spread
//! over the chunk)`, and `path:N:text` lines of at most the `first K` files its label names (3 when
//! no label says); it ends at the first other line, and after `| head -N` within N lines. An rg/grep
//! line of another file, or a line past the head limit, stays a match.
//! Since search-retro 817272677504635157723 a block also holds local-search's own hit headers
//! (`path:N  [kind] …`, `url  [report/section]`) and their indented text lines, and a replay marked
//! `LOCAL_SEARCH_NO_LOG` strips the same way: the NONE for `.cache/whisper|whisper-models`
//! (8ee59d2a…jsonl:901) was right, the next step's `rg -l` of unindexed config trees printed nothing,
//! and the step's trailing `LOCAL_SEARCH_NO_LOG=1 local-search search "cache/whisper" --corpus history`
//! printed a NONE whose `-- tool calls hold it --` lines (`…jsonl:679  [Bash] …`) made it false.
use super::quality::commands;

/// The output without the NONE blocks a local-search of the sweep step printed.
pub(super) fn strip(output: &str, sweep: &str) -> String {
    if !runs_search(sweep) {
        return output.to_string();
    }
    let cap = head_limit(sweep);
    let (mut kept, mut block, mut files, mut budget) = (Vec::new(), None::<usize>, Vec::<String>::new(), 3);
    for line in output.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("NONE") {
            (block, files, budget) = (Some(1), Vec::new(), 3);
            continue;
        }
        if let Some(n) = block {
            let room = cap.is_none_or(|c| n < c);
            let label = trimmed.starts_with("-- ") && trimmed.trim_end().ends_with("--");
            let near = trimmed.ends_with("(words spread over the chunk)") || hit_header(trimmed) || (line.starts_with("    ") && !trimmed.is_empty());
            let file = file_of(trimmed).filter(|f| files.contains(f) || files.len() < budget);
            if room && (label || near || file.is_some()) {
                if label { budget = first_k(trimmed).unwrap_or(budget); files.clear(); }
                if let Some(f) = file { if !files.contains(&f) { files.push(f); } }
                block = Some(n + 1);
                continue;
            }
            block = None;
        }
        kept.push(line);
    }
    kept.join("\n")
}

/// True when a simple command of the step runs `local-search search|query`, a `LOCAL_SEARCH_NO_LOG`
/// replay included (`quality::invocation` leaves replays out of the call count, not out of the step).
fn runs_search(sweep: &str) -> bool {
    commands(sweep).iter().any(|(_, segment)| {
        let words: Vec<&str> = segment.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
        words.first().is_some_and(|p| p.rsplit('/').next() == Some("local-search")) && matches!(words.get(1), Some(&("search" | "query")))
    })
}

/// A local-search hit header: a locator (`path:N`, `path:N-M` or a URL) then spaces and `[kind]`.
/// rg and grep print `path:N:text` without the spaced bracket.
fn hit_header(line: &str) -> bool {
    let Some((locator, rest)) = line.split_once("  ") else { return false };
    let rest = rest.trim_start();
    let located = locator.starts_with("http") || locator.rsplit_once(':').is_some_and(|(path, n)| !path.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '-'));
    located && !locator.contains(char::is_whitespace) && rest.starts_with('[') && rest[1..].split_once(']').is_some_and(|(kind, _)| !kind.is_empty() && !kind.contains(' '))
}

/// `path` of a `path:N:text` line (a path holds a `/` or a `.`, no spaces).
fn file_of(line: &str) -> Option<String> {
    let mut parts = line.splitn(3, ':');
    let (path, number) = (parts.next()?, parts.next()?);
    parts.next()?;
    let pathlike = !path.is_empty() && !path.contains(char::is_whitespace) && (path.contains('/') || path.contains('.'));
    (pathlike && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())).then(|| path.to_string())
}

/// `K` of a label `… first K --`.
fn first_k(label: &str) -> Option<usize> {
    label.split("first ").nth(1)?.split_whitespace().next()?.parse().ok()
}

/// `N` of a `| head -N` / `| head -n N` right after the step's local-search, when there is one.
fn head_limit(sweep: &str) -> Option<usize> {
    let segments = commands(sweep);
    let at = segments.iter().position(|(_, s)| s.split_whitespace().find(|w| !w.contains('=')).is_some_and(|p| p.rsplit('/').next() == Some("local-search")))?;
    // `2>&1` splits at `&` into `… 2>` and `1`: still the search's own redirection.
    let (piped, next) = segments[at + 1..].iter().find(|(_, s)| !s.trim().chars().all(|c| c.is_ascii_digit()))?;
    let words: Vec<&str> = next.split_whitespace().collect();
    if !piped || words.first() != Some(&"head") {
        return None;
    }
    match words.get(1) {
        Some(&"-n") => words.get(2)?.parse().ok(),
        Some(w) => w.strip_prefix('-')?.parse().ok(),
        None => Some(10),
    }
}
