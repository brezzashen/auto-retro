//! Is syspolicyd inside a Gatekeeper revocation re-check sweep? The `RevocationCheck` activity
//! (seen 2026-11-03 04:00, 11-08 00:00, 11-10 09:40 and 11-13 09:11 SGT) re-checks the unsigned
//! binaries in ExecPolicy `policy_scan_cache`: one online notarization lookup per row plus a
//! readdir of the binary's parent directory (cargo `debug/deps`, 751,977 entries on 11-13), which
//! holds 0.5-1 core for up to an hour at Maintenance QoS. Progress lives in the database, so a
//! restart does not end it: the successors resumed at 0.83 core (11-03) and 0.89 core (11-13,
//! 730 re-checks in its first 10 minutes). daemon_heal therefore holds syspolicyd while the sweep
//! advances (perf retro 87533033222017708964, cause-tree N7.2.2).
use std::path::Path;

pub(super) const HOLD: &str = "Gatekeeper revocation sweep active";
const DATABASE: &str = "file:/var/db/SystemPolicyConfiguration/ExecPolicy?mode=ro";
/// Rows checked in the last 10 minutes at least an hour after their first scan: re-checks, not
/// first scans of new executables.
const QUERY: &str = "select count(*) from policy_scan_cache where revocation_check_time > strftime('%s','now') - 600 \
    and revocation_check_time - timestamp > 3600";
/// A sweep re-checked 736-1,548 rows per 10 minutes on 11-13 and 208-326 on 11-10; outside a
/// sweep the 6 days before held one such row.
const MIN_RECHECKS: u64 = 50;
/// The probe runs only when a restart is otherwise due, at most once per CACHE_SECS.
const CACHE_SECS: f64 = 300.0;
const PROBE_TIMEOUT_SECS: u64 = 10;

#[derive(Default)]
pub(super) struct Sweep {
    probed_at: f64,
    rechecks: Option<u64>,
    /// Re-check count the next probe returns in tests, where no root query runs.
    #[cfg(test)]
    pub(super) fake: Option<u64>,
}

impl Sweep {
    /// True while the scan cache shows at least MIN_RECHECKS re-checks in the last 10 minutes.
    /// A failed probe counts as no sweep, which leaves the restart rule as it was.
    pub(super) fn active(&mut self, home: &Path, now: f64) -> bool {
        if self.probed_at == 0.0 || now < self.probed_at || now - self.probed_at >= CACHE_SECS {
            self.probed_at = now;
            self.rechecks = self.probe(home);
        }
        self.rechecks.is_some_and(|count| count >= MIN_RECHECKS)
    }

    #[cfg(test)]
    fn probe(&self, _home: &Path) -> Option<u64> {
        self.fake
    }

    #[cfg(not(test))]
    fn probe(&self, home: &Path) -> Option<u64> {
        rechecks(home).inspect_err(|error| crate::log(&format!("daemon heal: revocation sweep probe failed: {error:#}"))).ok()
    }
}

/// Read-only root query through the Keychain askpass; the database is root-only (0600).
fn rechecks(home: &Path) -> anyhow::Result<u64> {
    let askpass = format!("SUDO_ASKPASS={}/.local/bin/sudo-askpass", std::env::var("HOME")?);
    let args = [askpass.as_str(), "/usr/bin/sudo", "-A", "/usr/bin/sqlite3", "-readonly", DATABASE, QUERY];
    let text = super::command::capture(home, "/usr/bin/env", &args, PROBE_TIMEOUT_SECS)?;
    parse(&text).ok_or_else(|| anyhow::anyhow!("unexpected sqlite3 output {text:?}"))
}

/// sqlite3 prints the count alone on one line.
fn parse(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_count_sqlite_prints() {
        assert_eq!(parse("730\n"), Some(730));
        assert_eq!(parse("0\n"), Some(0));
        assert_eq!(parse("Error: unable to open database file\n"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn a_sweep_needs_enough_rechecks_and_is_probed_once_per_cache_window() {
        let home = std::env::temp_dir();
        let mut sweep = Sweep { fake: Some(730), ..Sweep::default() };
        assert!(sweep.active(&home, 1000.0));
        // The cached answer stands until CACHE_SECS passed.
        sweep.fake = Some(0);
        assert!(sweep.active(&home, 1000.0 + CACHE_SECS - 1.0));
        assert!(!sweep.active(&home, 1000.0 + CACHE_SECS));
        sweep.fake = Some(MIN_RECHECKS - 1);
        assert!(!sweep.active(&home, 2000.0));
        sweep.fake = Some(MIN_RECHECKS);
        assert!(sweep.active(&home, 3000.0));
        // A failed probe is no sweep.
        sweep.fake = None;
        assert!(!sweep.active(&home, 4000.0));
    }

    /// `cargo test -p cpu-watchdog live_revocation -- --ignored --nocapture` runs the real root query.
    #[test]
    #[ignore]
    fn live_revocation_recheck_count() {
        let home = crate::telemetry::test_home("gatekeeper-sweep-live");
        println!("re-checks in the last 10 minutes: {}", rechecks(&home).unwrap());
        std::fs::remove_dir_all(home).unwrap();
    }
}
