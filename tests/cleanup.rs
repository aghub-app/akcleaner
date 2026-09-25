use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use akcleaner::{
    cleanup::{self, ChangeAction, CleanupError},
    discovery::{self, Attribution, Finding, Integrity},
    rules::{RuleSet, load_rules},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

fn load_all_rules() -> Vec<RuleSet> {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    load_rules(&repository.join("rules")).expect("bundled first-round rules load")
}

fn write(home: &Path, relative: &str, contents: impl AsRef<[u8]>) {
    let path = home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn findings(home: &Path, rules: &[RuleSet]) -> Vec<Finding> {
    discovery::scan(home, rules).findings
}

fn sha256(contents: &[u8]) -> String {
    let digest = Sha256::digest(contents);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn rule<'a>(rules: &'a [RuleSet], product_id: &str) -> &'a RuleSet {
    rules
        .iter()
        .find(|ruleset| ruleset.product.id == product_id)
        .unwrap()
}

#[test]
fn hooks_remove_selected_original_indices_preserve_users_and_multi_product_file() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let config = r#"{
  "userSetting"  : "retain exact bytes",
  "hooks": {
    "Stop": [
      { "matcher": "", "hooks": [
        {"type":"command","command":"muxy-run muxy-notification-hook"},
        {"type":"command","command":"superset-run $SUPERSET_HOME_DIR/hooks/notify.sh"},
        {"type":"command","command":"private-user-command-DO-NOT-LEAK"}
      ]},
      { "hooks": [{"type":"command","command":"second muxy-notification-hook"}] }
    ],
    "Notification": [{"hooks": [{"type":"command","command":"only muxy-notification-hook"}]}]
  }
}"#;
    write(&home, ".claude/settings.json", config);
    let rules = load_all_rules();
    let selected = findings(&home, &rules)
        .into_iter()
        .filter(|finding| finding.surface == "claude.hooks.user")
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 4, "two products share the same file");

    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    assert_eq!(plan.changes.len(), 1);
    assert_eq!(plan.changes[0].action, ChangeAction::EditJson);
    assert_eq!(plan.changes[0].item_count, 4);
    assert!(!plan.backup_root.exists(), "prepare is read-only");
    let debug_plan = format!("{plan:?}");
    assert!(!debug_plan.contains("private-user-command-DO-NOT-LEAK"));
    assert!(!debug_plan.contains("muxy-run"));

    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 4);
    assert_eq!(result.changed_files, 1);
    assert!(result.failures.is_empty());
    let output = fs::read_to_string(home.join(".claude/settings.json")).unwrap();
    assert!(output.contains("\"userSetting\"  : \"retain exact bytes\""));
    assert!(output.contains("private-user-command-DO-NOT-LEAK"));
    assert!(!output.contains("muxy-notification-hook"));
    assert!(!output.contains("$SUPERSET_HOME_DIR/hooks/notify.sh"));
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert_eq!(
        parsed["hooks"]["Stop"][0]["hooks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(parsed["hooks"].get("Notification").is_none());
    assert!(
        findings(&home, &rules).is_empty(),
        "post-clean scan is idempotent"
    );
}

#[test]
fn candidate_hook_removal_requires_explicit_selection() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &home,
        ".codex/hooks.json",
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"muxy-notification-hook selected"},{"type":"command","command":"$SUPERSET_HOME_DIR/hooks/notify.sh retained"},{"type":"command","command":"user"}]}]}}"#,
    );
    let rules = load_all_rules();
    let candidates = findings(&home, &rules)
        .into_iter()
        .filter(|finding| finding.product_id == "muxy")
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].attribution, Attribution::Candidate);
    let plan = cleanup::prepare(&home, &rules, &candidates).unwrap();
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 1);
    let output = fs::read_to_string(home.join(".codex/hooks.json")).unwrap();
    assert!(!output.contains("muxy-notification-hook"));
    assert!(output.contains("$SUPERSET_HOME_DIR/hooks/notify.sh retained"));
    assert!(output.contains("\"command\":\"user\""));
}

