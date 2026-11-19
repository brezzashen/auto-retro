//! Action signatures of one tool call: what the call did, stripped of the values that change from
//! session to session, so equal work done by different sessions (or ten times in one) compares
//! equal. A call yields several signatures: each shell segment's program and subcommand
//! (`git status`, `bun read.ts`, `bun spec.ts set`), each file it reads (`file:report/SKILL.md`),
//! each URL host it touches (`url:reports.example.com`), and for an inline script the APIs it calls
//! (`bun -e`, `api:subtle.decrypt`), so hand-written decryption counts however it is spelled.
use regex::{Captures, Regex};
use serde_json::Value;
use std::{collections::HashMap, sync::LazyLock};

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://([A-Za-z0-9.-]+)").unwrap());
static INLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(bun|node|deno|tsx|python3?|ruby|perl)\s+(?:-e|-c|--eval|-)(?:\s|$)").unwrap());
static API: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)+)\s*\(").unwrap());
static HEREDOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<<-?\s*['"]?([A-Za-z_]+)['"]?"#).unwrap());
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4,}").unwrap());
static CODEX_CMD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\bcmd\s*:\s*"((?:[^"\\]|\\.)*)""#).unwrap());
/// A Codex code-mode cell of plain JavaScript that runs no shell command (`const a = load("x"); text(a)`,
/// `await tools.write_stdin(...)`): it signed as program `const` (79 calls in 10 sessions, required in
/// conversation retro 87474079431764110852) although each cell was a different device step.
static JS_CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?://[^\n]*\n\s*)*(?:const|var|await)\s|^\s*(?://[^\n]*\n\s*)*let\s+[A-Za-z_$][\w$]*\s*=\s").unwrap());
/// A whole statement that sets a shell variable: `S=/x/demo-session.ts`, `export C="$S/capture.js"`.
static ASSIGN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*(?:export\s+)?([A-Za-z_]\w*)=(?:"([^"\s]*)"|'([^'\s]*)'|([^\s;&|"'`()]+))\s*$"#).unwrap());
static VAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{?([A-Za-z_]\w*)\}?").unwrap());
/// One leading `NAME=value` word of a statement, the value quoted or bare; `NAME=$(` and NAME=`
/// stop at the substitution so the command inside it is what the statement runs.
static LEADING_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(?:export\s+)?[A-Za-z_]\w*=(?:\$\(|`|(?:"(?:[^"\\]|\\.)*"|'[^']*'|[^\s"'`]+)*(?:\s+|$))"#).unwrap()
});

const INTERPRETERS: [&str; 5] = ["bun", "node", "deno", "tsx", "python3"];
/// Programs whose first word names the action (`git status`, `kubectl get`).
const SUBCOMMANDS: [&str; 22] = ["git", "gh", "kubectl", "gcloud", "docker", "orca", "asc", "xcrun", "brew", "launchctl", "npm",
    "pnpm", "cargo", "bunx", "npx", "cpu-watchdog", "local-search", "limit-resume", "agent-browser", "browser-lease", "cpu-build-gate", "ssh"];
const READERS: [&str; 7] = ["cat", "sed", "head", "tail", "nl", "less", "wc"];
/// Shell builtins and keywords; `break` was the second-ranked "repeat" of conversation retro
/// 81740295292268599208 (307 calls) although it only ends polling loops.
const SKIP: [&str; 31] = ["cd", "echo", "printf", "true", "false", "sleep", "export", "set", "then", "fi", "do", "done", "else",
    "if", "for", "while", "exit", "test", "[", "[[", "{", "}", "local", "return", "break", "continue", "elif", "until", "case", "esac", "!"];
const PREFIXES: [&str; 7] = ["sudo", "time", "timeout", "env", "command", "exec", "nohup"];
/// Method names too common to say anything about the work of an inline script.
const COMMON: [&str; 40] = ["map", "filter", "forEach", "reduce", "join", "split", "slice", "push", "trim", "replace", "includes",
    "startsWith", "endsWith", "toString", "find", "some", "every", "keys", "values", "entries", "sort", "then", "catch", "log",
    "error", "warn", "parse", "stringify", "test", "match", "exec", "get", "set", "has", "add", "indexOf", "at", "toFixed", "from", "exit"];

