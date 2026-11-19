//! Read-only source of the CRM turn retro: stored Mars CRM chat turns of the production database
//! (`agent.mastra_messages` assistant rows carrying `metadata.marsTurnTelemetry`), read through
//! `mars canary sql --role=agent` in Mars_CRM. Only timing, token, credit and tool metadata leave the
//! database: tool names, error codes and a hash of each call's arguments, never message text.
//! The canary track and example.com share this database, so one read covers both.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{os::unix::process::CommandExt, path::Path, process::{Command, Stdio}, time::{Duration, Instant}};

const CRM: &str = "/path/to/workspace/Mars_CRM";
const MARS: &str = "/home/user/.local/bin/mars";
const RUN_TIMEOUT: Duration = Duration::from_secs(240);
/// One statement reads at most this span: a longer projection hits the production statement timeout
/// (turn-scoring `turnRows.js` reads one day per query for the same reason).
const SLICE_MS: f64 = 86_400_000.0;
/// QA traffic that is not production use: the demo account's `resourceId` prefix.
const EXCLUDED_OWNER: &str = "89d9%";
/// How far back a window thread's previous turn is read. Its cache entry is long gone after a day, but without
/// the row a resumed thread's first step reads as `thread_start` and its expired history is not counted as the
/// `idle_expiry` it is (costly-turn 84507709929894033359: 9d185810 resumed after 24 h 20 min).
const LOOKBACK_MS: f64 = 7.0 * 86_400_000.0;
const PRIOR_CHUNK: usize = 200;

const SCOPE: &str = r#"role = 'assistant' and "resourceId" not like '{owner}' and "createdAtZ" > to_timestamp({from}) and "createdAtZ" <= to_timestamp({to})"#;

/// `content` is `text`: a row can be valid `json` yet invalid `jsonb` (a lone surrogate escape such as
/// `\udc8f`), and one such row failed every statement from 2026-11-12 to 2026-11-15 ("invalid input syntax
/// for type json"). Rows read only through this guard; `count_sql` reports how many it skipped.
const VALID: &str = "pg_input_is_valid(content, 'jsonb')";

/// The row's content as jsonb, or null when the cast would fail. A CASE evaluates the cast only on rows that
/// pass the guard; a WHERE alone does not fix the order in which Postgres evaluates quals and projections.
fn body() -> String { format!("case when {VALID} then content::jsonb end") }

fn scope(from_ms: f64, to_ms: f64) -> String {
    SCOPE.replace("{owner}", EXCLUDED_OWNER).replace("{from}", &format!("{:.3}", from_ms / 1000.0)).replace("{to}", &format!("{:.3}", to_ms / 1000.0))
}

