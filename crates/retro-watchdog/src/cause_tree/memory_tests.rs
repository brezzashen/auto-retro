use super::*;

fn tree(status: &str) -> Value {
    json!({"nodes": [
        {"id": "N8.4.2", "parent": "N8.4", "status": status, "probe": PROBE, "match": ["fseventsd"], "min_gib": 16.0},
        {"id": "N8.13", "parent": "N8", "status": "closed", "probe": PROBE, "match": ["Python"]},
    ]})
}

fn event(trigger: &str, name: &str, footprint: f64) -> Value {
    json!({"trigger": trigger, "relief": {"holders": {"holders": [
        {"pid": 109, "name": name, "footprint_gib": footprint, "compressed_gib": 30.0},
        {"pid": 177, "name": "WindowServer", "footprint_gib": 4.3, "compressed_gib": 1.7}]}}})
}

#[test]
fn the_largest_holder_names_its_node_when_it_holds_enough() {
    assert_eq!(probe(&tree("closed"), &event(TRIGGER, "fseventsd", 38.3)).unwrap(),
        json!({"node": "N8.4.2", "holder": "fseventsd", "footprint_gib": 38.3, "min_gib": 16.0, "explains": true}));
    let small = probe(&tree("closed"), &event(TRIGGER, "fseventsd", 9.0)).unwrap();
    assert_eq!((&small["node"], &small["explains"]), (&json!("N8.4.2"), &json!(false)), "below min_gib it is only named");
    assert_eq!(probe(&tree("open"), &event(TRIGGER, "fseventsd", 38.3)).unwrap()["explains"], false, "an open node explains nothing");
    assert_eq!(probe(&tree("closed"), &event(TRIGGER, "Python", 36.0)).unwrap()["node"], "N8.13", "min_gib defaults to 16");
}

#[test]
fn an_unknown_holder_is_reported_without_a_node() {
    let found = probe(&tree("closed"), &event(TRIGGER, "Blender", 40.0)).unwrap();
    assert_eq!(found, json!({"node": null, "holder": "Blender", "footprint_gib": 40.0, "min_gib": 16.0, "explains": false}));
}

#[test]
fn other_triggers_and_older_incidents_have_no_result() {
    assert!(probe(&tree("closed"), &event("windowserver_hot", "fseventsd", 38.3)).is_none());
    assert!(probe(&tree("closed"), &json!({"trigger": TRIGGER, "relief": {"shut_down": []}})).is_none());
    assert!(probe(&tree("closed"), &json!({"trigger": TRIGGER, "relief": {"holders": {"error": "timed out", "holders": []}}})).is_none());
}

#[test]
fn a_process_tree_group_larger_than_every_process_is_the_holder() {
    // 2026-11-17 03:06 SGT: WindowServer 4.11 GiB led the rows while six simulators held about 58 GiB.
    let tree = json!({"nodes": [{"id": "N8.7.1", "parent": "N8.7", "status": "closed", "probe": PROBE, "match": ["simulators"], "min_gib": 48.0}]});
    let mut event = event(TRIGGER, "WindowServer", 4.11);
    event["relief"]["holders"]["groups"] = json!([{"name": "simulators", "trees": 6, "processes": 1180, "footprint_gib": 58.2, "compressed_gib": 54.0},
        {"name": "agent sessions", "trees": 38, "processes": 253, "footprint_gib": 18.1, "compressed_gib": 7.4}]);
    assert_eq!(probe(&tree, &event).unwrap(), json!({"node": "N8.7.1", "holder": "simulators", "footprint_gib": 58.2, "min_gib": 48.0, "explains": true}));
    event["relief"]["holders"]["groups"][0]["footprint_gib"] = json!(38.8);
    assert_eq!(probe(&tree, &event).unwrap()["explains"], false, "four devices are named but stay below min_gib");
    event["relief"]["holders"]["holders"][0] = json!({"pid": 109, "name": "fseventsd", "footprint_gib": 45.0, "compressed_gib": 30.0});
    assert_eq!(probe(&tree, &event).unwrap()["holder"], "fseventsd", "a larger single process still wins");
}