#[test]
fn marker_skill_removes_only_skill_document_and_keeps_directory_contents() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    let skill = home.join(".agents/skills/custom-helper");
    fs::create_dir_all(skill.join("references")).unwrap();
    write(
        &home,
        ".agents/skills/custom-helper/SKILL.md",
        b"<!-- superset-managed-skill v1 -->\nprivate-skill-body\n",
    );
    fs::write(skill.join("README.md"), b"user-authored notes\n").unwrap();
    fs::write(skill.join("references/guide.md"), b"additional user file\n").unwrap();
    let rules = load_all_rules();
    let selected = findings(&home, &rules);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].integrity, Integrity::Unknown);
    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    assert_eq!(plan.changes.len(), 1);
    assert_eq!(plan.changes[0].path, skill.join("SKILL.md"));
    assert_eq!(plan.changes[0].action, ChangeAction::RemoveFile);
    assert!(!format!("{plan:?}").contains("private-skill-body"));
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 1);
    assert!(!skill.join("SKILL.md").exists());
    assert!(skill.join("README.md").exists());
    assert!(skill.join("references/guide.md").exists());
    assert!(skill.is_dir());
}

#[test]
fn paseo_manifest_remains_discoverable_after_only_skill_md_is_removed() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    let skill = home.join(".agents/skills/portable");
    fs::create_dir_all(&skill).unwrap();
    let skill_text = b"<!-- superset-managed-skill v1 -->\n";
    let helper_text = b"Paseo managed helper\n";
    fs::write(skill.join("SKILL.md"), skill_text).unwrap();
    fs::write(skill.join("helper.sh"), helper_text).unwrap();
    fs::write(skill.join("user-notes.txt"), b"keep this file\n").unwrap();
    fs::write(
        skill.join(".paseo-managed-files.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "files": { "helper.sh": sha256(helper_text) }
        }))
        .unwrap(),
    )
    .unwrap();

    let rules = load_all_rules();
    let initial = findings(&home, &rules);
    let marker = initial
        .iter()
        .find(|finding| finding.product_id == "superset")
        .unwrap()
        .clone();
    cleanup::execute(cleanup::prepare(&home, &rules, &[marker]).unwrap()).unwrap();
    assert!(!skill.join("SKILL.md").exists());
    assert!(skill.join("helper.sh").exists());
    assert!(skill.join(".paseo-managed-files.json").exists());

    let second_scan = findings(&home, &rules);
    assert_eq!(second_scan.len(), 1);
    assert_eq!(second_scan[0].product_id, "paseo");
    assert_eq!(second_scan[0].path, skill.join("helper.sh"));
    assert_eq!(second_scan[0].integrity, Integrity::Unchanged);

    cleanup::execute(cleanup::prepare(&home, &rules, &second_scan).unwrap()).unwrap();
    assert!(!skill.join("helper.sh").exists());
    assert!(!skill.join(".paseo-managed-files.json").exists());
    assert!(skill.join("user-notes.txt").exists());
    assert!(skill.is_dir());
}

#[test]
fn empty_selection_has_no_backup_or_filesystem_side_effects() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &home,
        ".claude/settings.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"muxy-notification-hook"}]}]}}"#,
    );
    let rules = load_all_rules();
    let config_before = fs::read(home.join(".claude/settings.json")).unwrap();
    let plan = cleanup::prepare(&home, &rules, &[]).unwrap();
    let backup = plan.backup_root.clone();
    assert!(plan.changes.is_empty());
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 0);
    assert_eq!(result.changed_files, 0);
    assert_eq!(result.backup_path, None);
    assert!(!backup.exists());
    assert!(!home.join(".local").exists());
    assert_eq!(
        fs::read(home.join(".claude/settings.json")).unwrap(),
        config_before
    );
}

