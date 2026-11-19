use super::*;

#[test]
fn gpu_counters_preserve_zero_but_never_invent_missing_activity() {
    let line="  | \"PerformanceStatistics\" = {\"Device Utilization %\"=0,\"Renderer Utilization %\"=12,\"Tiler Utilization %\"=4}";
    let gpu=parse(line).unwrap();
    assert_eq!(gpu["devices"][0]["device_utilization_pct"],0.0);
    assert_eq!(gpu["devices"][0]["renderer_utilization_pct"],12.0);
    assert!(parse("no GPU").is_err());
    assert!(parse(&line.replace("%=", "missing=").replace("%\"=0", "%\"=101")).is_err());
    assert!(parse("\"PerformanceStatistics\" = {\"Renderer Utilization %\"=2}").is_err());
    assert_eq!(parse(&format!("{line}\n{line}")).unwrap()["devices"].as_array().unwrap().len(),2);
}

#[test]
fn summary_clips_deduplicates_and_exposes_gaps_and_denominators() {
    let home=crate::telemetry::test_home("resource-summary");
    write_json(&root(&home).join("cpu-windows/0.json"),&json!({"samples":[
        {"start":0,"end":5000,"percent":20},{"start":5000,"end":10000,"percent":80},
        {"start":5000,"end":10000,"percent":80},{"start":-5000,"end":0,"percent":95}],
        "frames":[{"at_ms":5000,"cpu":{"consumers":[{"name":"rustc","cpu_percent_one_core":150},{"name":"other","cpu_percent_one_core":50}]}}]})).unwrap();
    let samples=root(&home).join("resource-samples");
    std::fs::create_dir_all(&samples).unwrap();
    std::fs::write(samples.join("0.jsonl"),format!("{}\n{}\n{}\n",
        json!({"at_ms":5000,"error":null,"gpu":{"devices":[{"device_utilization_pct":20}]}}),
        json!({"at_ms":9000,"error":null,"gpu":{"devices":[{"device_utilization_pct":40}]}}),
        json!({"at_ms":12000,"error":"unsupported"}))).unwrap();
    let dir=root(&home).join("analysis/42");
    let summary=summary::summarize(&home,2500.0,12500.0,&dir).unwrap();
    assert_eq!(summary["cpu"]["mean_host_pct"],60.0);
    assert_eq!(summary["cpu"]["coverage_pct"],75.0);
    assert_eq!(summary["cpu"]["consumers"][0]["share_of_top12_observed_cpu_pct"],75.0);
    assert_eq!(summary["gpu"]["mean_sampled_peak_device_pct"],30.0);
    assert_eq!(summary["gpu"]["errors"].as_array().unwrap().len(),1);
    assert!(Path::new(summary["sources"][0]["path"].as_str().unwrap()).exists());
    let empty=summary::summarize(&home,4_000_000.0,4_010_000.0,&dir).unwrap();
    assert!(empty["cpu"]["mean_host_pct"].is_null() && empty["gpu"]["mean_sampled_peak_device_pct"].is_null());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn completion_requires_analysis_and_real_optimization_measurements() {
    let home=crate::telemetry::test_home("resource-gate");
    let incident=json!({"requirements":{"resource_analysis":1},"window":{"from_ms":0,"to_ms":1000}});
    let dir=root(&home).join("analysis/42");
    write_json(&dir.join("resource-summary.json"),&json!({"incident":"42","window":incident["window"],"resources":{"sources":[]}})).unwrap();
    let mut report=json!({"resource_analysis":{"summary":"resource-summary.json","analysis":"No GPU task established.",
        "coverage":"No pre-install GPU history.","workload_basis":"One task from original transcript evidence.",
        "workloads":[{"kind":"unknown","count":1,"evidence":["/history/a:3"]}],
        "opportunities":[],"no_opportunities_reason":"No applicable kernel verified."}});
    assert!(validate(&home,"42",&incident,&json!({})).is_err());
    validate(&home,"42",&incident,&report).unwrap();
    report["resource_analysis"]["opportunities"]=json!([{"task":"image transform","reason":"Measured bottleneck", "evidence":["/history/a:3"],"decision":"optimized"}]);
    assert!(validate(&home,"42",&incident,&report).is_err());
    let item=&mut report["resource_analysis"]["opportunities"][0];
    item["before_ms"]=json!([100,101,102]);item["after_ms"]=json!([80,81,82]);
    item["correctness_passed"]=json!(true);item["conditions"]=json!("Identical inputs, warmed same host.");item["change"]=json!("kernel.rs:12");
    validate(&home,"42",&incident,&report).unwrap();
    report["resource_analysis"]["opportunities"][0]["after_ms"]=json!([120,121,122]);
    assert!(validate(&home,"42",&incident,&report).is_err());
    assert!(validate(&home,"43",&incident,&report).is_err());
    assert!(validate(&home,"42",&json!({}),&json!({})).is_ok());
    std::fs::remove_dir_all(home).unwrap();
}

/// Opt-in hardware verification keeps all state separate from resident incidents.
#[test]
#[ignore]
fn live_gpu_sample_and_resource_cli() {
    let home=crate::telemetry::test_home("resource-live");
    let from=crate::unix_now()*1000.0-1000.0;
    collect(&home).unwrap();
    let state: Value=serde_json::from_slice(&std::fs::read(root(&home).join("resource-usage.json")).unwrap()).unwrap();
    assert!(state["error"].is_null(),"{state}");
    let to=crate::unix_now()*1000.0+1.0;
    write_json(&root(&home).join("incidents/42.json"),&json!({"trigger":"conversation_count_reached","window":{"from_ms":from,"to_ms":to}})).unwrap();
    run(&home,vec!["42".into()]).unwrap();
    let path=root(&home).join("analysis/42/resource-summary.json");
    let summary: Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(summary["resources"]["gpu"]["samples"],1);
    assert!(summary["resources"]["cpu"]["mean_host_pct"].is_null());
    if let Some(out)=std::env::var_os("WATCHDOG_RESOURCE_RECEIPT_OUT") {
        std::fs::create_dir_all(&out).unwrap();
        std::fs::copy(path,Path::new(&out).join("live-resource-summary.json")).unwrap();
        std::fs::copy(root(&home).join("resource-usage.json"),Path::new(&out).join("live-resource-usage.json")).unwrap();
    }
    std::fs::remove_dir_all(home).unwrap();
}
