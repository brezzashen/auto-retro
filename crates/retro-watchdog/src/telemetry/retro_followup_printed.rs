//! Result-aware read probes (`"printed": true`, R11). Since 2026-10-25 `local-search query|search`
//! ends with the full text of its best hits as `==> path:start-end` blocks, so a read of a range it
//! has just printed is the redundant re-read that change targeted. A read of another range, another
//! file or a whole file (the transcript does not hold its length) is new information: counting every
//! read near a search kept R11 `ineffective` for 40 retros (conversation retro 813259229750867721848).
use regex::Regex;
use std::sync::LazyLock;

/// One printed or read span: path and inclusive line range.
pub(super) type Span = (String, u64, u64);

static PRINTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^==> (\S+?):(\d+)(?:-(\d+))?\s*$").unwrap());
static READ_TOOL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\{"file_path":"([^"]+)""#).unwrap());
static OFFSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""offset":(\d+)"#).unwrap());
static LIMIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""limit":(\d+)"#).unwrap());
static SED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"sed\s+-n\s+['"]?(\d+)(?:,(\d+))?p['"]?\s+['"]?([^\s'"|;&<>]+)"#).unwrap());
static PEEK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"peek(?:\.ts)?\s+([^\s:#|;&]+):(\d+)(?:-(\d+))?").unwrap());
/// The Read tool's default line count when no limit is given.
const READ_DEFAULT_LIMIT: u64 = 2000;

fn num(m: Option<regex::Match>) -> Option<u64> { m.and_then(|m| m.as_str().parse().ok()) }

/// Spans a local-search result printed in full.
pub(super) fn printed(result: &str) -> Vec<Span> {
    PRINTED.captures_iter(result).filter_map(|c| {
        let start = num(c.get(2))?;
        Some((c[1].to_string(), start, num(c.get(3)).unwrap_or(start)))
    }).collect()
}

/// The span a read call asks for: Read tool offset/limit, `sed -n a,bp file` or `peek file:a-b`.
pub(super) fn read(call: &str) -> Option<Span> {
    if let Some(c) = READ_TOOL.captures(call) {
        let start = num(OFFSET.captures(call).and_then(|o| o.get(1))).unwrap_or(1).max(1);
        let limit = num(LIMIT.captures(call).and_then(|l| l.get(1))).unwrap_or(READ_DEFAULT_LIMIT).max(1);
        return Some((c[1].to_string(), start, start + limit - 1));
    }
    if let Some(c) = SED.captures(call) {
        let start = num(c.get(1))?;
        return Some((c[3].to_string(), start, num(c.get(2)).unwrap_or(start)));
    }
    PEEK.captures(call).and_then(|c| {
        let start = num(c.get(2))?;
        Some((c[1].to_string(), start, num(c.get(3)).unwrap_or(start)))
    })
}

/// Last components of a path with `./`, `~/` and `$HOME/` dropped, so a relative read matches the
/// absolute path local-search printed (and either checkout root of the same repository).
fn tail(path: &str) -> Vec<&str> {
    let path = path.trim_start_matches("./").trim_start_matches("~/").trim_start_matches("$HOME/");
    path.split('/').filter(|p| !p.is_empty()).collect()
}

fn same_file(a: &str, b: &str) -> bool {
    let (a, b) = (tail(a), tail(b));
    let k = a.len().min(b.len()).min(3);
    k >= 1 && a[a.len() - k..] == b[b.len() - k..] && (k >= 2 || a.len() == b.len())
}

/// A read is redundant when at least half of its lines were printed for the same file.
pub(super) fn redundant(read: &Span, printed: &[Span]) -> bool {
    let len = read.2.saturating_sub(read.1) + 1;
    printed.iter().filter(|p| same_file(&read.0, &p.0)).any(|p| {
        let overlap = read.2.min(p.2).saturating_sub(read.1.max(p.1)) + u64::from(read.2.min(p.2) >= read.1.max(p.1));
        overlap * 2 >= len
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_printed_blocks_and_read_spans() {
        let out = "  /a/b/c.rs:1-21  [code/block] fn x\n\n==> /Users/x/automation/crates/w/src/f.rs:128-162\n  128\tfn f\n==> /Users/x/README.md:28\n";
        assert_eq!(printed(out), vec![("/Users/x/automation/crates/w/src/f.rs".into(), 128, 162), ("/Users/x/README.md".into(), 28, 28)]);
        assert_eq!(read(r#"{"file_path":"/p/q.rs","limit":30,"offset":130}"#), Some(("/p/q.rs".into(), 130, 159)));
        assert_eq!(read(r#"{"file_path":"/p/q.rs"}"#), Some(("/p/q.rs".into(), 1, 2000)));
        assert_eq!(read("cd ~/automation; sed -n '130,150p' crates/w/src/f.rs | cut -c1-200"), Some(("crates/w/src/f.rs".into(), 130, 150)));
        assert_eq!(read("bun scripts/utils/peek.ts src/f.rs:10-20"), Some(("src/f.rs".into(), 10, 20)));
        assert_eq!(read("cat -n src/f.rs"), None);
    }

    #[test]
    fn only_a_mostly_printed_range_of_the_same_file_is_redundant() {
        let shown = vec![("/Users/x/automation/crates/w/src/f.rs".to_string(), 128, 162)];
        assert!(redundant(&("crates/w/src/f.rs".into(), 130, 150), &shown));
        assert!(redundant(&("/Volumes/M/automation/crates/w/src/f.rs".into(), 120, 160), &shown));
        // Another range of the same file, a whole-file read and another file with the same name are new.
        assert!(!redundant(&("crates/w/src/f.rs".into(), 1, 60), &shown));
        assert!(!redundant(&("crates/w/src/f.rs".into(), 1, 2000), &shown));
        assert!(!redundant(&("crates/other/src/f.rs".into(), 130, 150), &shown));
        assert!(!redundant(&("f.rs".into(), 130, 150), &shown));
    }
}
