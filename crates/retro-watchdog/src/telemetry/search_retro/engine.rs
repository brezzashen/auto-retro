//! Engine side of the search quality measure: `local-search/queries.jsonl` rows inside the window
//! without `caller` (watchdog receipts excluded) and, unless `with_synthetic`, without a `synthetic`
//! label (`synthetic.rs`). Per subcommand: calls, hit rate (`hits` > 0),
//! the share within its engine-time budget and p50/p95 of embed + search + rerank + extras ms
//! (`extras_ms`, the sections printed after the hit list, logged since local-search 4c64541). `speed_rate`
//! is the share of all rows within budget; `slow` lists the rows over it, slowest first.
use super::quality::{percentile, ratio, FAILURES};
use serde_json::{json, Value};
use std::path::Path;

/// Engine time (embed + search + rerank + extras) one call should stay within (founder goal 2026-11-02).
pub(super) const SEARCH_BUDGET_MS: f64 = 500.0;
pub(super) const QUERY_BUDGET_MS: f64 = 2000.0;

fn ms_of(row: &Value) -> f64 {
    ["embed_ms", "search_ms", "rerank_ms", "extras_ms"].iter().map(|k| row[*k].as_f64().unwrap_or(0.0)).sum()
}

fn budget(sub: &str) -> f64 {
    if sub == "query" { QUERY_BUDGET_MS } else { SEARCH_BUDGET_MS }
}