fn manifest_home() -> (tempfile::TempDir, PathBuf, Vec<RuleSet>, Vec<Finding>) {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    let skill = home.join(".agents/skills/reviewer");
    fs::create_dir_all(skill.join("references")).unwrap();
    let skill_text = b"# managed skill document\n";
    let guide_text = b"# managed guide\n";
    fs::write(skill.join("SKILL.md"), skill_text).unwrap();
    fs::write(skill.join("references/guide.md"), guide_text).unwrap();
    fs::write(skill.join("private-notes.txt"), b"user data\n").unwrap();
    let manifest = json!({
        "version": 1,
        "files": {
            "SKILL.md": sha256(skill_text),
            "references/guide.md": sha256(guide_text),
        }
    });
    fs::write(
        skill.join(".paseo-managed-files.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let rules = load_all_rules();
    let findings = findings(&home, &rules)
        .into_iter()
        .filter(|finding| finding.product_id == "paseo")
        .collect();
    (temporary, home, rules, findings)
}

#[test]
fn manifest_partial_cleanup_updates_only_successfully_removed_nested_file() {
    let (_temporary, home, rules, discovered) = manifest_home();
    assert_eq!(discovered.len(), 2);
    let selected = discovered
        .iter()
        .find(|finding| finding.path.ends_with("SKILL.md"))
        .unwrap()
        .clone();
    let plan = cleanup::prepare(&home, &rules, &[selected]).unwrap();
    assert_eq!(plan.changes.len(), 2);
    assert!(!plan.backup_root.exists());
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 1);
    assert_eq!(result.changed_files, 2);
    let skill = home.join(".agents/skills/reviewer");
    assert!(!skill.join("SKILL.md").exists());
    assert!(skill.join("references/guide.md").exists());
    assert!(skill.join("private-notes.txt").exists());
    let manifest: Value =
        serde_json::from_slice(&fs::read(skill.join(".paseo-managed-files.json")).unwrap())
            .unwrap();
    assert!(manifest["files"].get("SKILL.md").is_none());
    assert!(manifest["files"].get("references/guide.md").is_some());
    assert!(skill.is_dir(), "cleanup leaves skill directories in place");
}

#[test]
fn manifest_full_cleanup_removes_only_listed_files_and_manifest() {
    let (_temporary, home, rules, discovered) = manifest_home();
    let plan = cleanup::prepare(&home, &rules, &discovered).unwrap();
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 2);
    let skill = home.join(".agents/skills/reviewer");
    assert!(!skill.join("SKILL.md").exists());
    assert!(!skill.join("references/guide.md").exists());
    assert!(!skill.join(".paseo-managed-files.json").exists());
    assert!(skill.join("private-notes.txt").exists());
    assert!(skill.is_dir());
}

fn mcp_home() -> (tempfile::TempDir, PathBuf, Vec<RuleSet>, Vec<Finding>) {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let linear_hash = "e48a98e3871f8c18d31f1524d8461c6c773621faec367faf259a38d1b2af697d";
    let keep_hash = "dbfc17dcdcd96a97683c1318400bd330ebfaedbfbec8a6abac53899c681c0711";
    let special_hash = "8d2cc3be522940dd6f457d86b9a951d529cff352b9c2bbe267fe0d634b03eae3";
    let config = json!({
        "theme": "keep",
        "mcpServers": {
            "linear": {
                "command": "VERY-PRIVATE-MCP-COMMAND",
                "args": ["--mode", "safe"],
                "env": {"HOME": "/tmp/synthetic", "PATH": "/usr/bin", "USER": "tester"},
                "type": "stdio"
            },
            "keep": {"command": "private-keep-command", "env": {"PATH": "/usr/bin"}},
            "sp/ace~server": {"command": "private-special-command"},
            "unverifiable": {"command": "unknown", "args": [1]}
        }
    });
    write(
        &home,
        ".claude.json",
        serde_json::to_vec_pretty(&config).unwrap(),
    );
    let config_path = home.join(".claude.json").to_string_lossy().into_owned();
    let ledger = json!({
        "version": 1,
        "files": {
            (config_path): {
                "linear": linear_hash,
                "keep": keep_hash,
                "sp/ace~server": special_hash,
                "unverifiable": "0".repeat(64)
            },
            "/another/synthetic/.claude.json": {"unselected": "a".repeat(64)}
        }
    });
    write(
        &home,
        ".superset/plugins/mcp-ledger.json",
        serde_json::to_vec_pretty(&ledger).unwrap(),
    );
    let rules = load_all_rules();
    let findings = findings(&home, &rules)
        .into_iter()
        .filter(|finding| finding.product_id == "superset" && finding.surface == "claude.mcp.user")
        .collect();
    (temporary, home, rules, findings)
}

