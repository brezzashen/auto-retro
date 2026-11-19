//! HogQL of the conversion watchdog. Dates are Asia/Singapore; `since` is the first production
//! event (`trackingStart`), `from` a SQL date expression bounding the cohorts shown. Server events are
//! counted by `uniq(uuid)`: overlapping `analytics-sync` windows store the same uuid more than once.

pub const PROD: &str = "properties.mars_env = 'production'";
const TZ: &str = "'Asia/Singapore'";

pub struct Step {
    pub key: &'static str,
    pub label: &'static str,
    pub condition: &'static str,
}

/// The ordered 24-hour visitor funnel; a person counts at a step only after every earlier one.
pub const STEPS: [Step; 7] = [
    Step { key: "visit", label: "访问", condition: "event = '$pageview'" },
    Step { key: "cta", label: "点击开始", condition: "event = 'cta_clicked' and coalesce(toString(properties.position), '') not in ('hero_app_store', 'wechat_app_banner')" },
    Step { key: "signup", label: "注册/验证码通过", condition: "event in ('user_signed_up', 'auth_otp_verified')" },
    Step { key: "message", label: "首条消息", condition: "event = 'message_sent'" },
    Step { key: "person", label: "首位联系人", condition: "event = 'person_created' and properties.source in ('chat', 'manual')" },
    Step { key: "checkout", label: "开始结账", condition: "event = 'checkout_started'" },
    Step { key: "paid", label: "付款", condition: "event = 'order_paid' or (event = 'checkout_returned' and properties.result = 'paid')" },
];

const FUNNEL_EVENTS: &str = "'$pageview', 'cta_clicked', 'user_signed_up', 'auth_otp_verified', 'message_sent', 'person_created', 'checkout_started', 'checkout_returned', 'order_paid'";
const AUTH_EVENTS: &str = "'auth_otp_sent', 'auth_otp_verified', 'auth_password_signed_in', 'auth_oauth_started', 'auth_failed'";

pub struct Dimension {
    pub key: &'static str,
    pub label: &'static str,
    /// Events the per-person value is read from, and the value (first one wins).
    pub filter: &'static str,
    pub value: &'static str,
}

pub const DIMENSIONS: [Dimension; 3] = [
    Dimension { key: "source", label: "首次来源", filter: "event = '$pageview'",
        value: "coalesce(nullIf(properties.utm_source, ''), nullIf(properties.$referring_domain, ''), '(none)')" },
    Dimension { key: "method", label: "登录方式", filter: "event in ('auth_otp_sent', 'auth_otp_verified', 'auth_password_signed_in', 'auth_oauth_started')",
        value: "properties.method" },
    Dimension { key: "platform", label: "设备", filter: "event = '$pageview'", value: "properties.$device_type" },
];

pub fn since_sql(since: &str) -> String {
    format!("toDateTime('{since}', {TZ})")
}

pub fn date_sql(day: &str) -> String {
    format!("toDate('{day}')")
}

pub(super) fn sgt_date(expr: &str) -> String {
    format!("toDate(toTimeZone({expr}, {TZ}))")
}

/// Events per environment, event and 24-hour window back from now (`ago` 0 is the last 24 h).
pub fn volume() -> String {
    "select coalesce(properties.mars_env, '(none)') env, event, intDiv(dateDiff('second', timestamp, now()), 86400) ago, count() events, uniq(uuid) unique_events \
     from events where timestamp > now() - interval 8 day group by env, event, ago limit 10000".into()
}

/// Per server event, hours since the newest copy of a last-24-hour row stored more than once (its
/// `created_at` is when PostHog ingested the resend); an event with no repeated uuid has no row.
pub fn resends(server_events: &[String]) -> String {
    let names = server_events.iter().map(|e| format!("'{}'", e.replace('\'', ""))).collect::<Vec<_>>().join(", ");
    format!("select event, dateDiff('minute', max(latest), now()) / 60.0 hours from (select event, uuid, max(created_at) latest, count() copies \
     from events where {PROD} and event in ({names}) and timestamp > now() - interval 1 day group by event, uuid having copies > 1) group by event")
}