/// Signatures of one call's text (a shell command, or a tool input as JSON), each once.
pub(crate) fn signatures(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(input)) => {
            let field = |k: &str| input.get(k).and_then(Value::as_str);
            if let Some(cmd) = field("cmd").or_else(|| field("command")) { shell(cmd, &mut out); }
            else if let Some(cmd) = input.get("command").and_then(Value::as_array).and_then(|a| a.last()).and_then(Value::as_str) { shell(cmd, &mut out); }
            else if let Some(skill) = field("skill") { out.push(format!("skill:{skill}")); }
            // An Edit, Write or MultiEdit input authors the file: 10 of the 13 file: signatures in
            // retro-necessary.json and 3 of 6 required in conversation retro 817759638105279248955 were
            // one session editing the file it was writing, judged necessary one retro at a time.
            else if ["new_string", "content", "edits", "new_source"].iter().any(|k| input.contains_key(*k)) {}
            else if let Some(path) = field("file_path").or_else(|| field("notebook_path")) { out.push(format!("file:{}", short_path(path))); }
            else if let Some(url) = field("url") { shell(url, &mut out); }
        }
        Ok(_) => {}
        Err(_) if text.contains("tools.exec_command") => {
            for cmd in CODEX_CMD.captures_iter(text) { shell(&cmd[1].replace("\\\"", "\"").replace("\\n", "\n"), &mut out); }
        }
        Err(_) if text.starts_with("*** Begin Patch") => {}
        Err(_) if JS_CELL.is_match(text) => { out.push("codex js".to_string()); apis(text, &mut out); }
        Err(_) => shell(text, &mut out),
    }
    let mut seen = Vec::new();
    for sig in out {
        let sig: String = DIGITS.replace_all(sig.trim(), "#").chars().take(80).collect();
        if !sig.is_empty() && !seen.contains(&sig) { seen.push(sig); }
    }
    seen
}

/// The object methods an inline script or code cell calls (`api:subtle.decrypt`), common ones left out.
fn apis(text: &str, out: &mut Vec<String>) {
    for api in API.captures_iter(text) {
        let parts: Vec<&str> = api[1].split('.').collect();
        let method = parts[parts.len() - 1];
        if !COMMON.contains(&method) { out.push(format!("api:{}.{method}", parts[parts.len() - 2])); }
    }
}

fn shell(text: &str, out: &mut Vec<String>) {
    for host in URL.captures_iter(text) { out.push(format!("url:{}", &host[1])); }
    let head = match INLINE.captures(text) {
        Some(inline) => {
            let whole = inline.get(0).unwrap();
            out.push(format!("{} -e", &inline[1]));
            apis(&text[whole.end()..], out);
            &text[..whole.start()]
        }
        None => text,
    };
    let head = expand_vars(&without_heredocs(head));
    out.extend(statements(&head).into_iter().filter_map(segment));
}

/// Variables set earlier in the same call, substituted where they are used: `S=/x/demo-session.ts;
/// bun $S request` signs as `bun demo-session.ts request`, not `bun` (1,285 bare `bun` calls in
/// conversation retro 81661669974698698044 were mostly `bun $S request` with S naming demo-session.ts).
fn expand_vars(text: &str) -> String {
    let mut vars: HashMap<String, String> = HashMap::new();
    for statement in statements(text) {
        let Some(c) = ASSIGN.captures(statement) else { continue };
        let value = c.get(2).or(c.get(3)).or(c.get(4)).map_or("", |m| m.as_str());
        let value = VAR.replace_all(value, |v: &Captures| vars.get(&v[1]).cloned().unwrap_or_else(|| v[0].to_string())).into_owned();
        vars.insert(c[1].to_string(), value);
    }
    if vars.is_empty() { return text.to_string(); }
    VAR.replace_all(text, |v: &Captures| vars.get(&v[1]).cloned().unwrap_or_else(|| v[0].to_string())).into_owned()
}