/// User chat turns stored in `(from_ms, to_ms]`, the newest one's time and the telemetry rows skipped because
/// their content is not valid jsonb. Reminder turns are not chat turns.
pub(super) fn count_sql(from_ms: f64, to_ms: f64) -> String {
    format!(r#"select count(*) filter (where turn) n, floor(extract(epoch from max(at) filter (where turn)) * 1000) newest_ms,
  count(*) filter (where not ok) skipped from (
  select "createdAtZ" at, ok, i is not null and i->>'trigger' is distinct from 'reminder' turn from (
    select "createdAtZ", {VALID} ok, ({body})->'metadata'->'marsTurnTelemetry'->'identity' i
    from agent.mastra_messages where {scope} and content like '%marsTurnTelemetry%') r) m"#, body = body(), scope = scope(from_ms, to_ms))
}

/// The ended user chat turns of `(from_ms, to_ms]` that settled more than `credits` (enforced mode) or whose first text
/// (else their end) came `slow_ms` or more after they started, oldest first: `turn`, `at_ms`, `credits`. The costly-turn
/// watch reads this small list before it reads any window.
pub(super) fn costly_sql(from_ms: f64, to_ms: f64, credits: f64, slow_ms: f64) -> String {
    format!(r#"select t->'identity'->>'turnId' turn, floor(extract(epoch from at) * 1000) at_ms,
  (t->'settledCreditsByMode'->'enforced'->>'observed')::numeric credits
from (select "createdAtZ" at, ({body})->'metadata'->'marsTurnTelemetry' t
    from agent.mastra_messages where {scope} and content like '%marsTurnTelemetry%' and {VALID} offset 0) v
where t->'identity' is not null and t->'identity'->>'trigger' is distinct from 'reminder' and t->'terminal'->>'status' is not null
  and ((t->'settledCreditsByMode'->'enforced'->>'observed')::numeric > {credits}
    or coalesce((t->'milestones'->>'text')::bigint, (t->'terminal'->>'endedAt')::bigint) - (t->'identity'->>'startedAt')::bigint >= {slow_ms})
order by at"#, body = body(), scope = scope(from_ms, to_ms), slow_ms = slow_ms as i64)
}

/// One row per stored turn of `(from_ms, to_ms]`, oldest first: the compact projection `analyze.rs` reads.
/// No column of `m` may share a name with an element alias (`c`, `x`, `p`): Postgres resolves the bare
/// name to the outer column, and the aggregate then belongs to the outer query.
pub(super) fn turns_sql(from_ms: f64, to_ms: f64) -> String {
    format!("{} order by m.at", projection(&scope(from_ms, to_ms)))
}

/// The newest stored turn of each thread in `(from_ms - LOOKBACK_MS, from_ms]`: the request a window's first
/// turn of that thread could reuse from cache. Thread ids are UUID text from the window's own rows.
pub(super) fn prior_sql(threads: &[String], from_ms: f64) -> String {
    let ids: Vec<String> = threads.iter().filter(|t| t.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')).map(|t| format!("'{t}'")).collect();
    let scope = format!(r#"role = 'assistant' and thread_id in ({}) and "createdAtZ" > to_timestamp({:.3}) and "createdAtZ" <= to_timestamp({:.3})"#,
        ids.join(","), (from_ms - LOOKBACK_MS) / 1000.0, from_ms / 1000.0);
    format!("select distinct on (thread_id) * from ({}) r order by thread_id, at_ms desc", projection(&scope))
}

fn projection(scope: &str) -> String {
    let array = |path: &str| format!("case when jsonb_typeof({path}) = 'array' then {path} else '[]'::jsonb end");
    format!(r#"select m.thread_id, left(m.owner, 8) u, floor(extract(epoch from m.at) * 1000) at_ms,
  t->'identity'->>'turnId' turn, (t->'identity'->>'startedAt')::bigint s, t->'identity'->>'trigger' trig,
  t->'identity'->>'selectedReasoningLevel' lvl, (t->'milestones'->>'text')::bigint txt,
  t->'terminal'->>'status' status, (t->'terminal'->>'endedAt')::bigint ended,
  (t->'settledCreditsByMode'->'enforced'->>'observed')::numeric credits,
  (select jsonb_agg(jsonb_build_array((c->>'startedAt')::bigint, (c->'terminal'->>'endedAt')::bigint, (c->'usage'->>'inputTokens')::bigint,
      (c->'usage'->>'cachedInputTokens')::bigint, (c->'usage'->>'outputTokens')::bigint, c->'correlation'->>'role', (c->>'attempt')::int,
      c->'terminal'->>'status', (c->'settlement'->>'settledCredits')::numeric,
      coalesce(c->'model'->>'resolvedModelId', c->'model'->>'requestedModelId'), (c->'usage'->>'reasoningTokens')::bigint, c->'correlation'->>'agentName', (c->>'firstChunkAt')::bigint) order by (c->>'startedAt')::bigint)
    from jsonb_array_elements({calls}) c where c->>'boundary' = 'transport-attempt') calls,
  (select jsonb_agg(jsonb_build_array((x->>'startedAt')::bigint, (x->'terminal'->>'endedAt')::bigint, x->>'name', x->'correlation'->>'role', x->'terminal'->>'status'))
    from jsonb_array_elements({tools}) x) tools,
  (select jsonb_agg(jsonb_build_array(p->'toolInvocation'->>'toolName', p->'toolInvocation'->'result'->>'error', left(md5((p->'toolInvocation'->'args')::text), 12)))
    from jsonb_array_elements({parts}) p where p->>'type' = 'tool-invocation') inv,
  {activations} oma
from (select thread_id, "resourceId" owner, "createdAtZ" at, b body, b->'metadata'->'marsTurnTelemetry' t from (
    select thread_id, "resourceId", "createdAtZ", {body} b
    from agent.mastra_messages where {scope} and content like '%marsTurnTelemetry%' and {VALID} offset 0) v) m
where t is not null"#, body = body(), calls = array("t->'calls'"), tools = array("t->'tools'"), parts = array("m.body->'parts'"),
        activations = activations())
}

/// The epoch-ms times at which Observational Memory activated observations during the turn: the thread's stored
/// `data-om-activation` markers whose `activatedAt` falls between the turn's start and its telemetry row. The
/// marker is appended to the thread's last assistant message, often one of an earlier turn, so it is matched by
/// `activatedAt`, never by the message's time (2026-11-16, e2e5f013: the 04:38:49 activation sits in the 04:25
/// message). Only an activation changes the observation block in the request; a buffer seal alone leaves the next
/// request extending the previous one (mastra 6c04ecb874, `observationBufferPrefix.test.ts`). Always an array,
/// so a row read before this column existed (saved incidents) is told apart by the missing key.
fn activations() -> String {
    let at = "(q->'data'->>'activatedAt')::timestamptz";
    format!(r#"(select coalesce(jsonb_agg(floor(extract(epoch from {at}) * 1000)), '[]'::jsonb)
    from (select {body} ob from agent.mastra_messages where thread_id = m.thread_id and role = 'assistant'
        and "createdAtZ" > to_timestamp((m.t->'identity'->>'startedAt')::bigint / 1000.0) - interval '{days} days'
        and "createdAtZ" <= m.at + interval '1 minute' and content like '%data-om-activation%' and {VALID} offset 0) om,
    jsonb_array_elements({parts}) q where q->>'type' = 'data-om-activation' and q->'data'->>'activatedAt' is not null
      and {at} >= to_timestamp((m.t->'identity'->>'startedAt')::bigint / 1000.0) and {at} <= m.at + interval '1 minute')"#,
        body = body(), days = (LOOKBACK_MS / 86_400_000.0) as u32,
        parts = "case when jsonb_typeof(om.ob->'parts') = 'array' then om.ob->'parts' else '[]'::jsonb end")
}

/// The JSON array `mars canary sql --format=json` prints after its header lines.
pub(super) fn rows(stdout: &str) -> Result<Vec<Value>> {
    let end = stdout.rfind(']').context("mars canary sql printed no JSON array")?;
    // A header line may itself start with a bracket ("[mars] ..."): take the first line start that parses.
    stdout.match_indices('[').map(|(i, _)| i).filter(|&i| i <= end && (i == 0 || stdout.as_bytes()[i - 1] == b'\n'))
        .find_map(|i| serde_json::from_str(&stdout[i..=end]).ok()).context("mars canary sql printed no JSON array")
}

/// Runs one read-only statement as the agent role.
fn query(sql: &str) -> Result<Vec<Value>> { query_as("agent", sql) }

/// Runs one read-only statement as `role` (`agent` or `crm`); a stuck run is ended at RUN_TIMEOUT.
pub(in crate::telemetry) fn query_as(role: &str, sql: &str) -> Result<Vec<Value>> {
    let dir = std::env::temp_dir().join(format!("crm-turn-retro-{}-{}", std::process::id(), (crate::unix_now() * 1e6) as u64));
    std::fs::create_dir_all(&dir)?;
    let (out, err) = (dir.join("out.json"), dir.join("err.log"));
    let started = Instant::now();
    let result = (|| -> Result<Vec<Value>> {
        let mut child = Command::new(MARS).args(["canary", "sql"]).arg(format!("--role={role}")).args(["--format=json", "--limit=200000", sql]).current_dir(CRM)
            .env("PATH", super::super::MARS_SCRIPT_PATH).stdin(Stdio::null())
            .stdout(std::fs::File::create(&out)?).stderr(std::fs::File::create(&err)?).process_group(0).spawn().context("spawn mars canary sql")?;
        let status = loop {
            if let Some(status) = child.try_wait()? { break status; }
            if started.elapsed() >= RUN_TIMEOUT {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
                let _ = child.wait();
                bail!("mars canary sql timed out after {} s", RUN_TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(500));
        };
        let stdout = std::fs::read_to_string(&out)?;
        if !status.success() {
            // stderr echoes the statement; the database's refusal is the last line of either stream.
            let last = |text: &str| text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(300).collect::<String>();
            bail!("mars canary sql failed ({status}): {} | {}", last(&stdout), last(&std::fs::read_to_string(&err).unwrap_or_default()));
        }
        rows(&stdout)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// `(count, newest_ms, skipped)` of user chat turns stored in `(from_ms, to_ms]`; `skipped` counts telemetry
/// rows left out because their content is not valid jsonb.
pub(super) fn count(from_ms: f64, to_ms: f64) -> Result<(usize, f64, usize)> {
    let mut total = (0, 0.0f64, 0);
    for (from, to) in slices(from_ms, to_ms) {
        let rows = query(&count_sql(from, to))?;
        let row = rows.first().context("count query returned no row")?;
        total.0 += number(&row["n"]).unwrap_or(0.0) as usize;
        total.1 = total.1.max(number(&row["newest_ms"]).unwrap_or(0.0));
        total.2 += number(&row["skipped"]).unwrap_or(0.0) as usize;
    }
    Ok(total)
}

/// Every stored turn row of `(from_ms, to_ms]`, oldest first.
pub(super) fn turns(from_ms: f64, to_ms: f64) -> Result<Vec<Value>> {
    let mut all = Vec::new();
    for (from, to) in slices(from_ms, to_ms) { all.extend(query(&turns_sql(from, to))?); }
    Ok(all)
}

/// The window's turn rows preceded by each thread's previous turn (`prior: true`, at most LOOKBACK_MS
/// back), so a thread's first turn in the window has the request its cache could reuse.
pub(super) fn window(from_ms: f64, to_ms: f64) -> Result<Vec<Value>> {
    let rows = turns(from_ms, to_ms)?;
    let mut threads: Vec<String> = rows.iter().filter_map(|r| r["thread_id"].as_str().map(str::to_string)).collect();
    threads.sort();
    threads.dedup();
    let mut all = Vec::new();
    for chunk in threads.chunks(PRIOR_CHUNK) {
        for mut row in query(&prior_sql(chunk, from_ms))? { row["prior"] = Value::Bool(true); all.push(row); }
    }
    all.extend(rows);
    Ok(all)
}

pub(super) fn slices(from_ms: f64, to_ms: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut from = from_ms;
    while from < to_ms {
        let to = (from + SLICE_MS).min(to_ms);
        out.push((from, to));
        from = to;
    }
    out
}

/// A numeric column: Postgres bigint and numeric arrive as JSON numbers or strings.
pub(super) fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

/// Rows saved by `crm-turn-stats --out` or an incident (`{"rows": [...]}` or a bare array).
pub(super) fn saved(path: &Path) -> Result<Vec<Value>> {
    let value: Value = serde_json::from_slice(&std::fs::read(path).with_context(|| format!("read {}", path.display()))?)?;
    value.as_array().or_else(|| value["rows"].as_array()).cloned().with_context(|| format!("{} holds no turn rows", path.display()))
}
