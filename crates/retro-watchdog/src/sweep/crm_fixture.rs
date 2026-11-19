//! Orphaned fixture `mars-crm-server` processes: the Rust CRM that `Mars_CRM/agent/tests/crmServer.ts` starts for
//! one `bun test` run, from a local cargo build, in the agent package directory. Re-parented to launchd it has no
//! runner left to serve. A server started with `MARS_SERVE_PARENT_PID` exits with its runner by itself; this closes
//! servers built before that guard and any future regression of it.

use crate::procs::Proc;

/// A just-reparented process gets this long to exit on its own before the sweep closes it.
pub const MIN_AGE_SECS: i64 = 60;

/// Whether `proc` is an orphaned fixture CRM: this user's, re-parented to launchd, a `mars-crm-server` from a
/// local `rust/target` build, working in an `agent` package directory, and older than [`MIN_AGE_SECS`].
pub fn is_orphaned(proc: &Proc, uid: u32, cwd: Option<&str>, now: i64) -> bool {
    let executable = proc.args.split_whitespace().next().unwrap_or_default();
    let local_build = ["/rust/target/release/mars-crm-server", "/rust/target/debug/mars-crm-server"]
        .iter()
        .any(|suffix| executable.ends_with(suffix));
    proc.uid == uid
        && proc.ppid == 1
        && local_build
        && cwd.is_some_and(|cwd| cwd.trim_end_matches('/').ends_with("/agent"))
        && now - proc.start >= MIN_AGE_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    const UID: u32 = 501;
    const NOW: i64 = 10_000;
    const AGENT: &str = "/path/to/workspace/Mars_CRM/agent";

    fn fixture(args: &str) -> Proc {
        Proc { pid: 4242, ppid: 1, uid: UID, start: NOW - 600, cpu_secs: 1.0, rss_kib: 190_000, name: "mars-crm-server".into(), args: args.into() }
    }

    const MAIN_TREE: &str = "/path/to/workspace/Mars_CRM/rust/target/release/mars-crm-server";

    #[test]
    fn an_orphan_from_the_main_checkout_or_a_canary_tree_is_closed() {
        assert!(is_orphaned(&fixture(MAIN_TREE), UID, Some(AGENT), NOW));
        let canary = "/var/folders/x/T/app-canary-77-1761-1/tree/rust/target/release/mars-crm-server";
        assert!(is_orphaned(&fixture(canary), UID, Some("/var/folders/x/T/app-canary-77-1761-1/tree/agent"), NOW));
    }

    #[test]
    fn a_server_with_a_live_runner_or_another_owner_is_kept() {
        assert!(!is_orphaned(&Proc { ppid: 900, ..fixture(MAIN_TREE) }, UID, Some(AGENT), NOW));
        assert!(!is_orphaned(&Proc { uid: 502, ..fixture(MAIN_TREE) }, UID, Some(AGENT), NOW));
    }

    #[test]
    fn a_young_orphan_gets_time_to_exit_with_its_runner() {
        assert!(!is_orphaned(&Proc { start: NOW - 10, ..fixture(MAIN_TREE) }, UID, Some(AGENT), NOW));
    }

    #[test]
    fn a_server_outside_an_agent_directory_or_not_a_local_build_is_kept() {
        assert!(!is_orphaned(&fixture(MAIN_TREE), UID, Some("/path/to/workspace/Mars_CRM"), NOW));
        assert!(!is_orphaned(&fixture(MAIN_TREE), UID, None, NOW));
        assert!(!is_orphaned(&fixture("/usr/local/bin/mars-crm-server"), UID, Some(AGENT), NOW));
        assert!(!is_orphaned(&fixture("/path/to/workspace/Mars_CRM/rust/target/release/mars-crm-isolated"), UID, Some(AGENT), NOW));
    }
}