pub(super) fn measure(home: &Path, from_ms: f64, to_ms: f64, with_synthetic: bool) -> Value {
    let text = std::fs::read_to_string(home.join(".local/share/local-search/queries.jsonl")).unwrap_or_default();
    let rows: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|r| r.get("caller").is_none() && (with_synthetic || super::synthetic::label(r).is_none()) && matches!(r["sub"].as_str(), Some("search" | "query")))
        .filter(|r| r["ts_ms"].as_f64().is_some_and(|t| t >= from_ms && t <= to_ms)).collect();
    let over = |r: &Value| ms_of(r) > budget(r["sub"].as_str().unwrap_or_default());
    let mut out = serde_json::Map::new();
    for sub in ["search", "query"] {
        let rows: Vec<&Value> = rows.iter().filter(|r| r["sub"] == sub).collect();
        let hits = rows.iter().filter(|r| r["hits"].as_u64().unwrap_or(0) > 0).count();
        let fast = rows.iter().filter(|r| !over(r)).count();
        let mut ms: Vec<f64> = rows.iter().map(|r| ms_of(r)).collect();
        out.insert(sub.into(), json!({"calls": rows.len(), "hit_rate": ratio(hits, rows.len()), "budget_ms": budget(sub),
            "within_budget_rate": ratio(fast, rows.len()), "p50_ms": percentile(&mut ms, 0.5), "p95_ms": percentile(&mut ms, 0.95)}));
    }
    let mut slow: Vec<&Value> = rows.iter().filter(|r| over(r)).collect();
    slow.sort_by(|a, b| ms_of(b).total_cmp(&ms_of(a)));
    out.insert("speed_rate".into(), ratio(rows.len() - slow.len(), rows.len()));
    out.insert("slow_calls".into(), json!(slow.len()));
    // Host perf incidents open meanwhile (`host_overlap.rs`, search-retro 812181474823629636591).
    let host = super::host_overlap::load(home, from_ms, to_ms);
    let host_of = |r: &Value| r["ts_ms"].as_f64().map_or(json!([]), |at| super::host_overlap::overlapping(&host, at, ms_of(r)));
    out.insert("slow_in_host_incident".into(), json!(slow.iter().filter(|r| host_of(r).as_array().is_some_and(|a| !a.is_empty())).count()));
    out.insert("slow".into(), json!(slow.iter().take(FAILURES).map(|r| { let mut row = json!({"sub": r["sub"], "ms": ms_of(r).round(), "embed_ms": r["embed_ms"], "search_ms": r["search_ms"], "extras_ms": r["extras_ms"], "corpus": r["corpus"],
        "route": r["route"], "stale_cache": r["stale_cache"], "ts_ms": r["ts_ms"],
        // Parts of extras_ms (local-search since search-retro 810449725933314328503); null on older rows.
        "extras_keyword_ms": r["extras_keyword_ms"], "extras_docs_ms": r["extras_docs_ms"], "extras_docs_cache_ms": r["extras_docs_cache_ms"],
        // Parts of search_ms (local-search since search-retro 812704372299363822041); null on older rows.
        "cache_ms": r["cache_ms"], "scan_ms": r["scan_ms"], "keyword_ms": r["keyword_ms"], "hits_ms": r["hits_ms"],
        // Lookups behind keyword_ms (local-search since search-retro 8894024098218094529).
        "keyword_calls": r["keyword_calls"],
        // Of a search row's search_ms: the literal filter's file reads (local-search since search-retro 816961722485483061920).
        "literal_ms": r["literal_ms"], "literal_files": r["literal_files"],
        // Process resource use (local-search since search-retro 89953860612474017944); null on older rows.
        // The NONE sweeps (git status, git ls-files, rg), local-search since search-retro 810495355461229102702.
        "sweep_ms": r["sweep_ms"],
        // Its two parallel parts (local-search since search-retro 85235413452762570019); null on older rows.
        "sweep_fresh_ms": r["sweep_fresh_ms"], "sweep_skipped_ms": r["sweep_skipped_ms"],
        // Steps of the fresh part (local-search since search-retro 813279914114122985562); null on older rows.
        "fresh_git_ms": r["fresh_git_ms"], "fresh_check_ms": r["fresh_check_ms"], "fresh_rg_ms": r["fresh_rg_ms"],
        // Each git child of fresh_git_ms, wall and own CPU (local-search since search-retro 813120864715573975968).
        "fresh_status_ms": r["fresh_status_ms"], "fresh_status_cpu_ms": r["fresh_status_cpu_ms"],
        "fresh_untracked_ms": r["fresh_untracked_ms"], "fresh_untracked_cpu_ms": r["fresh_untracked_cpu_ms"],
        "fresh_log_ms": r["fresh_log_ms"], "fresh_log_cpu_ms": r["fresh_log_cpu_ms"],
        // The rest of a search's extras_ms: upstream note, history tool calls, looser retry, side
        // sections, hit print (relax/side/print since search-retro 85878025330971494828); null on older rows.
        "upstream_ms": r["upstream_ms"], "tools_ms": r["tools_ms"], "relax_ms": r["relax_ms"], "side_ms": r["side_ms"], "print_ms": r["print_ms"],
        "rusage": r["rusage"], "cause": cause(r),
        // Sync steps running meanwhile (`sync_overlap.rs`, search-retro 814125901426638775162).
        "sync_steps": r["ts_ms"].as_f64().map_or(Value::Null, |at| super::sync_overlap::steps(home, at, ms_of(r))),
        "host_incidents": host_of(r),
        "query": r["query"].as_str().unwrap_or("").chars().take(120).collect::<String>()});
        // Memory reading and CPU samples around the row (`host_load.rs`, search-retro 85504050829602525700).
        row["host_load"] = r["ts_ms"].as_f64().map_or(Value::Null, |at| super::host_load::reading(&crate::monitor::root(home), at, ms_of(r)));
        // Page-ins, block reads and switches of each git child: a disk wait versus a run-queue wait
        // (local-search since search-retro 812751635617774196739); null on older rows.
        for k in GIT_CHILD_RUSAGE { row[k] = r[k].clone(); }
        row }).collect::<Vec<_>>()));
    Value::Object(out)
}

