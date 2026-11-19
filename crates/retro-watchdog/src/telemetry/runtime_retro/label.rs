//! Names the program a shell command waits on, for the runtime retro's tool-wait buckets.
//!
//! Runtime retro 811163801920064345384 (2026-11-07): the first program of a command took the whole
//! wait, so `mkdir -p $S; bun capture.js …`, `export ASC_KEY_ID=…; asc api …`, `cat > x <<EOF … EOF;
//! bun x` and `for i in …; do kubectl …; done` filed 24 of 63 tool-wait hours under `mkdir`,
//! `export`, `cat` and `for`. Setup and light read steps now yield to a later program, a heredoc body
//! is not a command, and a loop is named by its body.

const RUNNERS: [&str; 11] = ["bun", "bunx", "node", "npx", "npm", "cargo", "git", "kubectl", "gcloud", "orca", "xcrun"];
const WRAPPERS: [&str; 7] = ["timeout", "gtimeout", "env", "time", "nice", "command", "exec"];
/// Shell structure that runs nothing itself: a loop or branch header, or its closing word.
const STRUCTURE: [&str; 10] = ["for", "if", "elif", "case", "done", "fi", "esac", "}", ")", "cd"];
/// Words that only open the body that follows them.
const OPENERS: [&str; 6] = ["do", "then", "else", "{", "(", "!"];
/// Steps that prepare or read and finish at once; a later program in the same command is the wait.
const SETUP: [&str; 36] = [
    "mkdir", "export", "echo", "printf", "date", "rm", "cp", "mv", "touch", "ln", "chmod", "set", "unset", "source", ".",
    "trap", "true", ":", "local", "umask", "cat", "head", "tail", "ls", "sed", "grep", "rg", "jq", "wc", "sort", "uniq",
    "cut", "awk", "tr", "perl", "test",
];

/// The program a shell command runs, skipping `cd` steps and variable assignments; runners
/// (`bun`, `git`, `cargo`…) keep their subcommand or script so `bun run build` and `bun x.ts` differ.
/// A command made only of setup or read steps keeps its first one (`grep x f | head` → `grep`).
pub(in crate::telemetry) fn command_label(command: &str) -> String {
    let statements = split_steps(command);
    let steps: Vec<Vec<&str>> = statements.iter().map(|s| step_words(s))
        .filter(|words| words.first().is_some_and(|w| !STRUCTURE.contains(w)))
        .collect();
    let base = |w: &str| w.trim_matches(['"', '\'']).trim_end_matches('/').rsplit('/').next().unwrap_or(w).to_string();
    let programs: Vec<&Vec<&str>> = steps.iter().filter(|words| is_program(words[0])).collect();
    let chosen = programs.iter().find(|words| !SETUP.contains(&base(words[0]).as_str())).or(programs.first()).copied();
    let Some(words) = chosen.or(steps.first()) else {
        return String::new();
    };
    let first = base(words[0]);
    let mut rest: Vec<String> = Vec::new();
    let mut skip_value = false;
    for word in &words[1..] {
        // `git -C <dir>`, `git -c <key=value>`, `bun run --cwd <dir>` and `kubectl --context <c> -n <ns>` take a value that is not the subcommand.
        if std::mem::take(&mut skip_value) { continue; }
        // `bun -e '…'` and `node -e '…'` run inline code: the code is not a script name.
        if rest.is_empty() && matches!(*word, "-e" | "--eval") && matches!(first.as_str(), "bun" | "node") {
            rest.push("-e".into());
            break;
        }
        if word.starts_with('-') { skip_value = matches!(*word, "-C" | "-c" | "--cwd" | "--context" | "-n" | "--namespace"); continue; }
        rest.push(base(word));
        if rest.len() == 2 { break; }
    }
    let label = match (RUNNERS.contains(&first.as_str()), rest.as_slice()) {
        (true, [run, script, ..]) if run == "run" => format!("{first} run {script}"),
        (true, [next, ..]) => format!("{first} {next}"),
        _ => first,
    };
    label.chars().take(60).collect()
}

/// The words of one step without assignments, wrappers (`timeout 120`) or body openers (`do`).
fn step_words(step: &str) -> Vec<&str> {
    // `udid=$(sim-lease acquire …)` runs `sim-lease`.
    let mut words: Vec<&str> = step.split_whitespace()
        .map(|w| w.find("=$(").map_or(w, |at| &w[at + 3..]))
        .filter(|w| !w.is_empty() && !(w.contains('=') && !w.starts_with('-'))).collect();
    loop {
        if words.first().is_some_and(|w| OPENERS.contains(w)) {
            words.remove(0);
        } else if words.first().is_some_and(|w| WRAPPERS.contains(w)) {
            words.remove(0);
            while words.first().is_some_and(|w| w.starts_with('-') || w.chars().all(|c| c.is_ascii_digit() || c == '.' || c == 's')) { words.remove(0); }
        } else {
            return words;
        }
    }
}

/// A word that can name a program: not a flag, a number, a fragment of quoted text (`error:`) or a
/// bare variable; `$S/render.sh` and `~/.local/bin/x` count.
fn is_program(word: &str) -> bool {
    let word = word.trim_matches(['"', '\'']);
    let Some(first) = word.chars().next() else { return false };
    (first.is_ascii_alphabetic() || "._~/$".contains(first))
        && (first != '$' || word.contains('/'))
        && word.chars().all(|c| c.is_ascii_alphanumeric() || "._~/+-${}".contains(c))
}

/// The statements of a command: split on newlines, `;`, `|` and `&` outside quotes (not the `&` of
/// `2>&1`, `>&2` or `&>`), with every heredoc body (`<<'EOF'` … `EOF`) left out.
fn split_steps(command: &str) -> Vec<String> {
    let (mut steps, mut current) = (Vec::new(), String::new());
    let (mut single, mut double, mut escaped) = (false, false, false);
    let mut heredoc: Option<String> = None;
    for line in command.split('\n') {
        if let Some(tag) = &heredoc {
            if line.trim() == tag { heredoc = None; }
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let (quoted, was_escaped) = (single || double, std::mem::take(&mut escaped));
            if was_escaped {
            } else if c == '\\' && !single { escaped = true; }
            else if c == '\'' && !double { single = !single; } else if c == '"' && !single { double = !double; }
            let redirect = c == '&' && (i > 0 && chars[i - 1] == '>' || chars.get(i + 1) == Some(&'>'));
            if !quoted && !was_escaped && (c == ';' || c == '|' || (c == '&' && !redirect)) {
                steps.push(std::mem::take(&mut current));
            } else {
                current.push(c);
            }
        }
        if single || double {
            current.push('\n');
        } else {
            steps.push(std::mem::take(&mut current));
            heredoc = heredoc_tag(line);
        }
    }
    steps.push(current);
    steps
}

/// `cat > f <<'EOF'` → `EOF`; `<<-TAG` and `<<"TAG"` too; a here-string `<<<` is not a heredoc.
fn heredoc_tag(line: &str) -> Option<String> {
    let at = line.find("<<")?;
    let rest = &line[at + 2..];
    if rest.starts_with('<') { return None; }
    let tag: String = rest.trim_start_matches('-').trim_start().trim_start_matches(['\'', '"'])
        .chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    (!tag.is_empty()).then_some(tag)
}
