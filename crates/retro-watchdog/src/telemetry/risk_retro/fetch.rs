//! Read-only source of the risk retro: every warning, review and suspension Mars CRM recorded for a
//! non-exempt account, read through `mars canary sql --role=crm` (the canary track and example.com share
//! the production database). Only ids, kinds, times, counts, feature codes and the stored masked excerpt
//! leave the database; emails are masked in SQL and message text is never read.
//!
//! The policy is read from the Mars CRM source at each sample instead of being copied here, so it never
//! drifts: the QA exemption list (`RISK_EXEMPT_EMAILS`) and the provider-block warning band
//! (`WARN_FROM`..`SUSPEND_AT` blocks inside `WINDOW_MS`). The content-filter warning is computed per turn and
//! never stored, so it is rebuilt from `crm.risk_signal` with the same rule. A source that no longer parses
//! fails the sample (health shows it) rather than guessing.
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;

const CRM: &str = "/path/to/workspace/Mars_CRM";
const EXEMPT_SOURCE: &str = "rust/crates/mars-contracts/src/account_suspension.rs";
const FILTER_SOURCE: &str = "rust/crates/mars-crm-domain/src/risk/content_filter.rs";

/// The policy the events are judged by, read from the Mars CRM source.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Policy {
    pub exempt: Vec<String>,
    pub warn_from: u32,
    pub suspend_at: u32,
    pub window_ms: f64,
}

pub(super) fn policy() -> Result<Policy> {
    let read = |rel: &str| std::fs::read_to_string(std::path::Path::new(CRM).join(rel)).with_context(|| format!("read {rel}"));
    parse_policy(&read(EXEMPT_SOURCE)?, &read(FILTER_SOURCE)?)
}

pub(super) fn parse_policy(exempt_src: &str, filter_src: &str) -> Result<Policy> {
    let start = exempt_src.find("pub const RISK_EXEMPT_EMAILS").context("RISK_EXEMPT_EMAILS not found")?;
    let body = &exempt_src[start..];
    let list = &body[body.find("= &[").context("RISK_EXEMPT_EMAILS list")? + 4..body.find("];").context("RISK_EXEMPT_EMAILS end")?];
    let exempt: Vec<String> = list.split(',').map(|s| s.trim().trim_matches('"').to_lowercase()).filter(|s| !s.is_empty()).collect();
    ensure!(!exempt.is_empty(), "RISK_EXEMPT_EMAILS parsed empty");
    for email in &exempt {
        ensure!(email.contains('@') && email.bytes().all(|b| b.is_ascii_alphanumeric() || b"@.+_-".contains(&b)), "unexpected exempt entry {email}");
    }
    let constant = |name: &str| -> Result<f64> {
        let line = filter_src.lines().find(|l| l.contains(&format!("pub const {name}:"))).with_context(|| format!("{name} not found"))?;
        let expr = line.split('=').nth(1).context("constant value")?.trim().trim_end_matches(';');
        expr.split('*').map(|f| f.trim().replace('_', "").parse::<f64>().with_context(|| format!("{name}: {expr}"))).product()
    };
    let (warn_from, suspend_at, window_ms) = (constant("WARN_FROM")? as u32, constant("SUSPEND_AT")? as u32, constant("WINDOW_MS")?);
    ensure!(warn_from > 0 && warn_from < suspend_at && window_ms > 0.0, "implausible content filter policy {warn_from}/{suspend_at}/{window_ms}");
    Ok(Policy { exempt, warn_from, suspend_at, window_ms })
}

fn ts(ms: f64) -> String { format!("to_timestamp({:.3})", ms / 1000.0) }

/// One row per event recorded in `(from_ms, to_ms]`, oldest first: `kind`, `key`, `at_ms`, `user_id`,
/// `email_masked`, `detail`.
pub(super) fn events_sql(p: &Policy, from_ms: f64, to_ms: f64) -> String {
    let exempt: Vec<String> = p.exempt.iter().map(|e| format!("'{e}'")).collect();
    let (from, to) = (ts(from_ms), ts(to_ms));
    format!(r#"with ok as (select m.user_id, left(lower(m.email), 2) || '***@' || split_part(lower(m.email), '@', 2) email
    from crm.me m left join crm.entitlement e on e.user_id = m.user_id
   where not coalesce(e.is_admin, false) and not coalesce(lower(m.email) = any(array[{exempt}]), false)),
blocks as (select user_id, fingerprint, min(observed_at) at from crm.risk_signal
   where behaviour = 'content_filtered' and observed_at > {from} - interval '{window} milliseconds' and observed_at <= {to} group by 1, 2),
cf as (select b.*, count(*) over (partition by b.user_id order by b.at range between interval '{window} milliseconds' preceding and current row) n from blocks b)
select kind, key, floor(extract(epoch from at) * 1000) at_ms, user_id::text user_id, ok.email email_masked, detail from (
  select 'content_filter_warning' kind, 'cf:' || cf.user_id || ':' || cf.fingerprint key, cf.at, cf.user_id,
    jsonb_build_object('blocks', cf.n, 'remaining', {suspend} - cf.n) detail from cf where cf.at > {from} and cf.n >= {warn} and cf.n < {suspend}
  union all select 'risk_review', 'rv:' || r.id, r.opened_at, r.user_id,
    jsonb_build_object('behaviour', r.behaviour, 'confidence', r.confidence, 'status', r.status, 'features', r.features,
      'excerpt', r.excerpt, 'rules_version', r.rules_version, 'offense', r.offense, 'decided_by', r.decided_by)
    from crm.risk_review r where r.opened_at > {from} and r.opened_at <= {to}
  union all select 'cancellation_warning', 'cw:' || w.user_id || ':' || floor(extract(epoch from w.warned_at) * 1000), w.warned_at, w.user_id,
    jsonb_build_object('cancelled_turns', w.cancelled_turns, 'forgone_credits', w.forgone_credits, 'charged_credits', w.charged_credits,
      'window_days', w.window_days, 'notice_status', w.notice_status)
    from crm.cancellation_warning w where w.warned_at > {from} and w.warned_at <= {to}
  union all select 'abuse_flag', 'af:' || f.user_id || ':' || floor(extract(epoch from f.flagged_at) * 1000), f.flagged_at, f.user_id,
    jsonb_build_object('reason', f.reason, 'status', f.status, 'offenses', f.offenses, 'severe_offenses', f.severe_offenses,
      'suspended_until', f.suspended_until, 'decided_by', f.decided_by)
    from crm.billing_abuse_flag f where f.flagged_at > {from} and f.flagged_at <= {to}
) ev join ok using (user_id) order by at"#,
        exempt = exempt.join(","), window = p.window_ms as i64, warn = p.warn_from, suspend = p.suspend_at)
}

/// Every event recorded in `(from_ms, to_ms]`.
pub(super) fn events(from_ms: f64, to_ms: f64) -> Result<Vec<Value>> {
    if cfg!(test) { bail!("no production reads in tests"); }
    let policy = policy()?;
    super::super::crm_turn_retro::fetch::query_as("crm", &events_sql(&policy, from_ms, to_ms))
}