/// Shell statements split at newlines, `;`, `&&` and `||` outside quotes, so a quoted multi-line
/// argument (a `perl -e` or `sed` program) stays one statement.
fn statements(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let (mut out, mut start, mut quote, mut i) = (Vec::new(), 0, None::<u8>, 0);
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(b'"') if c == b'\\' => i += 1,
            Some(_) => {}
            None => match c {
                b'\\' => i += 1,
                b'\'' | b'"' => quote = Some(c),
                b'\n' | b';' => { out.push(&text[start..i]); start = i + 1; }
                b'&' | b'|' if bytes.get(i + 1) == Some(&c) => { out.push(&text[start..i]); start = i + 2; i += 1; }
                _ => {}
            },
        }
        i += 1;
    }
    out.push(&text[start.min(text.len())..]);
    out
}

/// The command lines of `text`, without heredoc bodies (file contents, not actions).
fn without_heredocs(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(doc) = HEREDOC.captures(rest) {
        let line_end = rest[doc.get(0).unwrap().end()..].find('\n').map_or(rest.len(), |i| doc.get(0).unwrap().end() + i);
        out.push_str(&rest[..line_end]);
        let body = &rest[line_end..];
        let close = body.split_inclusive('\n').scan(0, |at, line| { *at += line.len(); Some((*at, line.trim() == &doc[1])) }).find(|l| l.1);
        rest = close.map_or("", |(at, _)| &body[at..]);
        out.push('\n');
    }
    out + rest
}

/// `text` without its leading assignments. A quoted value with spaces is one word: split at
/// whitespace, `UA='Mozilla/5.0 (Macintosh; Intel Mac OS X)'` signed as the program `Macintosh;`
/// (42 calls, a required deep dive of conversation retro 87103953672303196097).
fn after_assignments(mut text: &str) -> &str {
    while let Some(found) = LEADING_ASSIGN.find(text).filter(|found| found.end() > 0) {
        text = &text[found.end()..];
    }
    text
}

fn segment(text: &str) -> Option<String> {
    let text = after_assignments(text);
    // Only a pipeline's first program acts; the rest filter its output.
    let text = text.split('|').next().unwrap_or(text);
    let mut tokens = text.split_whitespace().map(|t| t.trim_matches(['\'', '"', '(', ')', '`']))
        .skip_while(|t| PREFIXES.contains(t) || t.starts_with(|c: char| c.is_ascii_digit()) || (t.contains('=') && !t.starts_with('-')));
    let first = tokens.next()?;
    let program = first.rsplit('/').next().unwrap_or(first);
    if SKIP.contains(&program) || !program.starts_with(|c: char| c.is_ascii_alphanumeric()) { return None; }
    let rest: Vec<&str> = tokens.filter(|t| !t.starts_with('-') && !t.is_empty()).collect();
    let word = |i: usize| rest.get(i).copied().filter(|w| is_word(w));
    Some(match program {
        p if INTERPRETERS.contains(&p) => match rest.first().copied() {
            Some(sub @ ("run" | "test" | "x")) => format!("{p} {sub} {}", rest.get(1).map_or("", |s| s.rsplit('/').next().unwrap_or(s))),
            Some(script) if script.contains('.') || script.contains('/') => {
                format!("{p} {} {}", script.rsplit('/').next().unwrap_or(script), word(1).unwrap_or(""))
            }
            _ => p.to_string(),
        },
        p if SUBCOMMANDS.contains(&p) => format!("{p} {}", rest.first().copied().filter(|w| is_word(w) || p == "ssh").unwrap_or("")),
        p if READERS.contains(&p) => rest.iter().rev().find(|t| t.contains('/') || t.contains('.'))
            .filter(|t| !t.starts_with(|c: char| c.is_ascii_digit()))
            .map_or_else(|| p.to_string(), |path| format!("file:{}", short_path(path))),
        // A raw fetch names its host: `curl reports.example.com` is reading a report page by hand.
        p @ ("curl" | "wget") => URL.captures(text).map_or_else(|| p.to_string(), |url| format!("{p} {}", &url[1])),
        p => p.to_string(),
    })
}

fn is_word(text: &str) -> bool {
    !text.is_empty() && text.len() <= 24 && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The last two components, so the same file read from different checkouts compares equal.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.trim_end_matches('/').rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}