/// Last-24-hour production events matching each witness condition, one column per condition in order.
pub fn witness_counts(conditions: &[String]) -> String {
    let columns = conditions.iter().enumerate().map(|(i, c)| format!("countIf({c}) w{i}")).collect::<Vec<_>>().join(", ");
    format!("select {columns} from events where {PROD} and timestamp > now() - interval 1 day")
}

/// Last-24-hour events that carry no `mars_env`, by host and path.
pub fn unlabelled() -> String {
    "select coalesce(properties.$host, '(none)') host, coalesce(properties.$pathname, '') path, event, count() c \
     from events where properties.mars_env is null and timestamp > now() - interval 1 day group by host, path, event order by c desc limit 20".into()
}

/// Production sign-ups since tracking started, and how many of them the web SDK identified.
pub fn stitching(since: &str) -> String {
    let since = since_sql(since);
    format!("with s as (select person_id p from events where event = 'user_signed_up' and {PROD} and timestamp >= {since} group by p), \
     w as (select person_id p, 1 web, max(event in ({AUTH_EVENTS})) auth from events where {PROD} and timestamp >= {since} - interval 1 day \
     and event not in ('user_signed_up', 'person_created', 'order_paid') group by p) \
     select count() signups, countIf(w.web = 1) with_web, countIf(w.auth = 1) with_auth from s left join w on w.p = s.p")
}

fn reached(k: usize) -> String {
    (1..=k).map(|i| format!("(x.t{i} > v.t - interval 1 hour and x.t{i} <= v.t + interval 24 hour)")).collect::<Vec<_>>().join(" and ")
}

/// CTEs `v` (first production visit per person), `x` (first time at each later step) and `b`
/// (one row per visitor with `s0`..`s6`: reached that step within 24 h, in order).
fn funnel_base(since: &str) -> String {
    let since = since_sql(since);
    let firsts: Vec<String> = STEPS.iter().enumerate().skip(1).map(|(i, s)| format!("minIf(timestamp, {}) t{i}", s.condition)).collect();
    let flags: Vec<String> = (0..STEPS.len()).map(|k| if k == 0 { "1 s0".into() } else { format!("{} s{k}", reached(k)) }).collect();
    format!("v as (select person_id p, min(timestamp) t, {} d from events where {PROD} and event = '$pageview' and timestamp >= {since} group by p), \
     x as (select person_id p, {} from events where {PROD} and timestamp >= {since} and event in ({FUNNEL_EVENTS}) group by p), \
     b as (select v.p p, v.d d, {} from v left join x on x.p = v.p)", sgt_date("min(timestamp)"), firsts.join(", "), flags.join(", "))
}

/// Visitors by the Singapore date of their first visit, and how many of them reached each step in 24 h.
pub fn funnel(since: &str, from: &str) -> String {
    let sums: Vec<String> = STEPS.iter().enumerate().map(|(k, s)| format!("sum(s{k}) {}", s.key)).collect();
    format!("with {} select d, {} from b where d >= {from} group by d order by d limit 400", funnel_base(since), sums.join(", "))
}

/// Of the visitors in `[from, ..]` who reached step `k`, how many reached `k + 1`, split by `dim`.
pub fn breakdown(since: &str, from: &str, k: usize, dim: &Dimension) -> String {
    format!("with {}, dim as (select person_id p, argMin({}, timestamp) v from events where {PROD} and ({}) and timestamp >= {} group by p) \
     select coalesce(nullIf(toString(dim.v), ''), '(none)') value, sum(b.s{k}) reached, sum(b.s{}) next \
     from b left join dim on dim.p = b.p where b.d >= {from} and b.s{k} = 1 group by value order by reached desc limit 12",
        funnel_base(since), dim.value, dim.filter, since_sql(since), k + 1)
}

