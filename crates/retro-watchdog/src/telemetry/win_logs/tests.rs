use super::*;

#[test]
fn utc_times_file_under_their_singapore_day() {
    assert_eq!(sgt_day("2026-11-06T15:59:59.000Z"), "2026-11-06");
    assert_eq!(sgt_day("2026-11-06T16:00:00.000Z"), "2026-11-07");
    assert_eq!(sgt_day("2026-12-31T20:00:00Z"), "2027-01-01");
    assert_eq!(sgt_day("2028-02-28T18:00:00Z"), "2028-02-29");
    assert_eq!(sgt_day("garbage"), "unknown");
    assert_eq!(iso(1_794_061_031.9), "2026-11-07T14:17:11Z");
}

#[test]
fn a_pull_renders_events_ssh_minutes_docker_and_a_changed_host() {
    // Shapes the probe returned from win on 2026-11-07 (the CPU swap of 2026-11-06 07:19 UTC included).
    let sample = json!({
        "events": [["2026-11-06T07:19:00.000Z", "System", 41, 1, "Kernel-Power", "系统已在未先正常关机的情况下重新启动。"]],
        "ssh": [["2026-11-07T12:27:05.000Z", "sshd: Connection reset by 192.0.2.33 port 52255"]],
        "minutes": {"2026-11-07T12:26": 15, "2026-11-07T16:01": 3},
        "docker": [["2026-11-07T15:30:19.681266900Z", "[2026-11-07T15:30:19.681266900Z] error CloseWrite"]],
        "cursor": {"System": 180300, "Application": 45655, "OpenSSH/Operational": 959198, "docker": "2026-11-07T15:40:01Z"},
        "counts": {"System": 3, "Application": 0, "OpenSSH/Operational": 120},
        "host": {"cpu": "AMD Ryzen 9 9950X3D2 16-Core Processor", "logical": 32, "boot": "2026-11-06T12:36:07.500Z"}
    });
    let before = json!({"cpu": "AMD Ryzen 7 9800X3D 8-Core Processor", "logical": 16, "boot": "2026-11-05T09:44:46.500Z"});
    let lines = render(&sample, &before, 1_794_061_031.0);
    let files: Vec<&str> = lines.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(files, ["2026-11-06/events.log", "2026-11-07/ssh.log", "2026-11-07/ssh.log", "2026-11-08/ssh.log", "2026-11-07/docker.log", "host.log"]);
    assert_eq!(lines[0].1, "2026-11-06T07:19:00.000Z System 41 L1 Kernel-Power 系统已在未先正常关机的情况下重新启动。");
    assert_eq!(lines[2].1, "2026-11-07T12:26:00Z sshd accepted 15 connection(s) this minute");
    assert!(lines[5].1.contains("logical=32") && lines[5].1.contains("was cpu=\"AMD Ryzen 7 9800X3D"), "{}", lines[5].1);
    // An unchanged host writes no host line.
    assert!(render(&sample, &sample["host"], 1_794_061_031.0).iter().all(|(f, _)| f != "host.log"));
}

#[test]
fn the_probe_carries_the_cursor_backfill_and_cap() {
    let first = script(&Value::Null);
    let cursor_line = first.lines().find(|l| l.starts_with("$cur=ConvertFrom-Json '")).expect("cursor line");
    let cursor: Value = serde_json::from_str(cursor_line.trim_start_matches("$cur=ConvertFrom-Json '").trim_end_matches('\'')).unwrap();
    assert_eq!(cursor, json!({"System": 0, "Application": 0, "OpenSSH/Operational": 0, "docker": ""}));
    assert!(first.contains("timediff(@SystemTime) <= 604800000") && first.contains("-MaxEvents 3000"));
    let next = script(&json!({"System": 180300, "docker": "2026-11-07T15:40:01Z"}));
    assert!(next.contains(r#""System":180300"#) && next.contains(r#""docker":"2026-11-07T15:40:01Z""#));
    let left = next.find("__").map(|i| next[i.saturating_sub(40)..(i + 40).min(next.len())].to_string());
    assert!(left.is_none(), "every placeholder is filled: {left:?}");
}

#[test]
fn lines_append_into_private_day_files() {
    let dir = std::env::temp_dir().join(format!("win-logs-test-{}", std::process::id()));
    write_lines(&dir, &[("2026-11-07/ssh.log".into(), "a".into()), ("2026-11-07/ssh.log".into(), "b".into())]).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("2026-11-07/ssh.log")).unwrap(), "a\nb\n");
    let _ = std::fs::remove_dir_all(&dir);
}
