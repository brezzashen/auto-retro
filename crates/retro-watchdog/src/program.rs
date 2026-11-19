//! Sanitized, cached program context for existing peak records; no argv or environment storage.
mod native;
mod source;

use crate::procs::Proc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::{
    path::Path,
    time::{Duration, SystemTime},
};

#[derive(Default)]
pub struct Programs {
    cache: VecDeque<(i32, i64, String, Value)>,
}

impl Programs {
    pub fn record(
        &mut self,
        home: &Path,
        p: &Proc,
        all: &[Proc],
        now: f64,
    ) -> anyhow::Result<Value> {
        let context = self.capture(p, all, now);
        let mut stable = context.clone();
        strip_time(&mut stable);
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&stable)?));
        let relative = format!(
            "program-records/{}-{}-{}.json",
            p.pid,
            p.start,
            &digest[..24]
        );
        let path = crate::monitor::root(home).join(&relative);
        if !path.exists() {
            crate::monitor::write_json(&path, &context)?;
        } else if path.metadata()?.modified()?.elapsed().unwrap_or_default()
            > Duration::from_secs(86400)
        {
            std::fs::File::open(&path)?.set_modified(SystemTime::now())?;
        }
        Ok(json!({"record":relative,"captured_at_ms":context["captured_at_ms"]}))
    }

    pub fn capture(&mut self, p: &Proc, all: &[Proc], now: f64) -> Value {
        if let Some((_, _, _, value)) = self.cache.iter().find(|(pid, start, name, value)| {
            *pid == p.pid
                && (*start - p.start).abs() <= 2
                && *name == p.name
                && value["captured_at_ms"]
                    .as_f64()
                    .is_some_and(|at| now * 1000.0 >= at && now * 1000.0 - at < 60_000.0)
        }) {
            return value.clone();
        }
        let mut value = capture(p, now);
        if value["identity_verified"] == true {
            let mut ancestors = Vec::new();
            let mut parent = p.ppid;
            for _ in 0..4 {
                let Some(item) = all.iter().find(|item| item.pid == parent) else {
                    break;
                };
                if item.pid == p.pid || ancestors.iter().any(|v: &Value| v["pid"] == item.pid) {
                    break;
                }
                let mut context = capture(item, now);
                context.as_object_mut().unwrap().remove("repository");
                ancestors.push(context);
                parent = item.ppid;
            }
            value["ancestors"] = json!(ancestors);
        }
        if self.cache.len() >= 128 {
            self.cache.pop_front();
        }
        self.cache
            .push_back((p.pid, p.start, p.name.clone(), value.clone()));
        value
    }
}

fn strip_time(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("captured_at_ms");
            for v in map.values_mut() {
                strip_time(v);
            }
        }
        Value::Array(rows) => {
            for v in rows {
                strip_time(v);
            }
        }
        _ => {}
    }
}

fn capture(p: &Proc, now: f64) -> Value {
    let mut value = json!({"schema":"process-context.v1","captured_at_ms":now*1000.0,
        "pid":p.pid,"ppid":p.ppid,"start":p.start,"name":p.name,"identity_verified":false,
        "arguments":"omitted","environment":"omitted","executable":null,"cwd":null,
        "entrypoint":null,"operation":null,"repository":null,
        "replay":"Metadata only; review invocation and inputs before rerunning"});
    if !native::same(p) {
        value["unavailable_reason"] = json!("Process exited, was reused, or libproc denied access");
        return value;
    }
    let (executable, cwd) = native::paths(p.pid);
    let (entry, operation) = source::invocation(&p.name, &p.args, cwd.as_deref());
    if !native::same(p) {
        value["unavailable_reason"] = json!("Process identity changed during capture");
        return value;
    }
    value["identity_verified"] = json!(true);
    value["executable"] = executable
        .as_deref()
        .map(|p| source::file(p, false))
        .unwrap_or(Value::Null);
    value["cwd"] = json!(cwd);
    value["entrypoint"] = entry
        .as_deref()
        .map(|p| source::file(p, true))
        .unwrap_or(Value::Null);
    value["operation"] = json!(operation);
    value["repository"] = cwd
        .as_deref()
        .and_then(source::repository)
        .unwrap_or(Value::Null);
    value["source_semantics"] =
        json!("Filesystem at capture, not proof of loaded code; source hash limited to 256 KiB");
    value
}

pub fn inspect(pid: i32) -> anyhow::Result<()> {
    let (now, all) = crate::procs::snapshot(crate::unix_now)?;
    let p = all
        .iter()
        .find(|p| p.pid == pid)
        .ok_or_else(|| anyhow::anyhow!("Process not found"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&Programs::default().capture(p, &all, now))?
    );
    Ok(())
}

#[cfg(test)]
#[path = "program/tests.rs"]
mod tests;
