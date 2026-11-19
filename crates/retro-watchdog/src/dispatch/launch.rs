use super::{quote, REPO};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub(super) fn select(report: &Value) -> Result<Value> {
    report["ranked"].as_array().context("account ranking has no ranked array")?
        .first().cloned().context("no account has enough fresh quota; watchdog session was not launched")
}

/// What leads a session `lead` must lead on the ranked `account`: `{lead, model, lead_fallback}`,
/// `model` being the family its `route --model` entry names. `lead_fallback` is true when that is
/// not `lead`: another model, another agent, or a ranking that named no model. `Null` without `lead`.
pub(super) fn led(account: &Value, lead: Option<&str>) -> Value {
    let Some(lead) = lead else { return Value::Null };
    let model = account["model"]["family"].as_str().filter(|_| account["agent"] == "claude");
    json!({"lead": lead, "model": model, "lead_fallback": model != Some(lead)})
}

/// The launch receipt with the `led` fields added.
pub(super) fn receipt(mut receipt: Value, led: &Value) -> Value {
    if let (Some(receipt), Some(led)) = (receipt.as_object_mut(), led.as_object()) { receipt.extend(led.clone()); }
    receipt
}

pub(super) fn shell(home: &Path, account: &Value, name: &str, text: &str, interactive: bool) -> Result<String> {
    let config = account["home"].as_str().context("ranked account has no home")?;
    let binary = |name: &str| quote(&home.join(".local/bin").join(name).to_string_lossy());
    let text = quote(text);
    let (env, args) = match account["agent"].as_str() {
        Some("claude") => {
            let env = if Path::new(config) == home.join(".claude") {
                "unset CLAUDE_CONFIG_DIR".to_string()
            } else { format!("export CLAUDE_CONFIG_DIR={}", quote(config)) };
            let mode = if interactive { format!("--remote-control {}", quote(name)) }
                else { format!("-p --name {}", quote(name)) };
            // A led session names its model; every other one starts on the profile's default.
            let model = account["model"]["id"].as_str().map_or(String::new(), |id| format!(" --model {}", quote(id)));
            (env, format!("{} --dangerously-skip-permissions{model} {mode} {text}", binary("claude")))
        }
        Some("ag1") => ("unset CLAUDE_CONFIG_DIR".to_string(),
            format!("{} --no-chat {} {text}", quote(config), if interactive { "-i" } else { "-p" })),
        Some("codex") => (format!("unset CLAUDE_CONFIG_DIR; export CODEX_HOME={}", quote(config)),
            format!("{} {} --dangerously-bypass-approvals-and-sandbox {text}", quote(&home.join(".local/shims/codex").to_string_lossy()),
                if interactive { "--no-daemon" } else { "exec" })),
        other => bail!("unsupported watchdog agent {other:?}; refusing to launch Claude with another account"),
    };
    Ok(format!("{env} && cd {} && exec {args}", quote(REPO)))
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
