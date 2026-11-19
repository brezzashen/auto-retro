use super::*;
use std::io::Write;

fn log(home: &Path) -> PathBuf {
    let path = home.join("Library/Containers/com.docker.docker/Data/log/vm/init.log");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    path
}
fn line(second: &str) -> String {
    format!(
        "{}\n",
        json!({"component":"kmsg","time":format!("2026-10-20T01:20:{second}Z"),
        "msg":"Out of memory: Killed process 42 (grafana) total-vm:1234kB"})
    )
}

#[test]
fn tail_ignores_history_deduplicates_partial_records_and_survives_rotation_restart() {
    let home = super::super::test_home("oom");
    let path = log(&home);
    std::fs::write(&path, line("00.000")).unwrap();
    observe(&home, 100.0).unwrap();
    assert!(!root(&home).join("incidents").exists());
    let context = json!({"component":"kmsg","time":"2026-10-20T01:20:01Z","msg":"oom-kill:constraint=CONSTRAINT_MEMCG task_memcg=/fixture"});
    let text = format!("malformed\n{context}\n{}{}", line("01.000"), line("01.001"));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(text.trim_end().as_bytes()).unwrap();
    assert_eq!(observe(&home, 115.0).unwrap()["new_incidents"], 1);
    file.write_all(b"\n").unwrap();
    assert_eq!(observe(&home, 130.0).unwrap()["new_incidents"], 0);
    let rotated = path.with_file_name("init.log.old");
    std::fs::rename(&path, &rotated).unwrap();
    file.write_all(line("02.000").as_bytes()).unwrap();
    drop(file);
    // Ensure ordering even on filesystems with coarse modification timestamps.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&path, line("03.000")).unwrap();
    assert_eq!(observe(&home, 145.0).unwrap()["new_incidents"], 2);
    assert_eq!(observe(&home, 160.0).unwrap()["new_incidents"], 0);
    let events = std::fs::read_dir(root(&home).join("incidents"))
        .unwrap()
        .count();
    assert_eq!(events, 3);
    let cursor: Cursor =
        serde_json::from_slice(&std::fs::read(root(&home).join("oom-cursor.json")).unwrap())
            .unwrap();
    let id = cursor.seen.last().unwrap();
    let first = cursor.seen.first().unwrap();
    let event: Value = serde_json::from_slice(
        &std::fs::read(root(&home).join("incidents").join(format!("{first}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(event["oom_scope"], "guest_global");
    assert!(event["kernel_context_before"]
        .to_string()
        .contains("task_memcg=/fixture"));
    assert!(crate::monitor::ack(&home, Some(id.clone())).is_err());
    let dir = root(&home).join("analysis").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    crate::log_recall::tests::capture_ok(&dir);
    crate::log_recall::tests::report(&dir);
    crate::monitor::ack(&home, Some(id.clone())).unwrap();
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn truncated_log_reports_gap_and_oom_victim_is_not_the_invoker() {
    let home = super::super::test_home("oom-truncate");
    let path = log(&home);
    std::fs::write(&path, "x".repeat(1000)).unwrap();
    observe(&home, 100.0).unwrap();
    std::fs::write(&path, line("04.000")).unwrap();
    let state = observe(&home, 115.0).unwrap();
    assert!(state["coverage_gap"].is_string());
    assert_eq!(state["new_incidents"], 1);
    assert!(
        victim(&json!({"component":"kmsg","time":"today","msg":"bun invoked oom-killer"}))
            .is_none()
    );
    assert!(victim(
        &json!({"component":"apiproxy","time":"today","msg":"Out of memory: Killed process 42"})
    )
    .is_none());
    std::fs::remove_dir_all(home).unwrap();
}
