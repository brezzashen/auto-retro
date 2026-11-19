use super::*;
use serde_json::json;

#[test]
fn no_quota_or_bad_ranking_never_falls_back_to_cc1() {
    assert!(select(&json!({"ranked": [], "skipped": [{"alias": "cc1"}]})).is_err());
    assert!(select(&json!({})).is_err());
    let co2 = json!({"agent": "codex", "alias": "co2", "home": "/u/.codex-profiles/default"});
    assert_eq!(select(&json!({"ranked": [co2.clone()]})).unwrap(), co2);
}

#[test]
fn selected_provider_owns_both_interactive_and_headless_launches() {
    for interactive in [true, false] {
        let co2 = shell(Path::new("/u"), &json!({"agent": "codex", "home": "/u/.codex-profiles/default"}), "n", "hello", interactive).unwrap();
        assert!(co2.contains("export CODEX_HOME='/u/.codex-profiles/default'"));
        assert!(co2.contains(if interactive { "codex' --no-daemon" } else { "codex' exec" }));
        assert!(!co2.contains("bin/claude"));
        let ag1 = shell(Path::new("/u"), &json!({"agent": "ag1", "home": "/u/.local/bin/ag1"}), "n", "hello", interactive).unwrap();
        assert!(ag1.contains(if interactive { "ag1' --no-chat -i" } else { "ag1' --no-chat -p" }));
        assert!(!ag1.contains("bin/claude"));
    }
}

#[test]
fn claude_default_login_keeps_its_keychain_namespace() {
    let default = shell(Path::new("/u"), &json!({"agent": "claude", "home": "/u/.claude"}), "n", "hello", true).unwrap();
    assert!(default.starts_with("unset CLAUDE_CONFIG_DIR &&"));
    let team = shell(Path::new("/u"), &json!({"agent": "claude", "home": "/u/.claude-profiles/team"}), "n", "hello", false).unwrap();
    assert!(team.starts_with("export CLAUDE_CONFIG_DIR='/u/.claude-profiles/team'"));
    assert!(team.contains("-p --name 'n' 'hello'"));
}

#[test]
fn a_led_session_names_its_model_in_both_launches_and_reports_a_fallback() {
    let fable = json!({"agent": "claude", "home": "/u/.claude-profiles/team", "model": {"family": "fable", "id": "fable"}});
    let orca = shell(Path::new("/u"), &fable, "n", "hello", true).unwrap();
    assert!(orca.ends_with("claude' --dangerously-skip-permissions --model 'fable' --remote-control 'n' 'hello'"), "{orca}");
    let headless = shell(Path::new("/u"), &fable, "n", "hello", false).unwrap();
    assert!(headless.ends_with("claude' --dangerously-skip-permissions --model 'fable' -p --name 'n' 'hello'"), "{headless}");
    assert_eq!(led(&fable, Some("fable")), json!({"lead": "fable", "model": "fable", "lead_fallback": false}));
    // Opus on a Claude account, another agent, and a ranking that names no model are fallbacks.
    let opus = json!({"agent": "claude", "home": "/u/.claude", "model": {"family": "opus", "id": "claude-opus-5-5"}});
    assert!(shell(Path::new("/u"), &opus, "n", "hello", true).unwrap().contains("--model 'claude-opus-5-5'"));
    assert_eq!(led(&opus, Some("fable")), json!({"lead": "fable", "model": "opus", "lead_fallback": true}));
    assert_eq!(led(&json!({"agent": "codex", "home": "/u/.codex-profiles/default", "model": null}), Some("fable"))["lead_fallback"], true);
    assert_eq!(led(&json!({"agent": "claude", "home": "/u/.claude"}), Some("fable"))["lead_fallback"], true);
    // A session without a lead keeps the profile's default and an unchanged receipt.
    assert!(led(&fable, None).is_null());
    assert_eq!(receipt(json!({"mode": "orca"}), &Value::Null), json!({"mode": "orca"}));
    assert_eq!(receipt(json!({"mode": "orca"}), &led(&fable, Some("fable")))["model"], "fable");
}

#[test]
fn unknown_agent_and_missing_home_fail_closed() {
    assert!(shell(Path::new("/u"), &json!({"agent": "unknown", "home": "/u/.unknown"}), "n", "p", true).is_err());
    assert!(shell(Path::new("/u"), &json!({"agent": "codex"}), "n", "p", true).is_err());
}

#[test]
fn prompt_and_profile_are_single_shell_arguments() {
    let p = "Don't execute $(touch /tmp/watchdog-injected) or `id`\nSecond line";
    let cmd = shell(Path::new("/u"), &json!({"agent": "codex", "home": "/u/it's $HOME"}), "n", p, true).unwrap();
    assert!(cmd.contains("CODEX_HOME='/u/it'\\''s $HOME'"));
    assert!(cmd.ends_with(&quote(p)));
}

#[test]
fn actual_shell_executes_the_selected_provider_and_preserves_prompt() {
    use std::os::unix::fs::PermissionsExt;
    let home = crate::telemetry::test_home("dispatch-launch");
    for (agent, path, config) in [
        ("claude", ".local/bin/claude", ".claude-profiles/team3"),
        ("ag1", ".local/bin/ag1", ".local/bin/ag1"),
        ("codex", ".local/shims/codex", ".codex-profiles/default"),
    ] {
        let file = home.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, format!("#!/bin/sh\nprintf '%s\\n' '{agent}' \"$CLAUDE_CONFIG_DIR\" \"$CODEX_HOME\" \"$@\"\n")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        let prompt = "Don't expand $HOME, $(false), `false`\nor drop this line";
        for interactive in [true, false] {
            let command = shell(&home, &json!({"agent": agent, "home": home.join(config)}), "probe", prompt, interactive).unwrap();
            let out = std::process::Command::new("/bin/zsh").args(["-c", &command])
                .env_remove("CODEX_HOME").env_remove("CLAUDE_CONFIG_DIR").output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let text = String::from_utf8(out.stdout).unwrap();
            assert!(text.starts_with(&format!("{agent}\n")) && text.ends_with(&format!("{prompt}\n")));
            if agent != "ag1" { assert!(text.contains(&home.join(config).to_string_lossy().to_string())); }
        }
    }
    std::fs::remove_dir_all(home).unwrap();
}
