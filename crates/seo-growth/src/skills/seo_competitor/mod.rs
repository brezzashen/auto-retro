//! Ports of `.claude/skills/seo-competitor/scripts/*.ts`. The CLI entries are `collect`,
//! `analyze`, `mrrEstimate`, `report`, the hourly `watch`, the search-engine `push`, `localeWords` and the adversarial `advisor`; the other files are library modules they share.

mod aisa;
mod analyze;
mod analyze_gsc;
mod analyze_keywords;
mod analyze_site;
mod cards;
mod charts;
mod collect;
mod config;
mod dashboard;
mod estimator;
pub(crate) mod files;
mod format;
mod gsc;
mod metrics;
mod monid;
mod mrr_estimate;
mod push;
mod push_bing;
mod push_heads;
mod push_config;
mod push_engines;
mod push_report;
mod push_site;
mod report;
mod self_case;
mod watch;
mod watch_advisor;
mod watch_advisor_pack;
mod watch_analyze;
mod watch_anomalies;
mod watch_machine_queries;
mod watch_page_cliff;
mod watch_baidu;
mod watch_bing;
mod watch_board;
mod watch_config;
mod watch_copy_cap;
mod watch_link_gap;
mod watch_new_page_grace;
mod watch_decisions;
mod watch_discover;
mod watch_fingerprints;
mod watch_hour;
pub(crate) mod watch_hour_db;
mod watch_hour_followup;
mod watch_hour_digest;
#[cfg(test)]
mod watch_hour_digest_tests;
mod watch_aisa;
mod watch_aisa_parse;
mod watch_aisa_view;
mod watch_creator;
mod watch_ads;
mod watch_ads_retro;
mod cpc_pool;
mod cpc_pool_cli;
mod cpc_pool_expand;
mod cpc_pool_judge;
mod cpc_pool_preview;
mod cpc_pool_view;
mod watch_creator_parse;
mod watch_creator_view;
mod watch_geo;
mod watch_geo_aisa;
mod watch_geo_ark;
mod watch_geo_bank;
mod watch_geo_view;
mod watch_gsc;
mod watch_birth;
mod watch_judge;
mod watch_judge_rules;
mod watch_keywords;
mod watch_locale_words;
mod watch_method;
mod watch_money;
mod watch_page_gate;
mod watch_persist;
mod watch_probe;
mod watch_reindex;
mod watch_reindex_resend;
pub(crate) mod watch_publish;
mod watch_rank;
mod watch_rank_history;
mod watch_rank_run;
mod watch_rank_serp;
mod watch_payments;
mod watch_rank_view;
mod watch_report;
mod watch_retro;
mod watch_segment_view;
mod watch_segments;
mod watch_signals;
mod watch_similarweb_words;
mod watch_origin;
mod watch_own_page;
mod watch_site_loop;
mod watch_synonyms;
mod watch_source;
mod watch_traffic;
pub(crate) mod watch_trends;
mod watch_trends_followup;
mod watch_trends_ledger;
mod watch_trends_market;
pub(crate) mod watch_trends_similarweb;
mod watch_trends_surge;
mod watch_trends_view;

pub const SCRIPTS: &[&str] = &["collect", "analyze", "mrrEstimate", "report", "watch", "push", "localeWords", "advisor", "hourDigest", "cpcPool"];

/// Runs `<script>` of the seo-competitor skill; `None` for an unknown script.
/// Every script except `advisor` has no help text and does real work (API calls, publishing, tabs),
/// so `-h`/`--help` stops before it runs.
pub fn main(script: &str, argv: &[String]) -> Option<i32> {
    if script != "advisor" && script != "hourDigest" && script != "cpcPool" && SCRIPTS.contains(&script) && argv.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("seo-competitor {script}: no --help; options are in .claude/skills/seo-competitor/SKILL.md and references/ (watch.md, push.md). Nothing ran.");
        return Some(2);
    }
    Some(match script {
        "collect" => collect::main(argv),
        "analyze" => analyze::main(argv),
        "mrrEstimate" => mrr_estimate::main(argv),
        "report" => report::main(argv),
        "watch" => watch::main(argv),
        "push" => push::main(argv),
        "localeWords" => watch_locale_words::main(argv),
        "advisor" => watch_advisor::main(argv),
        "hourDigest" => watch_hour_digest::main(argv),
        "cpcPool" => cpc_pool_cli::main(argv),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_stops_a_script_before_it_runs() {
        for script in ["watch", "push", "collect"] {
            assert_eq!(super::main(script, &["--help".into()]), Some(2));
            assert_eq!(super::main(script, &["-h".into()]), Some(2));
        }
        assert_eq!(super::main("nope", &["--help".into()]), None);
    }
}
