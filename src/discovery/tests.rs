use std::{fs, path::Path};

use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{
    hosts::{HostArtifact, HostInventory, SkillDirectory},
    rules::{Evidence, Product, Rule, RuleSet, Verified},
};

use super::{
    Attribution, Integrity, ScanReport, scan::canonical_hash_for_test, scan_with_inventory,
};

fn ruleset(product_id: &str, rule_id: &str, surface: &str, evidence: Evidence) -> RuleSet {
    RuleSet {
        schema_version: 1,
        product: Product {
            id: product_id.to_owned(),
            name: product_id.to_owned(),
        },
        verified: Verified {
            repository: "https://example.invalid/repo".to_owned(),
            commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        },
        rules: vec![Rule {
            id: rule_id.to_owned(),
            surfaces: vec![surface.to_owned()],
            evidence,
        }],
    }
}

fn skill_artifact(surface: &str, skill_dir: &Path, content: &str) -> HostArtifact {
    HostArtifact {
        surface: surface.to_owned(),
        path: skill_dir.join("SKILL.md"),
        locator: String::new(),
        value: json!({
            "skill_dir": skill_dir.to_string_lossy(),
            "content": content
        }),
    }
}

fn inventory(artifacts: Vec<HostArtifact>) -> HostInventory {
    let skill_directories = artifacts
        .iter()
        .filter_map(|artifact| {
            artifact.value["skill_dir"]
                .as_str()
                .map(|path| SkillDirectory {
                    surface: artifact.surface.clone(),
                    path: Path::new(path).to_owned(),
                })
        })
        .collect();
    HostInventory {
        artifacts,
        skill_directories,
        diagnostics: Vec::new(),
    }
}

fn report_json(report: &ScanReport) -> String {
    serde_json::to_string(report).unwrap()
}

#[test]
fn command_marker_is_candidate_unknown_and_does_not_expose_command() {
    let temporary = tempdir().unwrap();
    let rules = ruleset(
        "muxy",
        "notification-hooks",
        "claude.hooks.user",
        Evidence::CommandMarker {
            marker: "muxy-hook".to_owned(),
        },
    );
    let sensitive_command = "runner --token=very-secret muxy-hook";
    let artifacts = vec![HostArtifact {
        surface: "claude.hooks.user".to_owned(),
        path: temporary.path().join(".claude/settings.json"),
        locator: "/hooks/Stop/0/hooks/0".to_owned(),
        value: json!({ "type":"command", "command":sensitive_command }),
    }];
    let report = scan_with_inventory(temporary.path(), &[rules], inventory(artifacts));
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].attribution, Attribution::Candidate);
    assert_eq!(report.findings[0].integrity, Integrity::Unknown);
    let serialized = report_json(&report);
    assert!(!serialized.contains("very-secret"));
    assert!(!serialized.contains(sensitive_command));
}

#[test]
fn skill_marker_confirms_only_the_skill_document() {
    let temporary = tempdir().unwrap();
    let root = temporary.path().join(".agents/skills");
    let skill_dir = root.join("superset");
    fs::create_dir_all(&skill_dir).unwrap();
    let skill_path = skill_dir.join("SKILL.md");
    fs::write(&skill_path, "<!-- superset-managed-skill v1 -->").unwrap();
    fs::write(skill_dir.join("user-notes.txt"), "user owned").unwrap();
    let rules = ruleset(
        "superset",
        "managed-skill-marker",
        "shared.skills.user",
        Evidence::SkillMarker {
            marker: "superset-managed-skill v1".to_owned(),
        },
    );
    let report = scan_with_inventory(
        temporary.path(),
        &[rules],
        inventory(vec![skill_artifact(
            "shared.skills.user",
            &skill_dir,
            "<!-- superset-managed-skill v1 -->",
        )]),
    );
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].path, skill_path);
    assert_eq!(report.findings[0].attribution, Attribution::Confirmed);
    assert_eq!(report.findings[0].integrity, Integrity::Unknown);
}

