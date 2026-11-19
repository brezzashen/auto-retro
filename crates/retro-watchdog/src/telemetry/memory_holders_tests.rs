use super::*;

/// `top -l 1 -o mem -n 5 -stats pid,mem,cmprs,command` on 2026-11-15 01:35 SGT (incident 1794671950116).
const TOP_2026_11_15: &str = "Processes: 2687 total, 9 running, 2678 sleeping, 12925 threads \n\
PhysMem: 95G used (24G wired, 31G compressor), 228M unused.\n\
Disks: 4230762397/123T read, 740009806/25T written.\n\n\
PID    MEM   CMPRS COMMAND         \n\
109    39G   30G   fseventsd       \n\
177    4430M+ 1756M WindowServer    \n\
88242  3205M 3175M- Figma Helper (Re\n\
34386* 514M  3264K VTDecoderXPCServ\n\
0      683M  0B    kernel_task     \n";

#[test]
fn parses_footprint_and_compressed_columns_of_top() {
    let holders = parse(TOP_2026_11_15);
    assert_eq!(holders.len(), 5);
    assert_eq!(holders[0], Holder { pid: 109, name: "fseventsd".into(), footprint: 39.0 * GIB, compressed: 30.0 * GIB });
    assert_eq!(holders[1].footprint, 4430.0 * 1_048_576.0, "a changed column carries + or -");
    assert_eq!((holders[2].name.as_str(), holders[2].compressed), ("Figma Helper (Re", 3175.0 * 1_048_576.0), "names keep their spaces");
    assert_eq!((holders[3].pid, holders[3].compressed), (34386, 3264.0 * 1024.0));
    assert_eq!(holders[4].compressed, 0.0);
    assert!(parse("PhysMem: 95G used\n").is_empty(), "no header, no rows");
}

#[test]
fn names_the_largest_holder_for_the_incident_detail() {
    let captured = json!({"holders": [{"pid": 109, "name": "fseventsd", "footprint_gib": 38.31, "compressed_gib": 30.0}]});
    assert_eq!(largest(&captured).as_deref(), Some("fseventsd 38.3 GiB"));
    assert_eq!(largest(&json!({"error": "evidence probe timed out after 20s", "holders": []})), None);
}

#[test]
fn a_larger_process_tree_group_is_named_instead_of_the_largest_row() {
    // 2026-11-17 03:06 SGT (incident 1794856006950): the detail line read "largest holder WindowServer 4.1 GiB".
    let mut captured = json!({"holders": [{"pid": 177, "name": "WindowServer", "footprint_gib": 4.11, "compressed_gib": 1.86}],
        "groups": [{"name": "simulators", "trees": 6, "processes": 1180, "footprint_gib": 58.2, "compressed_gib": 54.0}]});
    assert_eq!(largest(&captured).as_deref(), Some("simulators 58.2 GiB (6 trees, 1180 processes)"));
    captured["groups"][0]["footprint_gib"] = json!(3.0);
    assert_eq!(largest(&captured).as_deref(), Some("WindowServer 4.1 GiB"));
    assert_eq!(largest_holder(&json!({"holders": [], "groups": []})), None);
}

/// Live probe: `cargo test -p cpu-watchdog live_capture -- --ignored --nocapture`.
#[test]
#[ignore = "reads the live process table of this host"]
fn live_capture_sums_groups_and_the_host_total() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let captured = capture(&home);
    println!("{}", json!({"groups": captured["groups"], "total": captured["total"], "largest": largest(&captured), "error": captured["groups_error"]}));
    assert!(captured["total"]["processes"].as_u64().unwrap() > 100, "every row counts, not only the kept ones");
    assert_eq!(captured["holders"].as_array().unwrap().len(), KEEP);
}
