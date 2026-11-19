use super::*;

const MIB: f64 = 1_048_576.0;

/// Shape of 2026-11-17 03:37 SGT: two simulator trees, one agent session with a helper, loose rows.
const PS: &str = "    1     0 /sbin/launchd\n\
  177     1 /System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer\n\
 7268     1 launchd_sim\n\
 7300  7268 /Library/Developer/CoreSimulator/Volumes/iOS_27C5/RuntimeRoot/System/Library/CoreServices/SpringBoard.app/SpringBoard\n\
 7301  7300 /Volumes/x/PosterBoard.app/MercuryPosterExtension\n\
10084     1 launchd_sim\n\
10100 10084 /usr/libexec/backboardd\n\
48323     1 /Applications/Orca.app/Contents/MacOS/Orca\n\
48399 48323 claude\n\
48410 48399 /home/user/.local/node/bin/node\n\
48500     1 /home/user/.local/bin/claude\n";

fn holder(pid: i64, name: &str, footprint_mib: f64, compressed_mib: f64) -> Holder {
    Holder { pid, name: name.into(), footprint: footprint_mib * MIB, compressed: compressed_mib * MIB }
}

#[test]
fn sums_small_processes_under_the_tree_that_roots_them() {
    let tree = parse_tree(PS);
    assert_eq!(tree[&7300], (7268, "SpringBoard".to_owned()), "the path is cut to its basename");
    let holders = [holder(177, "WindowServer", 4096.0, 1024.0), holder(7268, "launchd_sim", 10.0, 0.0), holder(7300, "SpringBoard", 2038.0, 1024.0),
        holder(7301, "MercuryPosterExt", 1024.0, 1024.0), holder(10100, "backboardd", 1024.0, 512.0), holder(48399, "2.1.325", 512.0, 0.0),
        holder(48410, "node", 512.0, 256.0), holder(48500, "2.1.325", 1024.0, 0.0), holder(48323, "Orca", 512.0, 0.0), holder(99999, "gone", 512.0, 0.0)];
    let (groups, total) = summarize(&holders, &tree);
    assert_eq!(groups[0], json!({"name": "simulators", "trees": 2, "processes": 4, "footprint_gib": 4.0, "compressed_gib": 2.5}));
    assert_eq!(groups[1], json!({"name": "agent sessions", "trees": 2, "processes": 3, "footprint_gib": 2.0, "compressed_gib": 0.25}),
        "a session's helper counts with it; its terminal host and a process that left the table do not");
    assert_eq!(total, json!({"processes": 10, "footprint_gib": 11.0, "compressed_gib": 3.75}));
}

#[test]
fn a_host_without_known_trees_has_no_group() {
    let (groups, total) = summarize(&[holder(177, "WindowServer", 1024.0, 0.0)], &parse_tree(PS));
    assert!(groups.is_empty());
    assert_eq!(total["footprint_gib"], 1.0);
    let cycle = parse_tree("5 6 a\n6 5 b\n");
    assert!(summarize(&[holder(5, "a", 1.0, 0.0)], &cycle).0.is_empty(), "a parent loop ends at the depth limit");
}

#[test]
fn android_emulators_are_one_group_of_single_process_trees() {
    let tree = parse_tree("    1     0 /sbin/launchd\n\
91096     1 /Volumes/MarsMedia/MobileDev/Android.noindex/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64-headless\n\
25858     1 /Volumes/MarsMedia/MobileDev/Android.noindex/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64-headless\n\
25900     1 /Volumes/MarsMedia/MobileDev/Android.noindex/sdk/emulator/netsimd\n");
    // `top` cuts the name to sixteen characters; the tree is matched by pid, not by that name.
    let holders = [holder(91096, "qemu-system-aarc", 8192.0, 6144.0), holder(25858, "qemu-system-aarc", 7168.0, 6144.0), holder(25900, "netsimd", 512.0, 0.0)];
    let (groups, total) = summarize(&holders, &tree);
    assert_eq!(groups, vec![json!({"name": "android emulators", "trees": 2, "processes": 2, "footprint_gib": 15.0, "compressed_gib": 12.0})]);
    assert_eq!(total["footprint_gib"], 15.5);
}