/// Rusage counts local-search logs per git child of a NONE sweep (`deadline::ChildKeys`).
const GIT_CHILD_RUSAGE: [&str; 12] = [
    "fresh_status_majflt", "fresh_status_inblock", "fresh_status_nvcsw", "fresh_status_nivcsw",
    "fresh_untracked_majflt", "fresh_untracked_inblock", "fresh_untracked_nvcsw", "fresh_untracked_nivcsw",
    "fresh_log_majflt", "fresh_log_inblock", "fresh_log_nvcsw", "fresh_log_nivcsw",
];

/// Involuntary switches a starved lookup reaches: eight warm code searches on 2026-11-04 made 13-110
/// (inblock 0, cpu 7-37 ms), so 200 is above any warm run.
const STARVED_SWITCHES: u64 = 200;

/// Page-ins below which a row's faults cannot explain an over-budget wait. Search-retro
/// 83485594338778809860: every code vector query pages in its vector file (13 within-budget rows,
/// 274-1868 ms, made 7.8k-43.4k major faults with inblock 0), and the window's 32- and 11-fault
/// searches spent 1451 and 576 ms while preempted 515 and 548 times.
const PAGE_IN_FAULTS: u64 = 1000;

/// Why an over-budget row was slow, from its process `rusage`: `compute` when CPU time covers at least
/// half the engine time, else `cpu_starved` when involuntary switches outnumber voluntary ones and
/// pass `STARVED_SWITCHES`, else `disk` when it read blocks or paged in PAGE_IN_FAULTS or more
/// (cold index pages), else `wait` (a lock or socket). Starvation is read before page-ins since
/// search-retro 83485594338778809860: all three slow rows read `disk` while two were preempted
/// 10002 and 515 times (CPU busy 17:00-17:20 SGT, and a concurrent 19.8 s watchdog query).
/// Null on rows logged before local-search recorded rusage (search-retro 89953860612474017944).
/// `none_sweep` comes first: a NONE whose child sweeps (`sweep_ms`: git status, git ls-files and rg in
/// child processes) took at least half the engine time; on rows older than `sweep_ms`, a search with no
/// hit whose `extras_ms` holds half the time. Their CPU is not in `RUSAGE_SELF`, so search-retro
/// 810495355461229102702 read `agy plugin` (561 ms, extras 561, own CPU 18.8 ms, host CPU 30 %) as
/// `cpu_starved`. `compute` adds the children's CPU (`children_cpu_ms`) when logged.
/// `upstream_note` comes before both when the upstream note's git calls (`upstream_ms`) took half the
/// engine time and no sweep part took longer: search-retro 85725950721375456226 read a 611 ms search
/// with 4 hits (`upstream_ms` 409.6, own CPU 84 ms) as `cpu_starved`.
/// `unindexed_sweep` comes before all of them: the rg that answers a scope the index holds no file
/// of (`unindexed_ms`, a git worktree or another checkout) took half the time, whether it printed
/// lines or not. Search-retro 81262765853708033592: 3 of 4 slow rows were worktree rg sweeps of
/// `Mars_CRM/ios` (1.4-2.9 s child CPU) read as `none_sweep`, and two of them had printed matches.
pub(super) fn cause(row: &Value) -> Value {
    let u = &row["rusage"];
    let Some(cpu) = u["cpu_ms"].as_f64() else { return Value::Null };
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let half = ms_of(row) / 2.0;
    let f = |k: &str| row[k].as_f64().unwrap_or(0.0);
    let upstream = f("upstream_ms") >= half && f("upstream_ms") >= f("sweep_fresh_ms").max(f("sweep_skipped_ms"));
    let sweep = match row["sweep_ms"].as_f64() {
        Some(s) => s >= half,
        None => row["sub"] == "search" && row["hits"].as_u64() == Some(0) && row["extras_ms"].as_f64().unwrap_or(0.0) >= half,
    };
    let cause = if f("unindexed_ms") >= half {
        "unindexed_sweep"
    } else if upstream {
        "upstream_note"
    } else if sweep {
        "none_sweep"
    } else if cpu + u["children_cpu_ms"].as_f64().unwrap_or(0.0) >= half {
        "compute"
    } else if n("nivcsw") > n("nvcsw") && n("nivcsw") >= STARVED_SWITCHES {
        "cpu_starved"
    } else if n("inblock") > 0 || n("majflt") >= PAGE_IN_FAULTS {
        "disk"
    } else {
        "wait"
    };
    json!(cause)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn a_slow_row_is_classified_from_its_rusage() {
        let row = |u: serde_json::Value| json!({"sub": "search", "search_ms": 4000.0, "extras_ms": 0.0, "rusage": u});
        assert_eq!(super::cause(&row(json!({"cpu_ms": 3500.0, "inblock": 0, "majflt": 0, "nvcsw": 1, "nivcsw": 0}))), "compute");
        assert_eq!(super::cause(&row(json!({"cpu_ms": 80.0, "inblock": 1200, "majflt": 3, "nvcsw": 900, "nivcsw": 40}))), "disk");
        assert_eq!(super::cause(&row(json!({"cpu_ms": 80.0, "inblock": 0, "majflt": 0, "nvcsw": 3, "nivcsw": 400}))), "cpu_starved");
        assert_eq!(super::cause(&row(json!({"cpu_ms": 20.0, "inblock": 0, "majflt": 0, "nvcsw": 30, "nivcsw": 2}))), "wait");
        // A warm run's switch counts (110 involuntary, 100 voluntary) are not starvation.
        assert_eq!(super::cause(&row(json!({"cpu_ms": 21.5, "inblock": 0, "majflt": 0, "nvcsw": 100, "nivcsw": 110}))), "wait");
        assert!(super::cause(&json!({"sub": "search", "search_ms": 900.0})).is_null());
    }

    #[test]
    fn preemption_outranks_the_page_ins_every_vector_query_makes() {
        // The three slow rows of search-retro 83485594338778809860.
        let q = json!({"sub": "query", "embed_ms": 367.8, "search_ms": 2232.0, "extras_ms": 370.2,
            "rusage": {"cpu_ms": 437.0, "inblock": 0, "majflt": 43349, "nivcsw": 10002, "nvcsw": 150}});
        assert_eq!(super::cause(&q), "cpu_starved");
        let s = json!({"sub": "search", "search_ms": 1451.7, "extras_ms": 348.7,
            "rusage": {"cpu_ms": 26.6, "inblock": 0, "majflt": 32, "nivcsw": 515, "nvcsw": 128}});
        assert_eq!(super::cause(&s), "cpu_starved");
        let w = json!({"sub": "search", "search_ms": 75.7, "extras_ms": 576.3,
            "rusage": {"cpu_ms": 70.3, "inblock": 0, "majflt": 11, "nivcsw": 548, "nvcsw": 1347}});
        assert_eq!(super::cause(&w), "wait");
        let cold = json!({"sub": "query", "search_ms": 3000.0, "rusage": {"cpu_ms": 300.0, "inblock": 0, "majflt": 43000, "nivcsw": 90, "nvcsw": 400}});
        assert_eq!(super::cause(&cold), "disk");
    }

    #[test]
    fn time_in_the_upstream_note_is_named_before_starvation_or_the_sweep() {
        // The three slow rows of search-retro 85725950721375456226 (queries.jsonl ts 1794522419339,
        // 1794513861770, 1794508074705).
        let hit = json!({"sub": "search", "hits": 4, "search_ms": 106.9, "extras_ms": 504.0, "upstream_ms": 409.6,
            "rusage": {"cpu_ms": 84.2, "children_cpu_ms": 19.7, "inblock": 0, "majflt": 0, "nivcsw": 1324, "nvcsw": 276}});
        assert_eq!(super::cause(&hit), "upstream_note");
        let none = json!({"sub": "search", "hits": 0, "search_ms": 38.3, "extras_ms": 480.4, "sweep_ms": 402.3,
            "sweep_fresh_ms": 203.6, "sweep_skipped_ms": 216.4, "upstream_ms": 401.7,
            "rusage": {"cpu_ms": 50.8, "children_cpu_ms": 126.7, "inblock": 0, "majflt": 0, "nivcsw": 1089, "nvcsw": 424}});
        assert_eq!(super::cause(&none), "upstream_note");
        // A fresh sweep slower than the note stays a sweep.
        let fresh = json!({"sub": "search", "hits": 7, "search_ms": 7.7, "extras_ms": 503.2, "sweep_ms": 419.9,
            "sweep_fresh_ms": 419.8, "sweep_skipped_ms": 208.2, "upstream_ms": 415.4,
            "rusage": {"cpu_ms": 87.3, "children_cpu_ms": 332.9, "inblock": 0, "majflt": 0, "nivcsw": 1291, "nvcsw": 129}});
        assert_eq!(super::cause(&fresh), "none_sweep");
    }

    #[test]
    fn a_none_spent_in_its_child_sweeps_is_not_starvation() {
        // Search-retro 810495355461229102702, queries.jsonl:25562 (`agy plugin`, host CPU 30 %).
        let u = json!({"cpu_ms": 18.8, "inblock": 0, "majflt": 0, "nivcsw": 213, "nvcsw": 25});
        let old = json!({"sub": "search", "hits": 0, "search_ms": 2.7, "extras_ms": 561.0, "rusage": u});
        assert_eq!(super::cause(&old), "none_sweep");
        let new = json!({"sub": "search", "hits": 0, "search_ms": 2.7, "extras_ms": 561.0, "sweep_ms": 540.0, "rusage": u});
        assert_eq!(super::cause(&new), "none_sweep");
        // A hit's extras are other-corpus lookups in process, and a short sweep leaves the old rules.
        let hit = json!({"sub": "search", "hits": 3, "search_ms": 2.7, "extras_ms": 561.0, "rusage": u});
        assert_eq!(super::cause(&hit), "cpu_starved");
        let short = json!({"sub": "search", "hits": 0, "search_ms": 2.7, "extras_ms": 561.0, "sweep_ms": 90.0, "rusage": u});
        assert_eq!(super::cause(&short), "cpu_starved");
        // Children's CPU counts as compute.
        let kids = json!({"sub": "search", "hits": 2, "search_ms": 600.0, "extras_ms": 0.0,
            "rusage": {"cpu_ms": 20.0, "children_cpu_ms": 400.0, "inblock": 0, "majflt": 0, "nivcsw": 300, "nvcsw": 10}});
        assert_eq!(super::cause(&kids), "compute");
    }

    #[test]
    fn an_unindexed_scope_rg_is_named_whether_it_printed_lines_or_not() {
        // Search-retro 81262765853708033592: worktree `--path Mars_CRM/ios` sweeps (queries.jsonl
        // ts 1794990450835 printed two matches, ts 1794988729481 printed NONE).
        let u = json!({"cpu_ms": 28.2, "children_cpu_ms": 1475.2, "inblock": 0, "majflt": 1, "nivcsw": 995, "nvcsw": 66});
        let hit = json!({"sub": "search", "hits": 2, "search_ms": 33.5, "extras_ms": 866.8, "unindexed_ms": 860.0, "rusage": u});
        assert_eq!(super::cause(&hit), "unindexed_sweep");
        let none = json!({"sub": "search", "hits": 0, "search_ms": 4.5, "extras_ms": 529.2, "unindexed_ms": 520.0, "rusage": u});
        assert_eq!(super::cause(&none), "unindexed_sweep");
        // A short unindexed rg leaves the other rules.
        let short = json!({"sub": "search", "hits": 2, "search_ms": 600.0, "extras_ms": 40.0, "unindexed_ms": 35.0, "rusage": u});
        assert_eq!(super::cause(&short), "compute");
    }
}