#[test]
fn mcp_removes_exact_key_and_updates_ledger_without_leaking_command_values() {
    let (_temporary, home, rules, discovered) = mcp_home();
    let linear = discovered
        .iter()
        .find(|finding| finding.locator == "/mcpServers/linear")
        .unwrap()
        .clone();
    let keep = discovered
        .iter()
        .find(|finding| finding.locator == "/mcpServers/keep")
        .unwrap();
    let special = discovered
        .iter()
        .find(|finding| finding.locator == "/mcpServers/sp~1ace~0server")
        .unwrap()
        .clone();
    assert_eq!(linear.integrity, Integrity::Unchanged);
    assert_eq!(keep.integrity, Integrity::Unchanged);
    assert_eq!(special.integrity, Integrity::Unchanged);
    let plan = cleanup::prepare(&home, &rules, &[linear, special]).unwrap();
    let debug = format!("{plan:?}");
    assert!(!debug.contains("VERY-PRIVATE-MCP-COMMAND"));
    assert!(!debug.contains("private-keep-command"));
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 2);
    assert_eq!(result.changed_files, 2);

    let config: Value =
        serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(config["theme"], "keep");
    assert!(config["mcpServers"].get("linear").is_none());
    assert!(config["mcpServers"].get("sp/ace~server").is_none());
    assert!(config["mcpServers"].get("keep").is_some());
    assert!(config["mcpServers"].get("unverifiable").is_some());
    let ledger: Value =
        serde_json::from_slice(&fs::read(home.join(".superset/plugins/mcp-ledger.json")).unwrap())
            .unwrap();
    let config_path = home.join(".claude.json").to_string_lossy().into_owned();
    assert!(ledger["files"][&config_path].get("linear").is_none());
    assert!(ledger["files"][&config_path].get("sp/ace~server").is_none());
    assert!(ledger["files"][&config_path].get("keep").is_some());
    assert!(ledger["files"]["/another/synthetic/.claude.json"].is_object());

    let receipt: Value = serde_json::from_slice(
        &fs::read(result.backup_path.unwrap().join("execution-receipt.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["operations"].as_array().unwrap().len(), 2);
    assert!(
        receipt["operations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|operation| operation["status"] == "succeeded")
    );
    assert!(
        !serde_json::to_string(&receipt)
            .unwrap()
            .contains("VERY-PRIVATE-MCP-COMMAND")
    );
}

#[test]
fn modified_unknown_and_conflicting_findings_are_skipped() {
    let (_temporary, home, rules, mcp_findings) = mcp_home();
    let unknown = mcp_findings
        .iter()
        .find(|finding| finding.locator == "/mcpServers/unverifiable")
        .unwrap()
        .clone();
    assert_eq!(unknown.integrity, Integrity::Unknown);
    let unknown_plan = cleanup::prepare(&home, &rules, &[unknown]).unwrap();
    assert!(unknown_plan.changes.is_empty());
    assert_eq!(unknown_plan.skipped.len(), 1);
    assert!(!unknown_plan.backup_root.exists());

    let (_temporary, home, rules, discovered) = manifest_home();
    let manifest = home.join(".agents/skills/reviewer/.paseo-managed-files.json");
    let document: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    let original_hash = document["files"]["SKILL.md"].as_str().unwrap();
    write(
        &home,
        ".agents/skills/reviewer/SKILL.md",
        b"edited after install\n",
    );
    assert_ne!(sha256(b"edited after install\n"), original_hash);
    let modified = findings(&home, &rules)
        .into_iter()
        .find(|finding| finding.path.ends_with("SKILL.md"))
        .unwrap();
    assert_eq!(modified.integrity, Integrity::Modified);
    let plan = cleanup::prepare(&home, &rules, &[modified]).unwrap();
    assert!(plan.changes.is_empty());
    assert_eq!(plan.skipped.len(), 1);
    assert_eq!(findings(&home, &rules).len(), discovered.len());

    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    let skill = home.join(".agents/skills/reviewer");
    fs::create_dir_all(&skill).unwrap();
    let text = b"# unchanged\n";
    fs::write(skill.join("SKILL.md"), text).unwrap();
    fs::write(
        skill.join(".paseo-managed-files.json"),
        serde_json::to_vec(&json!({"version":1,"files":{"SKILL.md":sha256(text)}})).unwrap(),
    )
    .unwrap();
    let mut conflicting_rules = load_all_rules();
    let mut duplicate_owner = rule(&conflicting_rules, "paseo").clone();
    duplicate_owner.product.id = "other-owner".to_owned();
    duplicate_owner.product.name = "Other owner".to_owned();
    conflicting_rules.push(duplicate_owner);
    let conflict_findings = findings(&home, &conflicting_rules);
    assert_eq!(conflict_findings.len(), 2);
    assert!(
        conflict_findings
            .iter()
            .all(|finding| finding.attribution == Attribution::Conflicting)
    );
    let plan = cleanup::prepare(&home, &conflicting_rules, &conflict_findings).unwrap();
    assert!(plan.changes.is_empty());
    assert_eq!(
        plan.skipped.len(),
        1,
        "same physical conflict is deduplicated"
    );
    assert!(skill.join("SKILL.md").exists());
}

#[test]
fn prepare_rejects_duplicate_key_json_without_writing() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &home,
        ".claude/settings.json",
        br#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"muxy-notification-hook"}]}]},"unrelated":1,"unrelated":2}"#,
    );
    let rules = load_all_rules();
    let selected = findings(&home, &rules);
    assert_eq!(selected.len(), 1, "scanner still finds the action");
    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    assert!(plan.changes.is_empty());
    assert_eq!(plan.skipped.len(), 1);
    assert!(plan.skipped[0].reason.contains("ambiguous"));
    assert!(!plan.backup_root.exists());

    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &home,
        ".claude/settings.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"muxy-notification-hook"}]}]}}"#,
    );
    let rules = load_all_rules();
    let selected = findings(&home, &rules);
    let jsonc = "// user comment\n{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"muxy-notification-hook\"}]}]}}";
    write(&home, ".claude/settings.json", jsonc);
    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    assert!(plan.changes.is_empty());
    assert_eq!(plan.skipped.len(), 1);
    assert_eq!(
        fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
        jsonc
    );
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.backup_path, None);
    assert_eq!(
        fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
        jsonc
    );
}

