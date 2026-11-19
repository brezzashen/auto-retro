//! `mars-skill-tools conversion-analytics watch`: the hourly PostHog conversion and retention
//! watchdog of the conversion-analytics skill (launchd `local.autoretro.conversion-watch`).

mod alerts;
mod api;
mod collect;
mod config;
mod dashboard;
mod funnel;
mod health;
mod onboarding_queries;
mod hourly;
pub(crate) mod posthog;
mod probes;
mod publish;
mod queries;
mod exceptions;
mod recovery;
mod query;
mod report;
mod retro;
mod sources;
mod upstream;
mod visitors;
mod watch;

pub const SCRIPTS: &[&str] = &["watch", "query", "api"];

/// Runs `<script>` of the conversion-analytics skill; `None` for an unknown script.
pub fn main(script: &str, argv: &[String]) -> Option<i32> {
    Some(match script {
        "watch" => watch::main(argv),
        "query" => query::main(argv),
        "api" => api::main(argv),
        _ => return None,
    })
}
