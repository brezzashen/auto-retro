//! Consumes Mac Mini recovery handoffs inside the existing minute evidence worker.
use crate::monitor::{root,write_json};
use anyhow::Result;
use serde_json::{json,Value};
use std::path::Path;
pub fn poll(home:&Path)->Result<()> {
    let policy=root(home).join("network-analysis-policy.json");
    if !policy.exists(){return Ok(());}
    let policy:Value=serde_json::from_slice(&std::fs::read(policy)?)?;
    if policy["enabled"]!=true{return Ok(());}
    anyhow::ensure!(policy["account"]=="CO2","network analysis only permits CO2");
    let directory=root(home).join("network-analysis");
    use std::os::unix::fs::{DirBuilderExt,PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    std::fs::set_permissions(&directory,std::fs::Permissions::from_mode(0o700))?;
    let out=super::command::capture(home,home.join(".local/bin/network-path-watchdog").to_str().unwrap(),
        &["consume",directory.to_str().unwrap()],55)?;
    let status:Value=serde_json::from_str(&out)?;
    write_json(&root(home).join("network-analysis.json"),&json!({"at_ms":crate::unix_now()*1000.0,"account":"CO2","status":status}))
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn non_co2_policy_is_rejected_before_launch() {
        let h=super::super::test_home("network-account-policy");
        write_json(&root(&h).join("network-analysis-policy.json"),&json!({"enabled":true,"account":"CO1"})).unwrap();
        assert!(poll(&h).unwrap_err().to_string().contains("only permits CO2"));
        std::fs::remove_dir_all(h).unwrap();
    }
}
