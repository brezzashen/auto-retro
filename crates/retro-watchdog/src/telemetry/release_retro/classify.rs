//! Pure classification for the release command review: which shell segment starts a release
//! entry, what its text alone breaks (`lints`), whether a result failed, and which instruction
//! error a failed result shows. Needles come from real transcripts (2026-10-26..28).
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

const EARLY_CHARS: usize = 2_000;

/// `(entry prefix, pattern at a segment start)`; the first capture, when present, ends the name.
static ENTRIES: LazyLock<Vec<(&str, Regex)>> = LazyLock::new(|| [
    ("deliver ", r"^(?:bun\s+(?:run\s+)?|\S*mars-(?:dev-tools|deliver)\s+|(?:\S*/)?mars\s+)(?:\S*/)?deliver(?:\.ts)?\s+(beta-web|beta|canary|ios|mac)\b"),
    ("canary ", r"^bun\s+(?:run\s+)?canary\s+(release|migrate)\b"),
    ("release ", r"^bun\s+(?:run\s+)?release\s+(ship-web|prepare|build)\b"),
    ("k8s ", r"^bun\s+(?:run\s+)?k8s\s+(deploy|rollback)\b"),
    ("xcodebuild", r"^(?:\S*/)?xcodebuild\b.*(?:\s(?:build|test|archive|build-for-testing|test-without-building)\b|-exportArchive)"),
    ("docker ", r"^docker\s+(?:buildx\s+)?(build|push)\b"),
    ("gcloud builds submit", r"^gcloud\s+builds\s+submit\b"),
    ("kubectl ", r"^kubectl\b.*\b(set\s+image|rollout\s+restart)\b"),
    ("testflight upload", r"^(?:\S*/)?(?:asc\b.*\b(?:publish|builds\s+upload)|xcrun\s+altool)\b"),
].into_iter().map(|(k, p)| (k, Regex::new(p).unwrap())).collect());
static ASSIGN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[A-Za-z_][A-Za-z0-9_]*=\S*\s+|(?:time|nohup|env|exec|command)\s+|g?timeout\s+\S+\s+)").unwrap());
static GATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:\S*/)?cpu-build-gate\s+run\s+(?:.*?\s)?--\s+").unwrap());
static HEREDOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<<-?\s*['"]?([A-Za-z_]+)['"]?"#).unwrap());
static EXIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(?:Exit code:?|Process exited with code)\s*(-?\d+)").unwrap());
static REFUSED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""refused":"((?:[^"\\]|\\.)*)""#).unwrap());
/// A `deliver` receipt naming its board run: the release started and failed on the board, whose own
/// failure class counts it. All 8 `unclassified` calls of release retro 82097144209858832811 were
/// such receipts (failed tests, a typecheck), so none was an instruction error the needles missed.
static BOARD_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""release":"(\d{8}T\d{6}Z-[a-z-]+-\d+)""#).unwrap());

/// `(class, lowercase needles)`; EARLY needles read only the head of the output, since an
/// instruction error stops a command before it prints much.
const EARLY: [(&str, &[&str]); 6] = [
    ("guard_blocked", &["pretooluse:bash hook error", "blocked: "]),
    ("shell_syntax", &["unmatched '", "unmatched \"", "parse error near", "bad substitution", "substitution replacement not terminated"]),
    ("unknown_flag", &["unknown flag", "unknown option", "unknown argument", "unexpected argument", "unrecognized option",
        "unrecognized arguments", "flag provided but not defined", "invalid value for"]),
    ("missing_flag", &["needs --", "--migrate=<versions>", "rerun with --migrate=", "needs the changed web paths", "requires a value", "missing required",
        "the following required arguments"]),
    ("wrong_place", &["not a git repository", "script not found", "command not found", "no such file or directory",
        "is not an ancestor", "could not find package.json", "production is not delivered", ".xcodeproj' does not exist"]),
    ("usage", &["usage:"]),
];
/// xcodebuild prints these after its build settings, and `deliver beta-web` names a path it cannot find
/// at the end of a long ship-web log, so they are read anywhere.
const ANYWHERE: [(&str, &[&str]); 2] = [
    ("xcode_input", &["unable to find a destination matching", "unable to find a device matching", "does not contain a scheme", "is not currently configured for the",
        "isn't a member of the specified test plan or scheme", "cannot test target", "build.db\": database is locked", "pass -allowprovisioningupdates"]),
    // Retro 81623886145556566588: a typed web path that exists neither in the workspace nor on beta (session bf8e951e).
    ("wrong_place", &["is neither in the workspace nor in the serving source"]),
];
/// Build-input failures: a source file the project names is missing from the checkout (another
/// session's uncommitted move in the shared tree, 2026-10-28), not a command typed in the wrong place.
const NOT_INSTRUCTION: [&str; 1] = ["error opening input file"];
pub(crate) const CLASSES: [&str; 7] = ["guard_blocked", "shell_syntax", "unknown_flag", "missing_flag", "wrong_place", "usage", "xcode_input"];
pub(crate) const LINTS: [&str; 3] = ["ungated_build", "shared_derived_data", "off_board"];
/// Failure markers a zero exit can hide: agents pipe release commands through `tail`.
const FAILED: [&str; 8] = ["\"ok\":false", "\"refused\":", "** build failed **", "** test failed **", "** archive failed **",
    "xcodebuild: error:", "error: failed to solve", "error: (gcloud."];

/// The command's lines without heredoc bodies, which are data, not commands.
fn command_lines(command: &str) -> Vec<&str> {
    let (mut out, mut until) = (Vec::new(), None::<String>);
    for line in command.lines() {
        if let Some(tag) = &until { if line.trim() == tag { until = None; } continue; }
        if let Some(c) = HEREDOC.captures(line) { until = Some(c[1].to_string()); }
        out.push(line);
    }
    out
}

/// Shell segments of one line, split on `&;|()` outside quotes. Quoted text is data: a grep
/// pattern such as `"a|xcodebuild test"` or a commit message counted as a build (2026-10-28..31).
/// A `$(…)` inside double quotes is a command again.
fn segments(line: &str) -> Vec<&str> {
    let (mut out, mut start, mut stack, mut prev) = (Vec::new(), 0, Vec::<char>::new(), ' ');
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let mut cut = false;
        match (stack.last().copied(), c) {
            (Some('\''), '\'') | (Some('"'), '"') => { stack.pop(); }
            (Some('\''), _) => {}
            (Some('"'), '\\') => { chars.next(); }
            (Some('"'), '(') if prev == '$' => { stack.push('('); cut = true; }
            (Some('"'), _) => {}
            (_, '\'' | '"') => stack.push(c),
            (_, '(') => { if prev == '$' { stack.push('('); } cut = true; }
            (top, ')') => { if top == Some('(') { stack.pop(); } cut = true; }
            (_, '&' | ';' | '|') => cut = true,
            _ => {}
        }
        if cut { out.push(&line[start..i]); start = i + c.len_utf8(); }
        prev = c;
    }
    out.push(&line[start..]);
    out
}

#[cfg(test)]
pub(crate) fn entry_of(command: &str) -> Option<String> { release_segment(command).map(|(entry, _)| entry) }

/// The release entry and the shell segment that runs it, without wrappers.
pub(crate) fn release_segment(command: &str) -> Option<(String, String)> {
    command_lines(command).into_iter().flat_map(segments).find_map(|segment| {
        let mut s = segment.trim().trim_start_matches(['{', '!', ' ']);
        while let Some(m) = ASSIGN.find(s).or_else(|| GATE.find(s)) { s = &s[m.end()..]; }
        let entry = ENTRIES.iter().find_map(|(name, re)| re.captures(s).map(|c| c.get(1).map_or(name.to_string(), |m| format!("{name}{}", m.as_str()))))?;
        Some((entry, s.to_string()))
    })
}

/// The board run a failed `deliver` receipt names, when it got that far.
pub(crate) fn board_run(output: &str) -> Option<String> {
    BOARD_RUN.captures(output).map(|c| c[1].to_string())
}

/// A `deliver` refusal, which build warnings printed before it would otherwise hide.
pub(crate) fn refusal(output: &str) -> Option<String> {
    REFUSED.captures(output).map(|c| c[1].replace("\\\"", "\""))
}

pub(crate) fn lints(entry: &str, command: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    if matches!(entry, "xcodebuild" | "docker build") && !command.contains("cpu-build-gate") { out.push("ungated_build"); }
    if entry == "xcodebuild" && !command.contains("-derivedDataPath") { out.push("shared_derived_data"); }
    let off_board = matches!(entry, "k8s deploy" | "gcloud builds submit" | "testflight upload") || entry.starts_with("kubectl ")
        || (entry == "canary release" && !command.contains("--dry-run"));
    if off_board { out.push("off_board"); }
    out
}

/// The instruction-error class of a failed call's output, else `release_failed` for a receipt that
/// names its failed board run, else `other`. A `deliver` refusal is read by itself, since build
/// warnings may come before it.
pub(crate) fn classify(output: &str) -> &'static str {
    let lower = output.to_lowercase();
    if NOT_INSTRUCTION.iter().any(|n| lower.contains(n)) { return "other"; }
    let head: String = REFUSED.captures(&lower).map_or_else(|| lower.chars().take(EARLY_CHARS).collect(), |c| c[1].to_string());
    EARLY.iter().find(|(_, needles)| needles.iter().any(|n| head.contains(n)))
        .or_else(|| ANYWHERE.iter().find(|(_, needles)| needles.iter().any(|n| lower.contains(n))))
        .map_or_else(|| if board_run(output).is_some() { RELEASE_FAILED } else { "other" }, |(class, _)| class)
}

/// The class of a failed call that started its board run; not an instruction error.
pub(crate) const RELEASE_FAILED: &str = "release_failed";

/// `(failed, output)` of a tool result; `exit` is a code the harness reported elsewhere.
pub(crate) fn outcome(text: &str, error: bool, exit: Option<i64>) -> (bool, String) {
    let (code, output) = match serde_json::from_str::<Value>(text) {
        Ok(v) if v["metadata"]["exit_code"].is_i64() => (v["metadata"]["exit_code"].as_i64(), v["output"].as_str().unwrap_or("").to_string()),
        _ => (exit.or_else(|| EXIT.captures(text).and_then(|c| c[1].parse::<i64>().ok())), text.to_string()),
    };
    let lower = NESTED.iter().fold(output.to_lowercase(), |text, &(key, open, close)| without_nested(&text, key, open, close))
        .replace(NOTICE_FAILED, "");
    (error || code.is_some_and(|c| c != 0) || FAILED.iter().any(|m| lower.contains(m)), output)
}

/// `deliver` receipt members whose own `"ok":false` is not the release failing (lowercased keys):
/// `notified` entries report whether another session's tab took the release notice (a succeeded
/// `deliver canary` counted failed, release retro 84705495676337590512); `diskRecovery` reports
/// the disk check before an S0 cleanup the run then passed (succeeded canary 20261113T151338Z-canary-31669
/// counted failed, release retro 814732438991896643353).
const NESTED: [(&str, char, char); 2] = [("\"notified\":[", '[', ']'), ("\"diskrecovery\":{", '{', '}')];
/// A `notified` entry's own failure when `| tail` cut the `"notified":[` key off: only notice entries
/// put `"receipt"` right after `"ok"` (succeeded canary 98425bf895 and beta-crm 03593139fe counted failed,
/// release retro 810781605592223812110).
const NOTICE_FAILED: &str = "\"ok\":false,\"receipt\":";

/// The text without each `key` member's value, from `key` to its matching `close` (or the end of
/// a truncated tail).
fn without_nested(text: &str, key: &str, open: char, close: char) -> String {
    let (mut out, mut rest) = (String::with_capacity(text.len()), text);
    while let Some(at) = rest.find(key) {
        out.push_str(&rest[..at]);
        let body = &rest[at + key.len()..];
        let (mut depth, mut quoted, mut escaped, mut end) = (1usize, false, false, body.len());
        for (i, c) in body.char_indices() {
            match (quoted, escaped, c) {
                (true, true, _) => escaped = false,
                (true, false, '\\') => escaped = true,
                (true, false, '"') => quoted = false,
                (true, false, _) => {}
                (false, _, '"') => quoted = true,
                (false, _, c) if c == open => depth += 1,
                (false, _, c) if c == close => { depth -= 1; if depth == 0 { end = i + 1; break; } }
                _ => {}
            }
        }
        rest = &body[end..];
    }
    out.push_str(rest);
    out
}

/// The flags of a release segment; a count such as `-20` is no flag.
pub(crate) fn flags(segment: &str) -> Vec<String> {
    let mut out: Vec<String> = segment.split_whitespace().map(|w| w.trim_matches(['"', '\'']))
        .filter(|w| w.len() > 2 && w.starts_with('-') && w[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '-')).map(str::to_string).collect();
    out.sort();
    out.dedup();
    out
}

/// Codex `exec_command` JSON (`cmd`), `shell` JSON (`command` array) or `exec` JavaScript.
pub(crate) fn codex_command(payload: &Value) -> String {
    let raw = payload["input"].as_str().or(payload["arguments"].as_str()).unwrap_or("");
    if let Ok(args) = serde_json::from_str::<Value>(raw) {
        if let Some(cmd) = args["cmd"].as_str() { return cmd.to_string(); }
        if let Some(parts) = args["command"].as_array() { return parts.last().and_then(Value::as_str).unwrap_or("").to_string(); }
    }
    static JS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"cmd:\s*"((?:[^"\\]|\\.)*)""#).unwrap());
    JS.captures(raw).map_or(String::new(), |c| c[1].replace("\\\"", "\"").replace("\\n", "\n").replace("\\\\", "\\"))
}
