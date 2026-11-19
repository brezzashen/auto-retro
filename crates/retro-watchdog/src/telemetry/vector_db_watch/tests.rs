use super::*;
use serde_json::json;

const GIB: u64 = 1024 * 1024 * 1024;

fn sized(total: u64) -> Value {
    json!({"total": total, "files": {"index.db": total}, "dbs": {"index.db": total}, "wal": {}})
}

#[test]
fn counts_databases_and_vector_sidecars_but_not_wal_locks_or_clones() {
    assert_eq!(counted("index.db"), Some("index.db"));
    assert_eq!(counted("index-dev.db"), Some("index-dev.db"));
    assert_eq!(counted("index.db.qwen3-embedding-4b.watchdog.q8.rows"), Some("index.db"));
    assert_eq!(counted("index-dev.db.qwen3-embedding-4b.code.q8.idx"), Some("index-dev.db"));
    for skipped in ["index.db-wal", "index.db-shm", "index.db.sync.code.lock", "index.db.qwen3-embedding-4b.code.q8.lock",
        "queries.jsonl", "graph.json", "clone.db", "index.db.tmp"] {
        assert_eq!(counted(skipped), None, "{skipped}");
    }
    let dir = crate::telemetry::test_home("vector-db-measure");
    std::fs::write(dir.join("index.db"), vec![0u8; 100]).unwrap();
    std::fs::write(dir.join("index.db-wal"), vec![0u8; 1000]).unwrap();
    std::fs::write(dir.join("index.db.m.watchdog.q8.rows"), vec![0u8; 10]).unwrap();
    std::fs::write(dir.join("index-dev.db"), vec![0u8; 7]).unwrap();
    let m = measure(&dir);
    assert_eq!(m["total"], 117);
    assert_eq!(m["dbs"]["index.db"], 110);
    assert_eq!(m["dbs"]["index-dev.db"], 7);
    assert_eq!(m["wal"]["index.db-wal"], 1000);
}

#[test]
fn install_sets_the_baseline_without_a_retro_and_five_gib_of_net_growth_opens_one() {
    let mut watch = VectorDbWatch::default();
    assert_eq!(watch.step(&sized(20 * GIB), 0.0, None), Some("baseline"));
    assert_eq!(watch.baseline["bytes"], 20 * GIB);
    // Until the install snapshot is stored (a restart lost it), each sample asks for it again.
    assert_eq!(watch.step(&sized(20 * GIB), 0.5, None), Some("baseline"));
    watch.baseline["snapshot"] = json!({"measure": sized(20 * GIB)});
    assert_eq!(watch.step(&sized(24 * GIB), 1.0, None), None);
    // A compact lowers the baseline, so the next retro needs 5 GiB over the smaller size.
    assert_eq!(watch.step(&sized(14 * GIB), 2.0, None), None);
    assert_eq!(watch.baseline["bytes"], 14 * GIB);
    assert_eq!(watch.step(&sized(19 * GIB - 1), 3.0, None), None);
    assert_eq!(watch.step(&sized(19 * GIB), 4.0, None), Some("crossing"));
}

#[test]
fn an_open_retro_or_the_daily_cap_blocks_the_next_one() {
    let mut watch = VectorDbWatch::default();
    watch.step(&sized(10 * GIB), 0.0, None);
    assert_eq!(watch.step(&sized(16 * GIB), 1.0, Some("8123".into())), None);
    assert_eq!(watch.blocked.as_ref().unwrap()["by"], "8123");
    for at in [10.0, 20.0] {
        watch.dispatches.push(json!({"incident": "x", "at_ms": at, "started": true}));
    }
    assert_eq!(watch.step(&sized(16 * GIB), 30.0, None), None);
    assert_eq!(watch.blocked.as_ref().unwrap()["by"], "daily_cap");
    assert_eq!(watch.step(&sized(16 * GIB), 86_400_000.0 + 20.0, None), Some("crossing"));
}