/// Sign-up cohorts (Singapore date) with first message, first chat/manual person and both within 24 h.
pub fn activation(since: &str) -> String {
    let within = |t: &str| format!("{t}.hit = 1 and {t}.t <= s.t + interval 24 hour");
    format!("with s as (select person_id p, min(timestamp) t from events where event = 'user_signed_up' and {PROD} and timestamp >= {} group by p), \
     pc as (select person_id p, min(timestamp) t, 1 hit from events where event = 'person_created' and properties.source in ('chat', 'manual') and {PROD} group by p), \
     m as (select person_id p, min(timestamp) t, 1 hit from events where event = 'message_sent' and {PROD} group by p) \
     select {} d, count() signups, countIf({}) messaged, countIf({}) person, countIf({} and {}) activated, max(s.t) <= now() - interval 24 hour matured \
     from s left join pc on pc.p = s.p left join m on m.p = s.p group by d order by d",
        since_sql(since), sgt_date("s.t"), within("m"), within("pc"), within("m"), within("pc"))
}

/// Sign-up cohorts and who came back (`$pageview` or `message_sent`) exactly 1, 7 and 30 days later.
pub fn retention(since: &str) -> String {
    let since = since_sql(since);
    format!("with c as (select person_id p, {} d0 from events where event = 'user_signed_up' and {PROD} group by p having min(timestamp) >= {since}), \
     a as (select person_id p, {} d from events where event in ('$pageview', 'message_sent') and {PROD} and timestamp >= {since} group by p, d) \
     select c.d0, uniq(c.p) size, uniqIf(c.p, a.d = addDays(c.d0, 1)) d1, uniqIf(c.p, a.d = addDays(c.d0, 7)) d7, uniqIf(c.p, a.d = addDays(c.d0, 30)) d30 \
     from c left join a on a.p = c.p group by c.d0 order by c.d0", sgt_date("min(timestamp)"), sgt_date("timestamp"))
}

/// Sign-in attempts by method since `from`.
pub fn signin_methods(from: &str) -> String {
    format!("select coalesce(properties.method, '(none)') method, countIf(event = 'auth_otp_sent') sent, countIf(event = 'auth_otp_verified') verified, \
     countIf(event = 'auth_password_signed_in') password, countIf(event = 'auth_oauth_started') oauth, countIf(event = 'auth_failed') failed \
     from events where {PROD} and event in ({AUTH_EVENTS}) and {} >= {from} group by method order by sent + password + oauth desc", sgt_date("timestamp"))
}

pub fn signin_failures(from: &str) -> String {
    format!("select coalesce(properties.step, '(none)') failed_step, coalesce(properties.error_code, '(none)') code, count() c \
     from events where event = 'auth_failed' and {PROD} and {} >= {from} group by failed_step, code order by c desc limit 15", sgt_date("timestamp"))
}

/// Production sign-ups since tracking started, by first-touch source and referring domain.
pub fn signup_sources(since: &str) -> String {
    format!("select coalesce(nullIf(person.properties.$initial_utm_source, ''), '(none)') utm, coalesce(nullIf(person.properties.$initial_referring_domain, ''), '(none)') ref, uniq(person_id) people \
     from events where event = 'user_signed_up' and {PROD} and timestamp >= {} group by utm, ref order by people desc limit 15", since_sql(since))
}

/// Production events per Singapore day and event over 14 days (the dashboard's tracking-health table).
pub fn daily_events() -> String {
    format!("select {} d, event, uniq(uuid) events from events where {PROD} and timestamp > now() - interval 14 day group by d, event order by d desc, events desc limit 1000", sgt_date("timestamp"))
}

/// Per Singapore day since tracking started: sign-in codes sent and verified, checkouts started, orders paid.
pub fn daily_rates(since: &str) -> String {
    format!("select {} d, countIf(event = 'auth_otp_sent') sent, countIf(event = 'auth_otp_verified') verified, \
     countIf(event = 'checkout_started') checkouts, uniqIf(uuid, event = 'order_paid') paid \
     from events where {PROD} and timestamp >= {} and event in ('auth_otp_sent', 'auth_otp_verified', 'checkout_started', 'order_paid') \
     group by d order by d", sgt_date("timestamp"), since_sql(since))
}

/// Production events and visitors per Singapore hour over the last 48 hours.
pub fn hourly() -> String {
    format!("select toStartOfHour(toTimeZone(timestamp, {TZ})) h, uniq(uuid) events, uniqIf(person_id, event = '$pageview') visitors \
     from events where {PROD} and timestamp > now() - interval 48 hour group by h order by h limit 100")
}
