mod common;

use std::process::{Command, Output};

use common::{Project, rule};
use serde_json::{Value, json};

fn cli(project: &Project, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_erislint"))
        .current_dir(project.root())
        .env_remove("jev_key")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn dry_run_discovers_config_from_subdirectories_and_never_needs_a_key() {
    let project = Project::new();
    project.json("erislint.json", &json!({ "rules": [rule("quality")] }));
    project.write("src/lib.rs", "fn task() {}");
    let output = Command::new(env!("CARGO_BIN_EXE_erislint"))
        .current_dir(project.root().join("src"))
        .env_remove("jev_key")
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["evaluations"][0]["request"]["state"]["name"], "task");
    assert_eq!(plan["evaluations"][0]["location"]["file"], "src/lib.rs");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Bearer"));
}

#[test]
fn configuration_check_is_offline_and_invalid_input_exits_two() {
    let project = Project::new();
    project.json("erislint.json", &json!({ "rules": [rule("quality")] }));
    assert!(cli(&project, &["--check-config"]).status.success());
    project.json("erislint.json", &json!({ "rules": [], "unexpected": true }));
    let output = cli(&project, &["--check-config"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field"));
}

#[test]
fn reports_missing_auth_and_parse_failures_as_operational_errors() {
    let project = Project::new();
    project.json("erislint.json", &json!({ "rules": [rule("quality")] }));
    project.write("src/lib.rs", "fn task() {}");
    let output = cli(&project, &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("jev_key"));
    project.write("src/lib.rs", "fn task( {");
    let output = cli(&project, &["--dry-run"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Rust syntax errors"));
}

#[test]
fn schemas_need_no_config_and_empty_evaluation_needs_no_key() {
    let project = Project::new();
    for kind in ["config", "rule"] {
        let output = cli(&project, &["--schema", kind]);
        assert!(output.status.success());
        let schema: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(schema.get("$schema").is_some());
    }
    project.json("erislint.json", &json!({ "rules": [rule("quality")] }));
    project.write("src/lib.rs", "struct Empty;");
    let output = cli(&project, &["--format", "json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["evaluations"], 0);
    assert_eq!(report["diagnostics"], json!([]));
}

#[cfg(unix)]
#[test]
fn malformed_secret_is_not_echoed_in_errors() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let project = Project::new();
    project.json("erislint.json", &json!({ "rules": [rule("quality")] }));
    project.write("src/lib.rs", "fn task() {}");
    let output = Command::new(env!("CARGO_BIN_EXE_erislint"))
        .current_dir(project.root())
        .env(
            "jev_key",
            OsString::from_vec(b"private-test-secret\xff".to_vec()),
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-test-secret"));
}

#[test]
fn errors_only_is_a_native_cli_mode() {
    let project = Project::new();
    project.json("erislint.json", &json!({ "rules": [rule("quality")] }));
    project.write("src/lib.rs", "struct Empty;");
    let output = cli(&project, &["--errors-only", "--color", "never"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "No errors found.\n"
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        cli(&project, &["--errors-only", "--all-answers"])
            .status
            .code(),
        Some(2)
    );
}
