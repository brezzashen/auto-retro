//! Whether the Codex process that last served a thread still runs.
//!
//! Codex logs each session-loop line to `<codex home>/logs_N.sqlite` with `thread_id` and
//! `process_uuid` = `pid:<pid>:<uuid>`. A code-mode `exec` call carries its clock inside script
//! text, so `rows::codex_turn_gone` cannot time it; a TUI that exited in the middle of such a call
//! (incident 86025906949814180909: pid 31872 logged last at 14:11:44 SGT, its `write_stdin`
//! stayed open) is told apart by that pid instead. Unknown (no home, no database, no row) is
//! `None`, and the caller keeps its conservative "alive" default; a reused pid reads as alive.
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Codex home of a rollout path: the directory that holds `sessions/` or `archived_sessions/`.
pub(super) fn codex_home(rollout: &str) -> Option<PathBuf> {
    let path = Path::new(rollout);
    path.ancestors().find(|a| a.file_name().is_some_and(|n| n == "sessions" || n == "archived_sessions"))?.parent().map(Path::to_path_buf)
}

/// The thread id a rollout file names: the trailing UUID of `rollout-<time>-<uuid>.jsonl`.
pub(super) fn thread_id(rollout: &str) -> Option<String> {
    let stem = Path::new(rollout).file_stem()?.to_str()?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    (id.len() == 36 && id.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')).then(|| id.to_string())
}

/// The pid inside a `process_uuid` of the form `pid:<pid>:<uuid>`.
pub(super) fn pid_of(process_uuid: &str) -> Option<i64> {
    process_uuid.trim().strip_prefix("pid:")?.split(':').next()?.parse().ok().filter(|p| *p > 0)
}

/// The newest-schema log database in a Codex home (`logs_2.sqlite` beats `logs_1.sqlite`).
fn logs_db(home: &Path) -> Option<PathBuf> {
    std::fs::read_dir(home).ok()?.flatten().filter_map(|e| {
        let name = e.file_name().to_str()?.to_string();
        let n: u32 = name.strip_prefix("logs_")?.strip_suffix(".sqlite")?.parse().ok()?;
        Some((n, e.path()))
    }).max_by_key(|(n, _)| *n).map(|(_, p)| p)
}

/// The `process_uuid` of the newest log line of `thread` in `db` (indexed by `idx_logs_thread_id_ts`).
pub(super) fn last_process(db: &Path, thread: &str) -> Option<String> {
    let sql = format!("select process_uuid from logs where thread_id='{thread}' order by ts desc limit 1;");
    let out = Command::new("/usr/bin/sqlite3").arg("-readonly").arg(db).arg(sql).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    out.status.success().then(|| text.trim().to_string()).filter(|s| !s.is_empty())
}

/// `Some(false)` when the process that last logged for this rollout's thread has exited.
pub(super) fn process_alive(rollout: &str, alive: impl Fn(i64) -> bool) -> Option<bool> {
    let (home, thread) = (codex_home(rollout)?, thread_id(rollout)?);
    Some(alive(pid_of(&last_process(&logs_db(&home)?, &thread)?)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLLOUT: &str = "/h/.codex/sessions/2026/11/14/rollout-2026-11-14T14-00-01-01a1d8a4-2f2f-7440-8f1b-6ea1aad24c1b.jsonl";

    #[test]
    fn a_rollout_path_names_its_home_and_thread() {
        assert_eq!(codex_home(ROLLOUT), Some(PathBuf::from("/h/.codex")));
        assert_eq!(codex_home("/h/.codex/archived_sessions/rollout-x.jsonl"), Some(PathBuf::from("/h/.codex")));
        assert_eq!(codex_home("/h/.claude/projects/p/a.jsonl"), None);
        assert_eq!(thread_id(ROLLOUT).as_deref(), Some("01a1d8a4-2f2f-7440-8f1b-6ea1aad24c1b"));
        assert_eq!(thread_id("/h/.codex/sessions/short.jsonl"), None);
    }

    #[test]
    fn process_uuid_yields_its_pid() {
        assert_eq!(pid_of("pid:31872:7a63147c-786f-4b84-bdd5-7186a1c84087\n"), Some(31872));
        assert_eq!(pid_of("pid:0:x"), None);
        assert_eq!(pid_of("thread-less"), None);
    }

    #[test]
    fn an_exited_codex_tui_reads_gone_and_an_unknown_thread_reads_unknown() {
        // Incident 86025906949814180909: the thread's newest line came from pid 31872, which had exited.
        let dir = std::env::temp_dir().join(format!("codex-live-{}", std::process::id()));
        let rollout = dir.join("sessions/2026/11/14/rollout-2026-11-14T14-00-01-01a1d8a4-2f2f-7440-8f1b-6ea1aad24c1b.jsonl");
        std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let db = dir.join("logs_2.sqlite");
        let _ = std::fs::remove_file(&db);
        let sql = "create table logs(ts integer, thread_id text, process_uuid text);\
            insert into logs values(1, '01a1d8a4-2f2f-7440-8f1b-6ea1aad24c1b', 'pid:11:a');\
            insert into logs values(2, '01a1d8a4-2f2f-7440-8f1b-6ea1aad24c1b', 'pid:31872:b');\
            insert into logs values(3, null, 'pid:11:a');";
        assert!(Command::new("/usr/bin/sqlite3").arg(&db).arg(sql).status().unwrap().success());
        let path = rollout.to_str().unwrap();
        assert_eq!(process_alive(path, |p| p != 31872), Some(false), "newest line names the exited pid");
        assert_eq!(process_alive(path, |_| true), Some(true));
        let other = path.replace("01a1d8a4-2f2f-7440-8f1b-6ea1aad24c1b", "01a1d8a4-0000-7440-8f1b-6ea1aad24c1b");
        assert_eq!(process_alive(&other, |_| false), None, "no row keeps the caller's default");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
