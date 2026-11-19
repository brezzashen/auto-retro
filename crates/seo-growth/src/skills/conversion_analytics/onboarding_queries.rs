//! Dashboard tables for the phone onboarding line's length and exits (founder 2026-11-07): the events of
//! web `onboardingExit.ts` and iOS `OnboardingExit.swift`, production only, dates in Asia/Singapore.

use super::queries::{PROD, sgt_date, since_sql};

const EXIT_EVENTS: &str = "('onboarding_step_viewed', 'onboarding_completed', 'onboarding_skipped', 'onboarding_abandoned')";

/// Per day: people who saw a step, finished, skipped through the survey or left early, and the median finish time.
pub fn daily(since: &str) -> String {
    format!("select {day} sgt_day, uniqIf(person_id, event = 'onboarding_step_viewed') walkers, \
     uniqIf(person_id, event = 'onboarding_completed' and properties.skipped = false) finished, \
     uniqIf(person_id, event = 'onboarding_skipped') skipped, uniqIf(person_id, event = 'onboarding_abandoned') left_early, \
     round(quantileIf(0.5)(toFloat(properties.elapsed_ms), event = 'onboarding_completed' and properties.skipped = false) / 1000) finish_median_s \
     from events where event in {EXIT_EVENTS} and {PROD} and timestamp >= {} group by sgt_day order by sgt_day desc limit 30",
        since_sql(since), day = sgt_date("timestamp"))
}

/// Skip-survey answers over `from`, with the step it was opened on and the median time spent before it.
pub fn skip_reasons(from: &str) -> String {
    format!("select properties.reason reason, properties.step_id step_id, uniq(person_id) people, \
     round(quantile(0.5)(toFloat(properties.elapsed_ms)) / 1000) median_s \
     from events where event = 'onboarding_skipped' and {PROD} and {} >= {from} group by reason, step_id order by people desc limit 30",
        sgt_date("timestamp"))
}

/// Where people left over `from` and never finished in that window: the step, how (`via`) and the median time in.
pub fn drop_steps(from: &str) -> String {
    let day = sgt_date("timestamp");
    format!("select properties.step_id step_id, properties.via via, uniq(person_id) people, \
     round(quantile(0.5)(toFloat(properties.elapsed_ms)) / 1000) median_s \
     from events where event = 'onboarding_abandoned' and {PROD} and {day} >= {from} \
     and person_id not in (select person_id from events where event = 'onboarding_completed' and {PROD} and {day} >= {from}) \
     group by step_id, via order by people desc limit 30")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_reads_production_onboarding_events() {
        for sql in [daily("2026-11-07"), skip_reasons("today() - 30"), drop_steps("today() - 30")] {
            assert!(sql.contains(PROD), "{sql}");
            assert!(sql.contains("onboarding_"), "{sql}");
        }
        assert!(drop_steps("today() - 30").contains("not in (select person_id"));
    }
}
