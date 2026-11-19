use super::*;
use std::fs;

#[test]
fn native_context_identifies_this_process_and_does_not_store_arguments() {
    let (now, all) = crate::procs::snapshot(crate::unix_now).unwrap();
    let mut p = all
        .iter()
        .find(|p| p.pid == std::process::id() as i32)
        .unwrap()
        .clone();
    p.args = "cpu-watchdog --password secret-marker --token secret-marker".into();
    let value = Programs::default().capture(&p, &all, now);
    assert_eq!(value["identity_verified"], true);
    assert!(
        value["executable"]["path"]
            .as_str()
            .unwrap()
            .contains("retro_watchdog")
    );
    assert!(value["cwd"].as_str().is_some());
    assert!(!value.to_string().contains("secret-marker"));
    p.start -= 100;
    let stale = Programs::default().capture(&p, &all, now);
    assert_eq!(stale["identity_verified"], false);
    assert!(stale["cwd"].is_null());
}

#[test]
fn source_reads_worktree_refs_without_git_or_dirty_claims() {
    let home = crate::telemetry::test_home("program-repo");
    let common = home.join("repo/.git");
    let work = home.join("work");
    let git = common.join("worktrees/demo");
    fs::create_dir_all(&git).unwrap();
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join(".git"), format!("gitdir: {}\n", git.display())).unwrap();
    fs::write(git.join("commondir"), "../..\n").unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/demo\n").unwrap();
    let sha = "a".repeat(40);
    fs::write(
        common.join("packed-refs"),
        format!("{sha} refs/heads/demo\n"),
    )
    .unwrap();
    let value = source::repository(&work).unwrap();
    assert_eq!(value["head"], sha);
    assert_eq!(value["branch"], "demo");
    assert_eq!(value["worktree_state"], "not_measured");
    fs::write(git.join("HEAD"), "ref: refs/../../outside\n").unwrap();
    assert!(source::repository(&work).unwrap()["head"].is_null());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn only_entry_file_and_fixed_operation_survive_argument_projection() {
    let home = crate::telemetry::test_home("program-source");
    fs::write(home.join("task.ts"), "console.log('example')").unwrap();
    let (entry, operation) =
        source::invocation("bun", "bun task.ts --token secret-marker", Some(&home));
    assert_eq!(
        entry.as_deref(),
        Some(home.join("task.ts").canonicalize().unwrap().as_path())
    );
    assert!(operation.is_none());
    let metadata = source::file(entry.as_ref().unwrap(), true);
    assert_eq!(metadata["sha256"].as_str().unwrap().len(), 64);
    assert!(!metadata.to_string().contains("example"));
    assert_eq!(
        source::invocation("bun", "bun run test:secret-marker", Some(&home))
            .1
            .as_deref(),
        Some("test:*")
    );
    assert!(
        source::invocation("bun", "bun -e secret-marker.ts", Some(&home))
            .0
            .is_none()
    );
    fs::write(home.join("large.ts"), vec![b'x'; 256 * 1024 + 1]).unwrap();
    assert!(source::file(&home.join("large.ts"), true)["sha256"].is_null());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn detached_head_and_changed_source_have_distinct_evidence() {
    let home = crate::telemetry::test_home("program-detached");
    fs::create_dir_all(home.join(".git")).unwrap();
    fs::write(home.join(".git/HEAD"), "b".repeat(40)).unwrap();
    let value = source::repository(&home).unwrap();
    assert!(value["branch"].is_null());
    assert_eq!(value["head"], "b".repeat(40));
    fs::write(home.join("task.ts"), "one").unwrap();
    let first = source::file(&home.join("task.ts"), true);
    fs::write(home.join("task.ts"), "two").unwrap();
    assert_ne!(
        first["sha256"],
        source::file(&home.join("task.ts"), true)["sha256"]
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn archive_deduplicates_unchanged_context_and_uses_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let home = crate::telemetry::test_home("program-record");
    let (now, all) = crate::procs::snapshot(crate::unix_now).unwrap();
    let p = all
        .iter()
        .find(|p| p.pid == std::process::id() as i32)
        .unwrap();
    let mut programs = Programs::default();
    let first = programs.record(&home, p, &all, now).unwrap();
    let second = programs.record(&home, p, &all, now + 61.0).unwrap();
    assert_eq!(first["record"], second["record"]);
    let path = crate::monitor::root(&home).join(first["record"].as_str().unwrap());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["pid"], p.pid);
    assert_eq!(value["identity_verified"], true);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    fs::remove_dir_all(home).unwrap();
}
