use std::fs;

use serde_json::{Value, json};
use tempfile::tempdir;

use super::{RuleLoadError, load_rules};

fn valid_ruleset(product_id: &str) -> Value {
    json!({
        "schema_version": 1,
        "product": { "id": product_id, "name": "Example" },
        "verified": {
            "repository": "https://example.invalid/repo",
            "commit": "0123456789abcdef0123456789abcdef01234567"
        },
        "rules": [{
            "id": "hook-marker",
            "surfaces": ["claude.hooks.user"],
            "evidence": { "type": "command_marker", "marker": "example-hook" }
        }]
    })
}

fn write_rules(directory: &std::path::Path, child: &str, value: &Value) {
    let product_directory = directory.join(child);
    fs::create_dir_all(&product_directory).unwrap();
    fs::write(
        product_directory.join("rules.json"),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
}

fn load_one(value: Value) -> Result<Vec<super::RuleSet>, RuleLoadError> {
    let temporary = tempdir().unwrap();
    write_rules(temporary.path(), "product", &value);
    load_rules(temporary.path())
}

#[test]
fn rejects_unknown_fields_and_evidence_types() {
    let mut unknown_field = valid_ruleset("example");
    unknown_field["unexpected"] = json!(true);
    assert!(matches!(
        load_one(unknown_field),
        Err(RuleLoadError::Parse { .. })
    ));

    let mut unknown_evidence = valid_ruleset("example");
    unknown_evidence["rules"][0]["evidence"] = json!({"type":"shell_exec","command":"x"});
    assert!(matches!(
        load_one(unknown_evidence),
        Err(RuleLoadError::Parse { .. })
    ));

    let mut extra_evidence_field = valid_ruleset("example");
    extra_evidence_field["rules"][0]["evidence"]["token"] = json!("must not be accepted");
    assert!(matches!(
        load_one(extra_evidence_field),
        Err(RuleLoadError::Parse { .. })
    ));
}

#[test]
fn rejects_unsupported_versions_ids_empty_markers_and_incompatible_surfaces() {
    let mut wrong_version = valid_ruleset("example");
    wrong_version["schema_version"] = json!(2);
    assert!(matches!(
        load_one(wrong_version),
        Err(RuleLoadError::InvalidRules { .. })
    ));

    let mut illegal_id = valid_ruleset("Bad_ID");
    assert!(matches!(
        load_one(illegal_id.take()),
        Err(RuleLoadError::InvalidRules { .. })
    ));

    let mut empty_marker = valid_ruleset("example");
    empty_marker["rules"][0]["evidence"]["marker"] = json!("  ");
    assert!(matches!(
        load_one(empty_marker),
        Err(RuleLoadError::InvalidRules { .. })
    ));

    let mut incompatible = valid_ruleset("example");
    incompatible["rules"][0]["surfaces"] = json!(["claude.mcp.user"]);
    assert!(matches!(
        load_one(incompatible),
        Err(RuleLoadError::InvalidRules { .. })
    ));
}

#[test]
fn rejects_duplicate_product_and_rule_ids() {
    let temporary = tempdir().unwrap();
    write_rules(temporary.path(), "a", &valid_ruleset("same"));
    write_rules(temporary.path(), "b", &valid_ruleset("same"));
    assert!(matches!(
        load_rules(temporary.path()),
        Err(RuleLoadError::DuplicateProduct { .. })
    ));

    let mut duplicate_rule = valid_ruleset("example");
    duplicate_rule["rules"].as_array_mut().unwrap().push(json!({
        "id": "hook-marker",
        "surfaces": ["codex.hooks.user"],
        "evidence": { "type": "command_marker", "marker": "other" }
    }));
    assert!(matches!(
        load_one(duplicate_rule),
        Err(RuleLoadError::InvalidRules { .. })
    ));
}

#[test]
fn rejects_manifest_and_ledger_paths_with_escape_or_absolute_forms() {
    for invalid in [
        "../outside.json",
        "nested/../../outside.json",
        "/tmp/outside.json",
        "C:\\outside.json",
        "\\\\server\\share\\ledger.json",
    ] {
        let mut manifest = valid_ruleset("example");
        manifest["rules"][0]["surfaces"] = json!(["shared.skills.user"]);
        manifest["rules"][0]["evidence"] = json!({
            "type":"skill_manifest",
            "filename": invalid
        });
        assert!(
            matches!(load_one(manifest), Err(RuleLoadError::InvalidRules { .. })),
            "accepted unsafe manifest path {invalid:?}"
        );
    }

    let mut ledger = valid_ruleset("example");
    ledger["rules"][0]["surfaces"] = json!(["claude.mcp.user"]);
    ledger["rules"][0]["evidence"] = json!({
        "type":"mcp_ledger",
        "path":"../../other-home/ledger.json"
    });
    assert!(matches!(
        load_one(ledger),
        Err(RuleLoadError::InvalidRules { .. })
    ));
}

#[test]
fn empty_rules_directory_is_not_a_successful_zero_rule_set() {
    let temporary = tempdir().unwrap();
    assert!(matches!(
        load_rules(temporary.path()),
        Err(RuleLoadError::NoRules(_))
    ));
}

#[test]
fn skips_fixture_directories_without_rules_json_but_loads_products() {
    let temporary = tempdir().unwrap();
    write_rules(temporary.path(), "product", &valid_ruleset("example"));
    fs::create_dir_all(temporary.path().join("fixtures/case/before/home")).unwrap();
    let rules = load_rules(temporary.path()).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].product.id, "example");
}

#[test]
fn present_invalid_or_symlink_rules_json_is_an_error() {
    let malformed = tempdir().unwrap();
    let product = malformed.path().join("product");
    fs::create_dir_all(&product).unwrap();
    fs::write(product.join("rules.json"), b"{").unwrap();
    assert!(matches!(
        load_rules(malformed.path()),
        Err(RuleLoadError::Parse { .. })
    ));

    #[cfg(unix)]
    {
        let symlinked = tempdir().unwrap();
        let product = symlinked.path().join("product");
        fs::create_dir_all(&product).unwrap();
        let target = symlinked.path().join("target.json");
        fs::write(
            &target,
            serde_json::to_vec(&valid_ruleset("example")).unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(target, product.join("rules.json")).unwrap();
        assert!(matches!(
            load_rules(symlinked.path()),
            Err(RuleLoadError::Symlink(_))
        ));
    }
}

#[test]
fn surface_set_is_exactly_the_six_shared_protocol_surfaces() {
    assert_eq!(
        super::Surface::ALL,
        [
            "claude.hooks.user",
            "codex.hooks.user",
            "claude.mcp.user",
            "shared.skills.user",
            "claude.skills.user",
            "codex.skills.user",
        ]
    );
    assert!(
        super::Surface::ALL
            .iter()
            .all(|surface| super::Surface::parse(surface).is_some())
    );
    assert!(super::Surface::parse("claude.mcp.project").is_none());
}