#[test]
fn manifest_checks_listed_files_individually_and_ignores_unlisted_files() {
    let temporary = tempdir().unwrap();
    let root = temporary.path().join(".agents/skills");
    let skill_dir = root.join("paseo");
    fs::create_dir_all(&skill_dir).unwrap();
    let original_skill = b"skill before edit";
    fs::write(skill_dir.join("SKILL.md"), original_skill).unwrap();
    fs::write(skill_dir.join("helper.js"), b"changed helper").unwrap();
    fs::write(skill_dir.join("user-notes.txt"), b"not in manifest").unwrap();
    let manifest = json!({
        "version": 1,
        "files": {
            "SKILL.md": sha256(original_skill),
            "helper.js": sha256(b"original helper"),
            "missing.md": sha256(b"missing")
        }
    });
    fs::write(
        skill_dir.join(".paseo-managed-files.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let rules = ruleset(
        "paseo",
        "managed-skill-files",
        "shared.skills.user",
        Evidence::SkillManifest {
            filename: ".paseo-managed-files.json".to_owned(),
        },
    );
    let report = scan_with_inventory(
        temporary.path(),
        &[rules],
        inventory(vec![skill_artifact("shared.skills.user", &skill_dir, "")]),
    );
    assert_eq!(report.findings.len(), 2);
    assert!(report.findings.iter().any(|finding| {
        finding.path.ends_with("paseo/SKILL.md") && finding.integrity == Integrity::Unchanged
    }));
    assert!(report.findings.iter().any(|finding| {
        finding.path.ends_with("paseo/helper.js") && finding.integrity == Integrity::Modified
    }));
    assert!(
        !report
            .findings
            .iter()
            .any(|finding| finding.path.ends_with("user-notes.txt"))
    );
    assert!(report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "manifest_file_missing" && diagnostic.path.ends_with("paseo/missing.md")
    }));
}

#[test]
fn manifest_path_escape_invalid_hash_and_symlink_are_diagnosed_without_following() {
    let temporary = tempdir().unwrap();
    let root = temporary.path().join(".agents/skills");
    let skill_dir = root.join("paseo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), b"skill").unwrap();
    fs::write(temporary.path().join("outside.txt"), b"outside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        temporary.path().join("outside.txt"),
        skill_dir.join("link.txt"),
    )
    .unwrap();
    let mut files = serde_json::Map::new();
    files.insert("../outside.txt".to_owned(), json!(sha256(b"outside")));
    files.insert("SKILL.md".to_owned(), json!("not-a-hash"));
    #[cfg(unix)]
    files.insert("link.txt".to_owned(), json!(sha256(b"outside")));
    fs::write(
        skill_dir.join("managed.json"),
        serde_json::to_vec(&json!({"version":1,"files":files})).unwrap(),
    )
    .unwrap();
    let rules = ruleset(
        "paseo",
        "managed-skill-files",
        "shared.skills.user",
        Evidence::SkillManifest {
            filename: "managed.json".to_owned(),
        },
    );
    let report = scan_with_inventory(
        temporary.path(),
        &[rules],
        inventory(vec![skill_artifact("shared.skills.user", &skill_dir, "")]),
    );
    assert!(report.findings.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "invalid_manifest_path")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "manifest_hash_invalid")
    );
    #[cfg(unix)]
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "manifest_file_symlink_skipped")
    );
}

#[test]
fn canonical_hash_matches_node_upstream_golden_and_preserves_array_order() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/superset-canonicalization.json"
    ))
    .unwrap();
    let standard = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "standard-command-and-env")
        .unwrap();
    assert_eq!(
        canonical_hash_for_test(&standard["value"]),
        standard["upstream_sha256"]
    );

    let mut reordered_array = standard["value"].clone();
    reordered_array["args"] = json!(["repo", "--project"]);
    assert_ne!(
        canonical_hash_for_test(&standard["value"]),
        canonical_hash_for_test(&reordered_array)
    );
}

