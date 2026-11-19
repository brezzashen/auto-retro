//! Evidence snapshot of the vector databases, taken on the watch's worker thread (never in the loop):
//! the file measure, `local-search stats` per corpus, sqlite page and freelist counts per database,
//! query latency per corpus over the last LATENCY_DAYS from `queries.jsonl`, and the count of
//! `database is locked` lines in the sync logs. Each part records its own error instead of failing.
use serde_json::{json, Map, Value};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const STATS_TIMEOUT: Duration = Duration::from_secs(600);
const SQLITE_TIMEOUT: Duration = Duration::from_secs(60);
const LATENCY_DAYS: f64 = 7.0;
const METRICS: [&str; 4] = ["search_ms", "scan_ms", "cache_ms", "embed_ms"];
const SYNC_LOGS: [&str; 2] = ["sync.log", "code-sync.log"];

pub(in crate::telemetry) fn snapshot(home: &Path) -> Value {
    let dir = super::index_dir(home);
    let measure = super::measure(&dir);
    let now_ms = crate::unix_now() * 1000.0;
    let stats = run(Command::new(home.join(".local/bin/local-search")).arg("stats"), STATS_TIMEOUT)
        .map(|out| parse_stats(&out)).unwrap_or_else(|e| json!({"error": e}));
    let mut pages = Map::new();
    for db in measure["dbs"].as_object().into_iter().flatten().map(|(db, _)| db) {
        let out = run(Command::new("/usr/bin/sqlite3").args(["-cmd", "PRAGMA query_only=1"]).arg(dir.join(db))
            .arg("pragma page_size; pragma page_count; pragma freelist_count;"), SQLITE_TIMEOUT);
        pages.insert(db.clone(), out.map(|o| parse_pages(&o)).unwrap_or_else(|e| json!({"error": e})));
    }
    let queries = std::fs::read_to_string(dir.join("queries.jsonl")).unwrap_or_default();
    let logs = home.join("Library/Logs/local-search");
    let locks: Map<String, Value> = SYNC_LOGS.iter().map(|name| {
        let text = std::fs::read(logs.join(name)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        (name.to_string(), json!(text.matches("database is locked").count()))
    }).collect();
    json!({"at_ms": now_ms, "measure": measure, "stats": stats, "pages": pages,
        "latency": latency(&queries, now_ms - LATENCY_DAYS * 86_400_000.0, now_ms), "latency_days": LATENCY_DAYS,
        "lock_failures": locks})
}

/// Runs a command with a deadline and returns its stdout, or the reason it failed.
fn run(cmd: &mut Command, timeout: Duration) -> Result<String, String> {
    let mut child = cmd.env("PATH", super::super::MARS_SCRIPT_PATH).stdin(Stdio::null()).stdout(Stdio::piped())
        .stderr(Stdio::null()).spawn().map_err(|e| format!("spawn: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    };
    let out = reader.join().unwrap_or_default();
    if status.success() { Ok(out) } else { Err(format!("exit {:?}", status.code())) }
}

/// `local-search stats` rows: `{corpora: {corpus: {sources, chunks, chars}}, vectors, dbs: [line]}`.
pub(super) fn parse_stats(out: &str) -> Value {
    let mut corpora = Map::new();
    let (mut vectors, mut dbs) = (Value::Null, Vec::new());
    for line in out.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        match cols.as_slice() {
            ["corpus", ..] => {}
            ["vectors", ..] => vectors = json!(line.trim_start_matches("vectors").trim()),
            ["db", ..] => dbs.push(json!(line.trim_start_matches("db").trim())),
            [corpus, sources, chunks, chars] => {
                if let (Ok(s), Ok(c), Ok(t)) = (sources.parse::<u64>(), chunks.parse::<u64>(), chars.parse::<u64>()) {
                    corpora.insert(corpus.to_string(), json!({"sources": s, "chunks": c, "chars": t}));
                }
            }
            _ => {}
        }
    }
    json!({"corpora": corpora, "vectors": vectors, "dbs": dbs})
}

/// Three pragma lines: page size, page count, freelist count.
pub(super) fn parse_pages(out: &str) -> Value {
    let n: Vec<u64> = out.lines().filter_map(|l| l.trim().parse().ok()).collect();
    let [size, count, free] = n[..] else { return json!({"error": format!("unexpected pragma output: {out:?}")}) };
    json!({"page_size": size, "page_count": count, "freelist_count": free, "free_bytes": size * free})
}

/// p50/p95 of each metric per `corpus/sub` for log lines with `from <= ts_ms <= to`.
pub(super) fn latency(jsonl: &str, from: f64, to: f64) -> Value {
    let mut groups: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    for line in jsonl.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let ts = row["ts_ms"].as_f64().unwrap_or(0.0);
        if ts < from || ts > to {
            continue;
        }
        let key = format!("{}/{}", row["corpus"].as_str().unwrap_or("all"), row["sub"].as_str().unwrap_or("?"));
        groups.entry(key).or_default().push(row);
    }
    let mut out = Map::new();
    for (key, rows) in groups {
        let mut entry = Map::new();
        entry.insert("n".into(), json!(rows.len()));
        for metric in METRICS {
            let mut v: Vec<f64> = rows.iter().filter_map(|r| r[metric].as_f64()).collect();
            if v.is_empty() {
                continue;
            }
            v.sort_by(f64::total_cmp);
            let pick = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
            entry.insert(metric.into(), json!({"p50": pick(0.5), "p95": pick(0.95)}));
        }
        out.insert(key, Value::Object(entry));
    }
    Value::Object(out)
}
