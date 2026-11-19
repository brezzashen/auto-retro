use super::*;

fn args(command: &Command) -> Vec<String> {
    command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

fn env(command: &Command, key: &str) -> Option<String> {
    command.get_envs().find(|(k, _)| *k == key).and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cpu-watchdog-crm-ops-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(format!("tools/{PACKAGE}"))).unwrap();
    dir
}

/// 2026-11-06..16: `mars cache-watch` ran package `mars-ops`, which has no pg client, so every run
/// failed with "a pg session runs in mars-ops-crm". The probe must run `mars-ops-crm`.
#[test]
fn a_prebuilt_mars_ops_crm_runs_the_probe_directly() {
    let cache = temp("prebuilt");
    let binary = cache.join(format!("tools/{PACKAGE}/abc/{PACKAGE}"));
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, "").unwrap();
    std::fs::write(cache.join(format!("tools/{PACKAGE}/prebuilt.json")),
        serde_json::json!({"binary": binary, "sha": "4eddef21"}).to_string()).unwrap();
    let command = command_in(&cache, Path::new("/r/Mars_CRM"), &["cache-watch"]);
    assert_eq!(command.get_program(), binary.as_os_str());
    assert_eq!(args(&command), ["cache-watch"]);
    assert_eq!(command.get_current_dir(), Some(Path::new("/r/Mars_CRM")));
    assert_eq!(env(&command, "MARS_CRM_ROOT").as_deref(), Some("/r/Mars_CRM"));
    assert_eq!(env(&command, "CPU_BUILD_GATE_TOOLING").as_deref(), Some("4eddef21"));
    assert_eq!(env(&command, "PATH").as_deref(), Some(crate::telemetry::MARS_SCRIPT_PATH));
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn without_a_prebuilt_binary_the_gate_builds_mars_ops_crm_not_mars_ops() {
    let cache = temp("missing");
    // A record whose binary is gone counts as no prebuilt.
    std::fs::write(cache.join(format!("tools/{PACKAGE}/prebuilt.json")), r#"{"binary":"/nonexistent/mars-ops-crm","sha":"x"}"#).unwrap();
    let command = command_in(&cache, Path::new("/r/Mars_CRM"), &["quota-economics", "--period", "weekly"]);
    assert_eq!(command.get_program(), "cpu-build-gate");
    assert_eq!(args(&command).join(" "),
        "cargo-run --shared-cache --root-env MARS_CRM_ROOT=/r/Mars_CRM -- --quiet --release --manifest-path /r/Mars_CRM/rust/Cargo.toml -p mars-ops-crm -- quota-economics --period weekly");
    let _ = std::fs::remove_dir_all(&cache);
}

/// The fallback must start (2026-10-26: cargo missing from the PATH made every run exit 127), and the
/// Mars_CRM checkout must still hold the package the probes run.
#[test]
#[ignore = "requires local host workspace checkout"]
fn the_fallback_resolves_on_the_script_path_and_the_package_exists() {
    let on_path = |program: &str| crate::telemetry::MARS_SCRIPT_PATH.split(':').any(|dir| Path::new(dir).join(program).is_file());
    assert!(on_path("cpu-build-gate"), "cpu-build-gate is not on MARS_SCRIPT_PATH");
    assert!(on_path("cargo"), "cargo is not on MARS_SCRIPT_PATH");
    assert!(Path::new(CRM).join(format!("rust/crates/{PACKAGE}/Cargo.toml")).is_file(), "{PACKAGE} is gone from {CRM}");
}