#[test]
fn ledger_only_confirms_current_home_claude_config_and_marks_modified_values() {
    let temporary = tempdir().unwrap();
    let home = temporary.path();
    let ledger_path = home.join(".superset/plugins/mcp-ledger.json");
    fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
    let config_path = home.join(".claude.json");
    fs::write(&config_path, b"synthetic host file").unwrap();
    let mcp_value = json!({
        "command":"demo",
        "type":"stdio",
        "args":["--project", "repo"],
        "env":{"PATH":"/usr/bin"}
    });
    let expected_hash = "de57dd269028cee1c38e708fb8bf9b9e7c49f80227b9f7a6f436e46b6054cb0d";
    fs::write(
        &ledger_path,
        serde_json::to_vec(&json!({
            "version":1,
            "files":{
                config_path.to_string_lossy().to_string(): {"mcp/name~one": expected_hash},
                "/other/home/.claude.json": {"mcp/name~one": expected_hash}
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let rules = ruleset(
        "superset",
        "managed-mcp-ledger",
        "claude.mcp.user",
        Evidence::McpLedger {
            path: ".superset/plugins/mcp-ledger.json".to_owned(),
        },
    );
    let artifact = HostArtifact {
        surface: "claude.mcp.user".to_owned(),
        path: config_path,
        locator: "/mcpServers/mcp~1name~0one".to_owned(),
        value: mcp_value,
    };
    let report = scan_with_inventory(home, &[rules], inventory(vec![artifact]));
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].attribution, Attribution::Confirmed);
    assert_eq!(report.findings[0].integrity, Integrity::Unchanged);
    let serialized = report_json(&report);
    assert!(!serialized.contains("/usr/bin"));
}

#[cfg(unix)]
#[test]
fn manifest_root_symlink_component_is_rejected_before_reading_manifest() {
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    let outside = temporary.path().join("outside");
    let skill_dir = home.join(".agents/skills/example");
    let real_skill_dir = outside.join("skills/example");
    fs::create_dir_all(&real_skill_dir).unwrap();
    fs::write(real_skill_dir.join("SKILL.md"), b"synthetic").unwrap();
    fs::write(
        real_skill_dir.join("managed.json"),
        serde_json::to_vec(&json!({"version":1,"files":{"SKILL.md":sha256(b"synthetic")}}))
            .unwrap(),
    )
    .unwrap();
    fs::create_dir_all(home.join(".agents")).unwrap();
    std::os::unix::fs::symlink(outside.join("skills"), home.join(".agents/skills")).unwrap();

    let rules = ruleset(
        "paseo",
        "managed-skill-files",
        "shared.skills.user",
        Evidence::SkillManifest {
            filename: "managed.json".to_owned(),
        },
    );
    let report = scan_with_inventory(
        &home,
        &[rules],
        inventory(vec![skill_artifact("shared.skills.user", &skill_dir, "")]),
    );
    assert!(report.findings.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "manifest_root_symlink_skipped");
}

#[test]
fn manifest_directory_inventory_is_limited_to_known_immediate_skill_roots() {
    let temporary = tempdir().unwrap();
    let home = temporary.path();
    let nested = home.join(".agents/skills/group/nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("managed.json"), br#"{"version":1,"files":{}}"#).unwrap();
    let rules = ruleset(
        "paseo",
        "managed-skill-files",
        "shared.skills.user",
        Evidence::SkillManifest {
            filename: "managed.json".to_owned(),
        },
    );
    let inventory = HostInventory {
        artifacts: Vec::new(),
        skill_directories: vec![SkillDirectory {
            surface: "shared.skills.user".to_owned(),
            path: nested,
        }],
        diagnostics: Vec::new(),
    };

    let report = scan_with_inventory(home, &[rules], inventory);
    assert!(report.findings.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "skill_inventory_invalid");
}

#[test]
fn public_scan_diagnoses_missing_and_non_directory_homes() {
    let temporary = tempdir().unwrap();
    let rules = vec![ruleset(
        "muxy",
        "notification-hooks",
        "claude.hooks.user",
        Evidence::CommandMarker {
            marker: "marker".to_owned(),
        },
    )];
    let missing = super::scan::scan(&temporary.path().join("missing"), &rules);
    assert!(missing.findings.is_empty());
    assert_eq!(missing.diagnostics[0].code, "home_missing");

    let file = temporary.path().join("not-a-home");
    fs::write(&file, b"synthetic").unwrap();
    let not_directory = super::scan::scan(&file, &rules);
    assert!(not_directory.findings.is_empty());
    assert_eq!(not_directory.diagnostics[0].code, "home_not_directory");
}

#[test]
fn ledger_rule_cannot_escape_home_and_ledger_symlink_is_skipped() {
    let temporary = tempdir().unwrap();
    let home = temporary.path();
    let outside_ledger = temporary.path().join("outside-ledger.json");
    fs::write(&outside_ledger, b"{}").unwrap();
    let link_parent = home.join(".superset/plugins");
    fs::create_dir_all(&link_parent).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_ledger, link_parent.join("ledger.json")).unwrap();
    fs::write(home.join(".claude.json"), b"{}").unwrap();
    let rules = ruleset(
        "superset",
        "managed-mcp-ledger",
        "claude.mcp.user",
        Evidence::McpLedger {
            path: ".superset/plugins/ledger.json".to_owned(),
        },
    );
    let artifact = HostArtifact {
        surface: "claude.mcp.user".to_owned(),
        path: home.join(".claude.json"),
        locator: "/mcpServers/name".to_owned(),
        value: json!({"command":"private"}),
    };
    let report = scan_with_inventory(home, &[rules], inventory(vec![artifact]));
    #[cfg(unix)]
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "ledger_symlink_skipped")
    );

    let invalid_rule = ruleset(
        "superset",
        "managed-mcp-ledger",
        "claude.mcp.user",
        Evidence::McpLedger {
            path: "../outside-ledger.json".to_owned(),
        },
    );
    // Rule loading rejects this path; direct construction is not a trusted input.
    let invalid_artifact = HostArtifact {
        surface: "claude.mcp.user".to_owned(),
        path: home.join(".claude.json"),
        locator: "/mcpServers/name".to_owned(),
        value: json!({"command":"private"}),
    };
    let invalid = scan_with_inventory(home, &[invalid_rule], inventory(vec![invalid_artifact]));
    assert!(invalid.findings.is_empty());
    assert!(
        invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "ledger_path_invalid")
    );
}

