use std::{fs, process::Command};

use tempfile::tempdir;

fn write_example_rules(root: &std::path::Path) {
    let directory = root.join("example");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("rules.json"),
        r#"{"schema_version":1,"product":{"id":"example","name":"Example"},"verified":{"repository":"https://example.invalid/repo","commit":"0123456789abcdef0123456789abcdef01234567"},"rules":[{"id":"hook-marker","surfaces":["claude.hooks.user"],"evidence":{"type":"command_marker","marker":"example-hook"}}]}"#,
    )
    .unwrap();
}

#[test]
fn scan_cli_reads_synthetic_home_and_reports_candidate() {
    let temporary = tempdir().unwrap();
    let rules = temporary.path().join("rules");
    let home = temporary.path().join("synthetic-home");
    write_example_rules(&rules);
    let config = home.join(".claude/settings.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"runner example-hook --private-token=fixture-only"}]}]}}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_akcleaner"))
        .args([
            "scan",
            "--rules-dir",
            rules.to_str().unwrap(),
            "--home",
            home.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["findings"].as_array().unwrap().len(), 1);
    assert_eq!(report["findings"][0]["attribution"], "candidate");
    assert_eq!(report["findings"][0]["integrity"], "unknown");
    assert!(report["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(report["coverage"]["project_directories_scanned"], false);
    let serialized = String::from_utf8(output.stdout).unwrap();
    assert!(!serialized.contains("private-token"));
    assert!(!serialized.contains("fixture-only"));
}

#[test]
fn cli_help_and_version_keep_clap_success_semantics() {
    let binary = env!("CARGO_BIN_EXE_akcleaner");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("交互式清理 agent 集成"));

    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("akcleaner "));
}

#[test]
fn no_command_and_clean_route_to_interactive_mode_and_refuse_non_tty() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("synthetic-home");
    fs::create_dir_all(&home).unwrap();
    let binary = env!("CARGO_BIN_EXE_akcleaner");

    for args in [vec![], vec!["clean", "--home", home.to_str().unwrap()]] {
        let output = Command::new(binary)
            .args(args)
            .current_dir(temporary.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stderr.is_empty());
    }
    assert!(!home.join(".local/state/akcleaner/backups").exists());
}

#[test]
fn clean_unknown_product_is_an_error_and_there_is_no_yes_flag() {
    let temporary = tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_akcleaner");
    let unknown = Command::new(binary)
        .args(["clean", "--product", "missing-product"])
        .current_dir(temporary.path())
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(unknown.stderr.is_empty());

    let yes = Command::new(binary)
        .args(["clean", "--yes"])
        .current_dir(temporary.path())
        .output()
        .unwrap();
    assert!(!yes.status.success());
    assert!(String::from_utf8_lossy(&yes.stderr).contains("unexpected argument"));
}

#[test]
fn bundled_scan_rules_work_from_an_unrelated_working_directory() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("synthetic-home");
    let unrelated_cwd = temporary.path().join("elsewhere");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&unrelated_cwd).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_akcleaner"))
        .args(["scan", "--home", home.to_str().unwrap(), "--json"])
        .current_dir(unrelated_cwd)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!output.stdout.contains(&0x1b));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["findings"].as_array().unwrap().is_empty());
    assert!(report["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn text_scan_escapes_control_characters_from_skill_paths() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("synthetic-home");
    let hostile_name = "skill\u{1b}[31m";
    let skill = home
        .join(".agents/skills")
        .join(hostile_name)
        .join("SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, "<!-- superset-managed-skill v1 -->\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_akcleaner"))
        .args(["scan", "--home", home.to_str().unwrap()])
        .current_dir(temporary.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains('\u{1b}'));
    assert!(text.contains("\\u{1b}[31m"));
}

#[test]
fn scan_cli_reports_missing_and_non_directory_home() {
    let temporary = tempdir().unwrap();
    let rules = temporary.path().join("rules");
    write_example_rules(&rules);
    let binary = env!("CARGO_BIN_EXE_akcleaner");

    let missing = Command::new(binary)
        .args([
            "scan",
            "--rules-dir",
            rules.to_str().unwrap(),
            "--home",
            temporary.path().join("missing").to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    let report: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(report["diagnostics"][0]["code"], "home_missing");

    let file = temporary.path().join("not-a-home");
    fs::write(&file, b"synthetic").unwrap();
    let not_directory = Command::new(binary)
        .args([
            "scan",
            "--rules-dir",
            rules.to_str().unwrap(),
            "--home",
            file.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!not_directory.status.success());
    let report: serde_json::Value = serde_json::from_slice(&not_directory.stdout).unwrap();
    assert_eq!(report["diagnostics"][0]["code"], "home_not_directory");
}

#[test]
fn rules_check_missing_directory_and_unknown_product_are_errors() {
    let temporary = tempdir().unwrap();
    let missing_rules = Command::new(env!("CARGO_BIN_EXE_akcleaner"))
        .args([
            "rules",
            "check",
            "--rules-dir",
            temporary.path().join("missing").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!missing_rules.status.success());
    assert!(String::from_utf8_lossy(&missing_rules.stderr).contains("does not exist"));

    let rules = temporary.path().join("rules");
    let home = temporary.path().join("synthetic-home");
    write_example_rules(&rules);
    fs::create_dir_all(&home).unwrap();

    let valid_rules = Command::new(env!("CARGO_BIN_EXE_akcleaner"))
        .args(["rules", "check", "--rules-dir", rules.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(valid_rules.status.success());
    assert!(
        String::from_utf8_lossy(&valid_rules.stdout).contains("Rules valid: 1 products, 1 rules.")
    );

    let unknown_product = Command::new(env!("CARGO_BIN_EXE_akcleaner"))
        .args([
            "scan",
            "--rules-dir",
            rules.to_str().unwrap(),
            "--home",
            home.to_str().unwrap(),
            "--product",
            "not-installed",
        ])
        .output()
        .unwrap();
    assert!(!unknown_product.status.success());
    assert!(String::from_utf8_lossy(&unknown_product.stderr).contains("unknown product id"));
}
