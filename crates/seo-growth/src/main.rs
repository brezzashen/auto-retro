//! `seo-growth`: CLI for autonomous SEO retro, keyword room, conversion analytics, and Similarweb queries.

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: seo-growth <module> <subcommand> [args...]\n\
         modules:\n  \
           seo         watch | advisor | hourDigest | cpcPool | localeWords | push | collect | analyze | mrrEstimate | report\n  \
           conversion  watch | query | api\n  \
           similarweb  status | sync-cookies | overview | keywords | anomalies"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        return usage();
    }
    let module = args[0].as_str();
    let script = args[1].as_str();
    let rest = &args[2..];
    let code = match module {
        "seo" | "seo-competitor" => seo_growth::skills::seo_competitor::main(script, rest),
        "conversion" | "conversion-analytics" => seo_growth::skills::conversion_analytics::main(script, rest),
        "similarweb" => seo_growth::skills::similarweb::main(script, rest),
        _ => None,
    };
    match code {
        Some(c) => ExitCode::from(c as u8),
        None => usage(),
    }
}