#[test]
fn repeated_scans_are_deterministic() {
    let temporary = tempdir().unwrap();
    let rules = vec![
        ruleset(
            "zeta",
            "z-rule",
            "claude.hooks.user",
            Evidence::CommandMarker {
                marker: "tag".to_owned(),
            },
        ),
        ruleset(
            "alpha",
            "a-rule",
            "claude.hooks.user",
            Evidence::CommandMarker {
                marker: "tag".to_owned(),
            },
        ),
    ];
    let artifacts = vec![
        HostArtifact {
            surface: "claude.hooks.user".to_owned(),
            path: temporary.path().join("settings.json"),
            locator: "/hooks/a".to_owned(),
            value: json!({"command":"tag one"}),
        },
        HostArtifact {
            surface: "claude.hooks.user".to_owned(),
            path: temporary.path().join("settings.json"),
            locator: "/hooks/b".to_owned(),
            value: json!({"command":"tag two"}),
        },
    ];
    let first = scan_with_inventory(temporary.path(), &rules, inventory(artifacts));
    let second = scan_with_inventory(
        temporary.path(),
        &rules,
        inventory(vec![
            HostArtifact {
                surface: "claude.hooks.user".to_owned(),
                path: temporary.path().join("settings.json"),
                locator: "/hooks/b".to_owned(),
                value: json!({"command":"tag two"}),
            },
            HostArtifact {
                surface: "claude.hooks.user".to_owned(),
                path: temporary.path().join("settings.json"),
                locator: "/hooks/a".to_owned(),
                value: json!({"command":"tag one"}),
            },
        ]),
    );
    assert_eq!(report_json(&first), report_json(&second));
}

fn sha256(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let bytes = Sha256::digest(value);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
