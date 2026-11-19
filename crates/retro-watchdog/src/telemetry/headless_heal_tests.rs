use super::*;

fn renderer(pid: i32, cpu_secs: f64, args: &str) -> Proc {
    Proc { pid, ppid: 75784, uid: 501, start: 100, cpu_secs, rss_kib: 50_000, name: RENDERER.into(), args: args.into() }
}

const HEADLESS: &str = "/Users/u/Library/Caches/mars-headless-chrome/155/Google Chrome Helper (Renderer)";
const DESKTOP: &str = "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Helper (Renderer)";

#[test]
fn final_probe_preserves_recovered_navigated_or_missing_targets() {
    let due = vec![("recovered".into(), page("recovered", false).url),
        ("wedged".into(), page("wedged", false).url),
        ("navigated".into(), "old-url".into()), ("gone".into(), "gone-url".into())];
    let pages = vec![page("recovered", true), page("wedged", false), page("navigated", false)];
    assert_eq!(close_candidates(&due, &pages, 5.0), vec![("wedged".into(), page("wedged", false).url)]);
    assert!(close_candidates(&due, &pages, MAX_PROBE_GAP + 1.0).is_empty());
}

#[test]
fn cpu_history_gap_cannot_prove_a_continuous_hot_renderer() {
    let mut heal = HeadlessHeal::default();
    heal.sample(1000.0, &[renderer(1, 0.0, HEADLESS)]);
    assert!(heal.sample(1400.0, &[renderer(1, 400.0, HEADLESS)]).is_empty());
}

#[test]
fn flags_only_headless_renderers_pinned_for_the_whole_window() {
    let mut heal = HeadlessHeal::default();
    let mut hot = Vec::new();
    let mut now = 1000.0;
    while now <= 1000.0 + HOT_SECS {
        let busy = now - 1000.0;
        hot = heal.sample(now, &[renderer(1, busy * 0.95, HEADLESS), renderer(2, busy * 0.3, HEADLESS),
            renderer(3, busy * 1.0, DESKTOP)]);
        now += SAMPLE_SECS;
    }
    assert_eq!(hot.len(), 1);
    assert_eq!(hot[0]["pid"], 1);
    // A renderer that exited is forgotten.
    heal.sample(now, &[]);
    assert!(heal.renderers.is_empty());
}

#[test]
fn a_fresh_renderer_is_not_judged_before_the_window() {
    let mut heal = HeadlessHeal::default();
    assert!(heal.sample(1000.0, &[renderer(1, 0.0, HEADLESS)]).is_empty());
    assert!(heal.sample(1000.0 + HOT_SECS - 1.0, &[renderer(1, HOT_SECS, HEADLESS)]).is_empty());
}

fn page(id: &str, responsive: bool) -> Probed {
    Probed { id: id.into(), url: format!("http://localhost:6006/iframe.html?id={id}"), responsive }
}

#[test]
fn closes_a_page_only_after_it_failed_every_probe_for_the_hang_window() {
    let home = crate::telemetry::test_home("headless-heal");
    let mut heal = HeadlessHeal::default();
    heal.absorb(&home, Round { at: 1000.0, pages: Ok(vec![page("wedged", false), page("busy", false), page("ok", true)]), closed: vec![] });
    // The busy page answers again, so its clock resets; the wedged one keeps its first failure.
    heal.absorb(&home, Round { at: 1060.0, pages: Ok(vec![page("wedged", false), page("busy", true), page("ok", true)]), closed: vec![] });
    assert_eq!(heal.hung_since["wedged"].0, 1000.0);
    assert!(!heal.hung_since.contains_key("busy"));
    assert!(due(&heal.hung_since, 1000.0 + HANG_SECS - 1.0).is_empty());
    assert_eq!(due(&heal.hung_since, 1000.0 + HANG_SECS), vec![("wedged".to_string(), page("wedged", false).url)]);
    heal.absorb(&home, Round { at: 1200.0, pages: Ok(vec![]), closed: vec![json!({"id": "wedged", "outcome": "closed"})] });
    assert!(heal.hung_since.is_empty());
    let log = std::fs::read_dir(root(&home).join("headless-heals")).unwrap().next().unwrap().unwrap().path();
    assert_eq!(std::fs::read_to_string(log).unwrap().lines().count(), 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn an_unreachable_browser_clears_every_suspect() {
    let home = crate::telemetry::test_home("headless-heal-down");
    let mut heal = HeadlessHeal::default();
    heal.absorb(&home, Round { at: 1000.0, pages: Ok(vec![page("wedged", false)]), closed: vec![] });
    heal.absorb(&home, Round { at: 1030.0, pages: Err(anyhow!("connection refused")), closed: vec![] });
    assert!(heal.hung_since.is_empty());
    assert!(heal.last_probe["error"].is_string());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_lease_held_past_its_limit_with_live_waiters_opens_one_incident() {
    let home = crate::telemetry::test_home("lease-stall");
    let locks = home.join("locks");
    let me = std::process::id();
    std::fs::create_dir_all(locks.join("mars-crm-verify")).unwrap();
    std::fs::create_dir_all(locks.join("mars-crm-verify.queue")).unwrap();
    std::fs::write(locks.join("mars-crm-verify/owner.json"),
        json!({"holder": "ship-web-verify-1", "pid": me, "acquiredAt": 1_000_000.0}).to_string()).unwrap();
    std::fs::write(locks.join(format!("mars-crm-verify.queue/000000001100000-{me}-000001")), "w").unwrap();
    // A dead waiter's ticket does not count.
    std::fs::write(locks.join("mars-crm-verify.queue/000000001100000-999999-000001"), "w").unwrap();
    let limit = super::super::lease_stall::LEASES[0].1;
    let quiet = super::super::lease_stall::observe_in(&home, &locks, 1000.0 + limit - 1.0);
    assert_eq!(quiet["mars-crm-verify"]["waiters"], 1);
    assert!(std::fs::read_dir(root(&home).join("incidents")).map_or(true, |mut d| d.next().is_none()));
    super::super::lease_stall::observe_in(&home, &locks, 1000.0 + limit);
    super::super::lease_stall::observe_in(&home, &locks, 1000.0 + limit + 60.0);
    let incidents: Vec<_> = std::fs::read_dir(root(&home).join("incidents")).unwrap().collect();
    assert_eq!(incidents.len(), 1);
    let event = super::super::read(&incidents[0].as_ref().unwrap().path()).unwrap();
    assert_eq!(event["trigger"], "lease_stalled");
    assert_eq!(event["waiters"], 1);
    std::fs::remove_dir_all(home).unwrap();
}

/// Live check against the running shared headless Chrome: `cargo test -- --ignored live_probe`.
#[test]
#[ignore]
fn live_probe_lists_and_answers_pages() {
    let pages = probe(&PathBuf::from(std::env::var("HOME").unwrap()).join(ACTIVE_PORT)).unwrap();
    for page in &pages { eprintln!("{} {} {}", page.responsive, page.id, page.url); }
    assert!(pages.iter().any(|page| page.responsive));
}
