use super::command::capture;
use crate::monitor::{root, write_json};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Default, Deserialize, Serialize)]
struct Store {
    #[serde(default)]
    alerts_started_at_ms: Option<f64>,
    manifests: BTreeMap<PathBuf, u128>,
    logs: Vec<Value>,
}

pub fn artifacts(args: &str) -> Value {
    let words: Vec<_> = args.split_whitespace().collect();
    let mut result = serde_json::Map::new();
    for (flag, name) in [
        ("-derivedDataPath", "derived_data"),
        ("-resultBundlePath", "result_bundle"),
        ("-resultStreamPath", "result_stream"),
    ] {
        if let Some(index) = words.iter().position(|word| *word == flag) {
            if let Some(path) = words.get(index + 1) {
                if path.starts_with('/') && !path.contains(['\'', '"']) {
                    result.insert(name.into(), json!(path));
                }
            }
        }
    }
    Value::Object(result)
}

fn artifact_available(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        let file_type = metadata.file_type();
        file_type.is_file()
            || (file_type.is_dir() && path.extension().is_some_and(|ext| ext == "xcresult"))
    })
}

pub fn collect(home: &Path) -> Result<()> {
    let checkpoint = root(home).join("xcode-index.json");
    let mut store: Store = match std::fs::read(&checkpoint) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Store::default(),
        Err(error) => return Err(error.into()),
    };
    let alerts_started_at_ms = *store.alerts_started_at_ms.get_or_insert(crate::unix_now() * 1000.0);
    for record in &mut store.logs {
        let available = record["activity_log"]
            .as_str()
            .is_some_and(|path| artifact_available(Path::new(path)));
        if record["activity_log_exists"].as_bool() == Some(available) {
            continue;
        }
        record["activity_log_exists"] = json!(available);
        if let (Some(manifest), Some(id)) = (record["manifest"].as_str(), record["id"].as_str()) {
            let path = root(home)
                .join("xcode-records")
                .join(format!("{}.json", safe_id(Path::new(manifest), id)));
            write_json(&path, record)?;
        }
    }
    let derived = home.join("Library/Developer/Xcode/DerivedData");
    let mut directories = Vec::new();
    if derived.is_dir() {
        for entry in std::fs::read_dir(&derived)?.take(512) {
            directories.push(entry?.path());
        }
    }
    if let Some(tasks) = super::read(&root(home).join("tasks.json")) {
        if let Some(tasks) = tasks["tasks"].as_array() {
            directories.extend(tasks.iter().filter_map(|task| {
                task["artifacts"]["derived_data"]
                    .as_str()
                    .map(PathBuf::from)
            }));
        }
    }
    directories.sort();
    directories.dedup();
    let mut manifests = Vec::new();
    for directory in directories {
        for kind in ["Build", "Test", "Package"] {
            let path = directory
                .join("Logs")
                .join(kind)
                .join("LogStoreManifest.plist");
            if let Ok(metadata) = std::fs::metadata(&path) {
                let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
                if store.manifests.get(&path) != Some(&modified) {
                    manifests.push((path, modified));
                }
            }
        }
    }
    manifests.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    let pending = manifests.len().saturating_sub(16);
    let mut errors = Vec::new();
    let mut skipped_logs = 0;
    for (path, modified) in manifests.into_iter().take(16) {
        match capture(
            home,
            "/usr/bin/plutil",
            &["-convert", "json", "-o", "-", path.to_str().unwrap_or("")],
            2,
        )
        .and_then(|text| Ok(serde_json::from_str::<Value>(&text)?))
        {
            Ok(manifest) => {
                if let Some(logs) = manifest["logs"].as_object() {
                    let mut logs: Vec<_> = logs.iter().collect();
                    logs.sort_by(|(_, a), (_, b)| {
                        b["timeStartedRecording"]
                            .as_f64()
                            .unwrap_or(0.0)
                            .total_cmp(&a["timeStartedRecording"].as_f64().unwrap_or(0.0))
                    });
                    skipped_logs += logs.len().saturating_sub(32);
                    for (id, log) in logs.into_iter().take(32) {
                        let start = log["timeStartedRecording"]
                            .as_f64()
                            .map(|time| (time + 978307200.0) * 1000.0);
                        let stop = log["timeStoppedRecording"]
                            .as_f64()
                            .map(|time| (time + 978307200.0) * 1000.0);
                        let activity = log["fileName"]
                            .as_str()
                            .filter(|name| Path::new(name).components().count() == 1)
                            .map(|name| path.parent().unwrap().join(name));
                        let value = json!({"id":id,"manifest":path,"activity_log":activity,
                            "activity_log_exists":activity.as_ref().is_some_and(|path|artifact_available(path)),
                            "scheme":log["schemeIdentifier-schemeName"],"started_at_ms":start,
                            "finished_at_ms":stop,"observable":log["primaryObservable"],
                            "log_class":log["className"]});
                        if stop.is_some_and(|stop| stop > alerts_started_at_ms)
                            && (log["primaryObservable"]["highLevelStatus"] == "E"
                                || log["primaryObservable"]["totalNumberOfErrors"].as_u64().unwrap_or(0) > 0
                                || log["primaryObservable"]["totalNumberOfTestFailures"].as_u64().unwrap_or(0) > 0)
                        {
                            super::diagnostics::incident(home, &format!("build:{}:{id}", path.display()),
                                json!({"trigger":"xcode_build_or_test_failed","record":value}))?;
                        }
                        if store.logs.iter().any(|old| old == &value) {
                            continue;
                        }
                        let record = root(home)
                            .join("xcode-records")
                            .join(format!("{}.json", safe_id(&path, id)));
                        write_json(&record, &value)?;
                        store
                            .logs
                            .retain(|old| old["id"] != *id || old["manifest"] != value["manifest"]);
                        store.logs.push(value);
                    }
                }
                store.manifests.insert(path, modified);
            }
            Err(error) => errors.push(json!({"manifest":path,"error":format!("{error:#}")})),
        }
    }
    store.logs.sort_by(|a, b| {
        b["started_at_ms"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&a["started_at_ms"].as_f64().unwrap_or(0.0))
    });
    store.logs.truncate(128);
    write_json(&checkpoint, &store)?;
    write_json(
        &root(home).join("xcode.json"),
        &json!({"at_ms":crate::unix_now()*1000.0,
        "error":null,"probe_errors":errors,"pending_changed_manifests":pending,"indexed_manifests":store.manifests.len(),
        "skipped_older_log_entries":skipped_logs,"logs":store.logs,"semantics":"Apple reference seconds converted to Unix ms; metadata plus original log paths; no automatic xcactivitylog decoding; discovery bounded to 512 default directories, 16 changed manifests per probe and 32 recent entries per manifest; explicit paths with whitespace require incident-time resolution"}),
    )
}