#[test]
fn a_crossing_snapshot_opens_one_incident_and_becomes_the_next_baseline() {
    let home = crate::telemetry::test_home("vector-db-watch");
    let mut watch = VectorDbWatch::default();
    watch.step(&sized(10 * GIB), 0.0, None);
    let snapshot = json!({"measure": sized(16 * GIB), "stats": {}});
    watch.absorb(&home, "crossing", snapshot, 1000.0).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
    assert_eq!(watch.baseline["bytes"], 16 * GIB);
    let id = watch.dispatches[0]["incident"].as_str().unwrap().to_string();
    let incident: Value = serde_json::from_slice(&std::fs::read(root(&home).join("incidents").join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(incident["trigger"], "vector_db_growth_reached");
    assert_eq!(incident["growth_bytes"], 6 * GIB);
    assert_eq!(incident["growth_by_db"][0]["delta"], (6 * GIB) as i64);
    // Unacknowledged, it blocks the next crossing.
    assert_eq!(watch.open_incident(&home), Some(id.clone()));
    std::fs::write(root(&home).join("incidents").join(format!("{id}.ack")), "").unwrap();
    assert_eq!(watch.open_incident(&home), None);
    // A snapshot that fell back under the threshold opens nothing.
    watch.absorb(&home, "crossing", json!({"measure": sized(17 * GIB)}), 2000.0).unwrap();
    assert_eq!(watch.dispatches.len(), 1);
}

#[test]
fn evidence_parsers_read_stats_pragmas_and_latency() {
    let stats = evidence::parse_stats("corpus    sources    chunks        chars\nwatchdog   310083   4246557   3336174328\nvectors  4482155 chunks embedded, 332 pending (m)\ndb /x/index.db (13830 MB, watchdog, perf)\n");
    assert_eq!(stats["corpora"]["watchdog"]["chunks"], 4246557);
    assert_eq!(stats["dbs"][0], "/x/index.db (13830 MB, watchdog, perf)");
    let pages = evidence::parse_pages("4096\n3376703\n39\n");
    assert_eq!(pages["free_bytes"], 4096 * 39);
    let log = [r#"{"corpus":"watchdog","sub":"query","ts_ms":10,"search_ms":100.0}"#,
        r#"{"corpus":"watchdog","sub":"query","ts_ms":20,"search_ms":300.0}"#,
        r#"{"corpus":"code","sub":"search","ts_ms":5,"search_ms":1.0}"#].join("\n");
    let lat = evidence::latency(&log, 8.0, 30.0);
    assert_eq!(lat["watchdog/query"]["n"], 2);
    assert_eq!(lat["watchdog/query"]["search_ms"]["p95"], 300.0);
    assert!(lat.get("code/search").is_none());
}

#[test]
fn the_ack_gate_needs_attribution_split_decision_performance_and_measured_optimizations() {
    let mut report = json!({"report_url": "https://reports.example.com/x",
        "growth": [{"scope": "watchdog", "bytes": 5_000_000_000u64, "cause": "30-day log retention filled"}],
        "split": {"verdict": "keep", "reason": "index.db write lock shows no contention in sync.log"},
        "performance": {"verdict": "ok", "evidence": "watchdog/query search_ms p95 160 ms, unchanged"},
        "optimizations": [{"action": "page size 8192", "verdict": "proposed", "reason": "saves 2 GiB of slack"}]});
    validate::check(&report).unwrap();
    report["optimizations"][0]["verdict"] = json!("applied");
    assert!(validate::check(&report).is_err(), "applied needs before, after and recall");
    report["optimizations"][0]["before"] = json!("13.8 GB");
    report["optimizations"][0]["after"] = json!("11.6 GB");
    report["optimizations"][0]["recall"] = json!("golden sets watchdog Top5 unchanged");
    validate::check(&report).unwrap();
    report["split"]["verdict"] = json!("propose");
    assert!(validate::check(&report).is_err(), "a proposal names its plan");
    report["split"]["plan"] = json!("move perf to index-perf.db; recall parity via golden sets");
    validate::check(&report).unwrap();
    report["growth"] = json!([]);
    assert!(validate::check(&report).is_err());
}