#[test]
fn prepare_rejects_missing_and_non_directory_homes() {
    let temporary = tempdir().unwrap();
    let missing = temporary.path().join("missing-home");
    let regular_file = temporary.path().join("home-file");
    fs::write(&regular_file, b"not a home").unwrap();
    let rules = load_all_rules();
    assert!(matches!(
        cleanup::prepare(&missing, &rules, &[]),
        Err(CleanupError::HomeUnavailable)
    ));
    assert!(matches!(
        cleanup::prepare(&regular_file, &rules, &[]),
        Err(CleanupError::HomeUnavailable)
    ));
}

#[test]
fn execute_preflight_checks_all_files_before_backup_or_any_removal() {
    let (_temporary, home, rules, discovered) = manifest_home();
    let plan = cleanup::prepare(&home, &rules, &discovered).unwrap();
    let backup = plan.backup_root.clone();
    write(
        &home,
        ".agents/skills/reviewer/references/guide.md",
        b"changed after preview\n",
    );
    let error = cleanup::execute(plan).unwrap_err();
    assert!(matches!(error, CleanupError::PreflightChanged));
    assert!(!backup.exists());
    let skill = home.join(".agents/skills/reviewer");
    assert!(skill.join("SKILL.md").exists());
    assert!(skill.join(".paseo-managed-files.json").exists());
}

#[test]
fn execute_preflight_rejects_changed_hook_evidence_and_symlink() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &home,
        ".claude/settings.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"muxy-notification-hook private-hook-command"}]}]}}"#,
    );
    let rules = load_all_rules();
    let selected = findings(&home, &rules);
    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    let backup = plan.backup_root.clone();
    write(
        &home,
        ".claude/settings.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"user command after preview"}]}]}}"#,
    );
    assert!(matches!(
        cleanup::execute(plan),
        Err(CleanupError::PreflightChanged)
    ));
    assert!(!backup.exists());

    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    let outside = temporary.path().join("outside.txt");
    fs::create_dir_all(home.join(".agents/skills/custom")).unwrap();
    fs::write(&outside, b"external data\n").unwrap();
    let marked = format!(
        "<!-- superset-managed-skill v1 -->\n{secret}",
        secret = "private-skill-content"
    );
    write(&home, ".agents/skills/custom/SKILL.md", &marked);
    let rules = load_all_rules();
    let selected = findings(&home, &rules);
    assert_eq!(selected.len(), 1);
    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    let backup = plan.backup_root.clone();
    fs::remove_file(home.join(".agents/skills/custom/SKILL.md")).unwrap();
    std::os::unix::fs::symlink(&outside, home.join(".agents/skills/custom/SKILL.md")).unwrap();
    assert!(matches!(
        cleanup::execute(plan),
        Err(CleanupError::PreflightChanged)
    ));
    assert!(!backup.exists());
    assert_eq!(fs::read(&outside).unwrap(), b"external data\n");
}