fn safe_id(path: &Path, id: &str) -> String {
    let key = format!("{}:{id}", path.display());
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    });
    format!("{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_failed_build_alerts_once_without_replaying_historical_failures() {
        let home = super::super::test_home("xcode-alert");
        let directory = home.join("Library/Developer/Xcode/DerivedData/fixture/Logs/Build");
        std::fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("LogStoreManifest.plist");
        let make = |stop| json!({"logs":{"failed":{"timeStartedRecording":1.0,
            "timeStoppedRecording":stop,"primaryObservable":{"highLevelStatus":"E","totalNumberOfErrors":1}}}});
        std::fs::write(&manifest,serde_json::to_vec(&make(2.0)).unwrap()).unwrap();
        collect(&home).unwrap();
        assert!(!root(&home).join("incidents").exists());
        std::fs::write(&manifest,serde_json::to_vec(&make(crate::unix_now()-978307200.0+1.0)).unwrap()).unwrap();
        collect(&home).unwrap();
        collect(&home).unwrap();
        assert_eq!(std::fs::read_dir(root(&home).join("incidents")).unwrap().count(),1);
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn artifacts_omit_command_and_result_paths_are_not_log_ids() {
        let value=artifacts("xcodebuild -derivedDataPath /tmp/dd -resultBundlePath /tmp/result.xcresult TOKEN=secret");
        assert_eq!(value["derived_data"], "/tmp/dd");
        assert_eq!(value["result_bundle"], "/tmp/result.xcresult");
        assert!(!value.to_string().contains("secret"));
        assert!(safe_id(Path::new("/tmp/a"), "../../evil")
            .bytes()
            .all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn xcode_manifest_times_observables_and_record_identity_are_preserved() {
        let home = super::super::test_home("xcode");
        let directory = home.join("Library/Developer/Xcode/DerivedData/fixture/Logs/Build");
        std::fs::create_dir_all(&directory).unwrap();
        let manifest = json!({"logs":{"abc":{"fileName":"abc.xcactivitylog","timeStartedRecording":10.0,
            "timeStoppedRecording":20.0,"schemeIdentifier-schemeName":"Fixture",
            "primaryObservable":{"totalNumberOfErrors":2,"totalNumberOfWarnings":3},"className":"IDECommandLineBuildLog"}}});
        // plutil accepts JSON input as well as XML/binary plists.
        std::fs::write(
            directory.join("LogStoreManifest.plist"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(directory.join("abc.xcactivitylog"), "raw fixture").unwrap();
        collect(&home).unwrap();
        let state = super::super::read(&root(&home).join("xcode.json")).unwrap();
        assert_eq!(state["logs"][0]["started_at_ms"], 978307210000.0);
        assert_eq!(state["logs"][0]["finished_at_ms"], 978307220000.0);
        assert_eq!(state["logs"][0]["observable"]["totalNumberOfErrors"], 2);
        assert_eq!(state["logs"][0]["activity_log_exists"], true);
        collect(&home).unwrap();
        assert_eq!(
            std::fs::read_dir(root(&home).join("xcode-records"))
                .unwrap()
                .count(),
            1
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn result_bundle_directories_and_activity_logs_are_available_but_missing_or_other_directories_are_not(
    ) {
        let home = super::super::test_home("xcode-artifacts");
        let directory = home.join("Library/Developer/Xcode/DerivedData/fixture/Logs/Test");
        std::fs::create_dir_all(directory.join("run.xcresult")).unwrap();
        std::fs::create_dir_all(directory.join("unrelated")).unwrap();
        std::fs::write(directory.join("run.xcactivitylog"), "raw fixture").unwrap();
        let manifest = json!({"logs":{
            "bundle":{"fileName":"run.xcresult"},
            "activity":{"fileName":"run.xcactivitylog"},
            "missing":{"fileName":"missing.xcresult"},
            "directory":{"fileName":"unrelated"}
        }});
        std::fs::write(
            directory.join("LogStoreManifest.plist"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        collect(&home).unwrap();
        let state = super::super::read(&root(&home).join("xcode.json")).unwrap();
        let availability: BTreeMap<_, _> = state["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["id"].as_str().unwrap(),
                    entry["activity_log_exists"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(availability["bundle"], true);
        assert_eq!(availability["activity"], true);
        assert_eq!(availability["missing"], false);
        assert_eq!(availability["directory"], false);
        assert_eq!(
            state["logs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["id"] == "bundle")
                .unwrap()["activity_log"],
            directory.join("run.xcresult").to_string_lossy().as_ref()
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn unchanged_manifest_refreshes_persisted_result_bundle_availability() {
        let home = super::super::test_home("xcode-stale-artifact");
        let directory = home.join("Library/Developer/Xcode/DerivedData/fixture/Logs/Test");
        std::fs::create_dir_all(directory.join("run.xcresult")).unwrap();
        let manifest = directory.join("LogStoreManifest.plist");
        std::fs::write(&manifest, "{}").unwrap();
        let modified = std::fs::metadata(&manifest)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let stale = json!({"id":"bundle","manifest":manifest,"activity_log":directory.join("run.xcresult"),
            "activity_log_exists":false,"started_at_ms":1.0});
        let store = Store {
            alerts_started_at_ms: None,
            manifests: BTreeMap::from([(manifest.clone(), modified)]),
            logs: vec![stale.clone()],
        };
        std::fs::create_dir_all(root(&home)).unwrap();
        std::fs::write(
            root(&home).join("xcode-index.json"),
            serde_json::to_vec(&store).unwrap(),
        )
        .unwrap();
        let record_path = root(&home)
            .join("xcode-records")
            .join(format!("{}.json", safe_id(&manifest, "bundle")));
        write_json(&record_path, &stale).unwrap();

        collect(&home).unwrap();
        let state = super::super::read(&root(&home).join("xcode.json")).unwrap();
        assert_eq!(state["pending_changed_manifests"], 0);
        assert_eq!(state["logs"][0]["activity_log_exists"], true);
        assert_eq!(
            super::super::read(&record_path).unwrap()["activity_log_exists"],
            true
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
