use crate::monitor::{root, write_json};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::{Path, PathBuf}};

fn read(path: &Path) -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(path)?)?) }

pub fn run(home: &Path, args: Vec<String>) -> Result<()> {
    let id = args.first().context("usage: cpu-watchdog retro-resources <incident-id>")?;
    ensure!(args.len() == 1 && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()), "invalid incident id");
    let incident = read(&root(home).join(format!("incidents/{id}.json")))?;
    ensure!(incident["trigger"] == "conversation_count_reached", "not a conversation retro");
    let from = incident["window"]["from_ms"].as_f64().context("missing window start")?;
    let to = incident["window"]["to_ms"].as_f64().context("missing window end")?;
    ensure!(from.is_finite() && to.is_finite() && from >= 0.0 && to > from, "invalid window");
    let dir = root(home).join(format!("analysis/{id}"));
    let value = summarize(home, from, to, &dir)?;
    write_json(&dir.join("resource-summary.json"), &json!({"incident":id,"window":incident["window"],
        "at_ms":crate::unix_now()*1000.0,"resources":value}))?;
    println!("{}", dir.join("resource-summary.json").display());
    Ok(())
}

fn files(home: &Path, name: &str, from: f64, to: f64, bucket: u64) -> Result<Vec<PathBuf>> {
    let path = root(home).join(name);
    if !path.exists() { return Ok(vec![]); }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let path = entry.path();
        let Some(at) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok()) else { continue };
        if at as f64 <= to && (at + bucket) as f64 >= from { found.push(path); }
    }
    found.sort();
    Ok(found)
}

// Preserve the exact inputs before retention can expire or an open bucket can grow.
fn snapshot(path: &Path, dir: &Path, sources: &mut Vec<Value>) -> Result<String> {
    let bytes = std::fs::read(path)?;
    ensure!(bytes.len() <= 16 * 1024 * 1024, "resource archive too large: {}", path.display());
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let name = format!("{}-{}", path.parent().unwrap().file_name().unwrap().to_string_lossy(), path.file_name().unwrap().to_string_lossy());
    let copy = dir.join("raw/resources").join(name);
    use std::{io::Write, os::unix::fs::{DirBuilderExt, OpenOptionsExt}};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(copy.parent().unwrap())?;
    std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&copy)?.write_all(&bytes)?;
    sources.push(json!({"original":path,"path":copy,"sha256":digest}));
    Ok(String::from_utf8(bytes)?)
}

pub(super) fn summarize(home: &Path, from: f64, to: f64, dir: &Path) -> Result<Value> {
    let mut sources = Vec::new();
    let mut intervals = Vec::<(f64,f64,f64)>::new();
    let mut consumers = BTreeMap::<String, (f64,usize)>::new();
    let mut frame_count = 0;
    for path in files(home, "cpu-windows", from, to, 600_000)? {
        let window: Value = serde_json::from_str(&snapshot(&path, dir, &mut sources)?)?;
        for sample in window["samples"].as_array().into_iter().flatten() {
            let (Some(start),Some(end),Some(pct)) = (sample["start"].as_f64(),sample["end"].as_f64(),sample["percent"].as_f64()) else {continue};
            if start.is_finite() && end.is_finite() && end > start && end-start <= 15_000.0 && (0.0..=100.0).contains(&pct) && end > from && start < to {
                intervals.push((start.max(from),end.min(to),pct));
            }
        }
        for frame in window["frames"].as_array().into_iter().flatten() {
            if !frame["at_ms"].as_f64().is_some_and(|at| at >= from && at <= to) {continue;}
            frame_count += 1;
            for process in frame["cpu"]["consumers"].as_array().into_iter().flatten() {
                let (Some(name),Some(pct))=(process["name"].as_str(),process["cpu_percent_one_core"].as_f64()) else {continue};
                if pct.is_finite() && pct >= 0.0 { let entry=consumers.entry(name.to_string()).or_default(); entry.0+=pct; entry.1+=1; }
            }
        }
    }
    intervals.sort_by(|a,b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let (mut covered,mut weighted,mut end)=(0.0,0.0,from);
    for (start,stop,pct) in intervals { let start=start.max(end); if stop>start {covered+=stop-start;weighted+=(stop-start)*pct;end=stop;} }
    let mut gpu = Vec::<f64>::new();
    let mut gpu_times = Vec::<f64>::new();
    let mut errors = Vec::new();
    for path in files(home,"resource-samples",from,to,3_600_000)? {
        for line in snapshot(&path,dir,&mut sources)?.lines() {
            let sample: Value=serde_json::from_str(line)?;
            let Some(at)=sample["at_ms"].as_f64().filter(|at| *at>=from && *at<=to) else {continue};
            if !sample["error"].is_null() {errors.push(json!({"at_ms":at,"error":sample["error"]}));continue;}
            let peak=sample["gpu"]["devices"].as_array().into_iter().flatten()
                .filter_map(|d| d["device_utilization_pct"].as_f64()).filter(|p| (0.0..=100.0).contains(p)).max_by(f64::total_cmp);
            if let Some(pct)=peak {gpu.push(pct);gpu_times.push(at);}
        }
    }
    let total_cpu: f64=consumers.values().map(|v|v.0).sum();
    let mut top: Vec<_>=consumers.into_iter().map(|(name,(sum,n))|json!({"name":name,"observations":n,
        "mean_observed_pct_one_core":sum/n as f64,"share_of_top12_observed_cpu_pct":if total_cpu>0.0 {Some(100.0*sum/total_cpu)} else {None}})).collect();
    top.sort_by(|a,b| b["share_of_top12_observed_cpu_pct"].as_f64().unwrap_or(0.0).total_cmp(&a["share_of_top12_observed_cpu_pct"].as_f64().unwrap_or(0.0)));
    gpu_times.sort_by(f64::total_cmp);
    Ok(json!({"sources":sources,"cpu":{"mean_host_pct":if covered>0.0 {Some(weighted/covered)} else {None},
        "covered_ms":covered,"coverage_pct":100.0*covered/(to-from),"frames":frame_count,"consumers":top},
        "gpu":{"samples":gpu.len(),"mean_sampled_peak_device_pct":if gpu.is_empty(){None}else{Some(gpu.iter().sum::<f64>()/gpu.len() as f64)},
        "first_sample_ms":gpu_times.first(),"last_sample_ms":gpu_times.last(),
        "max_gap_ms":gpu_times.windows(2).map(|pair|pair[1]-pair[0]).max_by(f64::total_cmp),"errors":errors},
        "limits":["Host activity cannot identify which conversation or task caused it.",
        "CPU is time weighted with explicit coverage; GPU is a mean of driver snapshots, not continuous active time.",
        "CPU process shares cover only the observed top 12; they are not a share of all host work.",
        "CPU and GPU percentages have different denominators and must not be added; missing data is unknown."]}))
}