#[test]
fn backup_failure_keeps_every_original_and_modes_are_preserved_on_success() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &home,
        ".claude/settings.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"muxy-notification-hook"},{"type":"command","command":"user"}]}]}}"#,
    );
    let config_path = home.join(".claude/settings.json");
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o640)).unwrap();
    let rules = load_all_rules();
    let selected = findings(&home, &rules);
    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 1);
    assert_eq!(
        fs::metadata(&config_path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let backup_path = result.backup_path.unwrap();
    assert_eq!(
        fs::metadata(&backup_path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let backup_file = backup_path.join("files/.claude/settings.json");
    assert_eq!(
        fs::metadata(&backup_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        fs::read_to_string(&backup_file)
            .unwrap()
            .contains("muxy-notification-hook")
    );
    let mapping = fs::read_to_string(backup_path.join("backup-map.json")).unwrap();
    assert!(mapping.contains("source_path_bytes_hex"));
    assert!(!mapping.contains("muxy-notification-hook"));
    let mapping: Value = serde_json::from_str(&mapping).unwrap();
    assert_eq!(mapping["files"][0]["original_mode"], 0o640);
    assert_eq!(
        mapping["files"][0]["sha256"],
        sha256(&fs::read(&backup_file).unwrap())
    );

    let receipt_path = backup_path.join("execution-receipt.json");
    assert_eq!(
        fs::metadata(&receipt_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let receipt = fs::read_to_string(receipt_path).unwrap();
    assert!(!receipt.contains("muxy-notification-hook"));
    let receipt: Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt["status"], "complete");
    assert_eq!(receipt["operations"][0]["role"], "primary");
    assert_eq!(receipt["operations"][0]["status"], "succeeded");

    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    let outside = temporary.path().join("outside");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&outside).unwrap();
    write(
        &home,
        ".claude/settings.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"muxy-notification-hook"}]}]}}"#,
    );
    let rules = load_all_rules();
    let selected = findings(&home, &rules);
    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    std::os::unix::fs::symlink(&outside, home.join(".local")).unwrap();
    assert!(matches!(
        cleanup::execute(plan),
        Err(CleanupError::Backup(_))
    ));
    assert!(home.join(".claude/settings.json").exists());
    let output = fs::read_to_string(home.join(".claude/settings.json")).unwrap();
    assert!(output.contains("muxy-notification-hook"));
}

#[test]
fn selected_mcp_unknown_or_modified_never_produces_a_change() {
    let (_temporary, home, rules, discovered) = mcp_home();
    let unknown = discovered
        .iter()
        .find(|finding| finding.locator == "/mcpServers/unverifiable")
        .unwrap()
        .clone();
    let modified = discovered
        .iter()
        .find(|finding| finding.locator == "/mcpServers/keep")
        .unwrap()
        .clone();
    assert_eq!(modified.integrity, Integrity::Unchanged);
    let config_path = home.join(".claude.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["mcpServers"]["keep"]["command"] = json!("modified-value");
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let current = findings(&home, &rules);
    let modified = current
        .iter()
        .find(|finding| finding.locator == "/mcpServers/keep")
        .unwrap()
        .clone();
    assert_eq!(modified.integrity, Integrity::Modified);
    let plan = cleanup::prepare(&home, &rules, &[unknown, modified]).unwrap();
    assert!(plan.changes.is_empty());
    assert_eq!(plan.skipped.len(), 1, "same config path is deduplicated");
    assert!(home.join(".claude.json").exists());
}

#[test]
fn cst_edits_preserve_unrelated_json_and_remove_multiple_original_array_positions() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let config = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"muxy-notification-hook one"},{"type":"command","command":"muxy-notification-hook two"},{"type":"command","command":"user third"}]}]},"user":  [ 1, 2, 3 ]}"#;
    write(&home, ".claude/settings.json", config);
    let rules = load_all_rules();
    let selected = findings(&home, &rules);
    assert_eq!(selected.len(), 2);
    let plan = cleanup::prepare(&home, &rules, &selected).unwrap();
    let result = cleanup::execute(plan).unwrap();
    assert_eq!(result.removed_items, 2);
    let output = fs::read_to_string(home.join(".claude/settings.json")).unwrap();
    assert!(output.contains("\"user\":  [ 1, 2, 3 ]"));
    assert!(output.contains("user third"));
    assert!(!output.contains("muxy-notification-hook"));
    serde_json::from_str::<Value>(&output).unwrap();
}
