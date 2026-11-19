//! Which beta services a board run actually rolls out, so T9 counts only rollouts both runs
//! replace. `deliver beta` deploys the plan's `services` (`api|crm|agent`,
//! `Mars_CRM/rust/crates/mars-ops/src/release/compact.rs`) and keeps every other image
//! (`k8s deploy --release` prints `api keep …`); `beta-web` ships the gateway api image and
//! `beta-crm` the crm image. Judge 87201392560521810218: an agent-only beta run was paired with a
//! beta-web run because its source's web paths map to the api image it never deployed.
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Service names (image repository names) the run deploys, or `null` when unknown.
pub(super) fn deploys(entry: &Value) -> Value {
    match entry["target"].as_str() {
        Some("beta-web") => json!(["api"]),
        Some("beta-crm") => json!(["crm"]),
        // `deliver beta` records `--plan=<absolute plan.json>` as its only input.
        Some("beta") => entry["inputs"].as_array().into_iter().flatten().filter_map(Value::as_str)
            .find_map(plan_services).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn plan_services(path: &str) -> Option<Value> {
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let services: Vec<&str> = plan["services"].as_array()?.iter().filter_map(Value::as_str).collect();
    (!services.is_empty()).then(|| json!(services))
}

/// Rollout keys in `env` of the images whose repository name is one of `services`.
fn rollouts_of(images: &Value, env: &str, services: &[Value]) -> BTreeSet<String> {
    let repo = |name: &str| name.rsplit('/').next().unwrap_or(name).split(':').next().unwrap_or("").to_string();
    images["images"].as_array().into_iter().flatten()
        .filter(|i| i["deployments"].as_array().into_iter().flatten().any(|d| d["env"] == env))
        .filter(|i| i["name"].as_str().is_some_and(|n| services.iter().any(|s| s.as_str() == Some(repo(n).as_str()))))
        .flat_map(|i| i["conflict_key"]["rollout"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string))
        .collect()
}

/// Keeps only the rollouts every run with a known `deploys` list actually replaces. An image map
/// that does not name every image of `env` cannot tell services apart, so it narrows nothing.
pub(super) fn narrow(rollouts: Vec<String>, runs: [&Value; 2], images: &Value, env: &str) -> Vec<String> {
    let in_env: Vec<&Value> = images["images"].as_array().into_iter().flatten()
        .filter(|i| i["deployments"].as_array().into_iter().flatten().any(|d| d["env"] == env)).collect();
    if in_env.is_empty() || in_env.iter().any(|i| !i["name"].is_string()) { return rollouts; }
    runs.iter().filter_map(|r| r["deploys"].as_array()).fold(rollouts, |kept, services| {
        let own = rollouts_of(images, env, services);
        kept.into_iter().filter(|k| own.contains(k)).collect()
    })
}
