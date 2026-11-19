use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::path::Path;
use sha2::{Digest, Sha256};

pub fn validate(home: &Path, id: &str, incident: &Value, report: &Value) -> Result<()> {
    // Old incidents keep their original contract; new dispatches explicitly opt into this gate.
    if incident["requirements"]["resource_analysis"] != 1 { return Ok(()); }
    let dir=crate::monitor::root(home).join(format!("analysis/{id}"));
    let summary: Value=serde_json::from_slice(&std::fs::read(dir.join("resource-summary.json"))
        .context("run cpu-watchdog retro-resources before completing this retro")?)?;
    ensure!(summary["incident"]==id && summary["window"]==incident["window"], "resource summary belongs to another window");
    let sources=summary["resources"]["sources"].as_array().context("missing resource source inventory")?;
    for source in sources {
        let path=Path::new(source["path"].as_str().context("missing raw resource path")?);
        ensure!(path.parent()==Some(dir.join("raw/resources").as_path()),"invalid resource evidence path");
        let hash=format!("{:x}",Sha256::digest(std::fs::read(path)?));
        ensure!(source["sha256"]==hash,"raw resource evidence changed");
    }
    let analysis=&report["resource_analysis"];
    ensure!(analysis["summary"]=="resource-summary.json", "resource_analysis must reference resource-summary.json");
    for field in ["analysis","coverage","workload_basis"] {
        ensure!(analysis[field].as_str().is_some_and(|s|!s.trim().is_empty()),"resource_analysis needs {field}");
    }
    let workloads=analysis["workloads"].as_array().filter(|v|!v.is_empty()).context("classify workload counts as cpu/gpu/mixed/io_remote/unknown")?;
    let mut count=0u64;
    for workload in workloads {
        ensure!(matches!(workload["kind"].as_str(),Some("cpu"|"gpu"|"mixed"|"io_remote"|"unknown")),"invalid workload kind");
        count+=workload["count"].as_u64().context("workload count must be a nonnegative integer")?;
        nonempty(&workload["evidence"],"workload classification needs original evidence")?;
    }
    ensure!(count>0,"workload denominator must be positive");
    let opportunities=analysis["opportunities"].as_array().context("resource_analysis needs opportunities (empty only with no_opportunities_reason)")?;
    if opportunities.is_empty() {
        ensure!(analysis["no_opportunities_reason"].as_str().is_some_and(|s|!s.trim().is_empty()),"explain why no GPU opportunity is supported");
    }
    for item in opportunities {
        ensure!(item["task"].as_str().is_some_and(|s|!s.trim().is_empty()) && item["reason"].as_str().is_some_and(|s|!s.trim().is_empty()),"GPU candidate needs task and reason");
        nonempty(&item["evidence"],"GPU candidate needs source evidence")?;
        ensure!(matches!(item["decision"].as_str(),Some("optimized"|"not_beneficial"|"needs_measurement")),"invalid GPU candidate decision");
        if item["decision"]=="optimized" {
            ensure!(item["correctness_passed"]==true,"GPU optimization must preserve correctness");
            for field in ["before_ms","after_ms"] {
                let runs=item[field].as_array().filter(|r|r.len()>=3).context("GPU optimization needs at least three before/after runs")?;
                ensure!(runs.iter().all(|r|r.as_f64().is_some_and(|n|n.is_finite() && n>0.0)),"invalid benchmark time");
            }
            ensure!(item["conditions"].as_str().is_some_and(|s|!s.trim().is_empty()),"GPU benchmark needs comparable conditions");
            ensure!(item["change"].as_str().is_some_and(|s|!s.trim().is_empty()),"GPU optimization needs implementation evidence");
            let median=|field: &str| { let mut values: Vec<_>=item[field].as_array().unwrap().iter().map(|v|v.as_f64().unwrap()).collect(); values.sort_by(f64::total_cmp); let n=values.len(); (values[(n-1)/2]+values[n/2])/2.0 };
            ensure!(median("after_ms")<median("before_ms"),"GPU optimization must demonstrate a lower median runtime");
        }
    }
    Ok(())
}

fn nonempty(value: &Value, message: &str) -> Result<()> {
    ensure!(value.as_array().is_some_and(|a|!a.is_empty() && a.iter().all(|v|v.as_str().is_some_and(|s|!s.trim().is_empty()))),"{message}");
    Ok(())
}
