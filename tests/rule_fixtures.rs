use std::{
    fs,
    path::{Path, PathBuf},
};

use akcleaner::{discovery, hosts, rules::load_rules};
use serde_json::Value;
use tempfile::tempdir;

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FindingView {
    product_id: String,
    rule_id: String,
    surface: String,
    path: String,
    locator: String,
    attribution: String,
    integrity: String,
}

fn assert_fixture(case: &str) {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = repository.join("rules").join(case);
    let expected: Value = serde_json::from_slice(
        &fs::read(fixture.join("expected.json")).expect("fixture expectation exists"),
    )
    .expect("fixture expectation is valid JSON");
    let temporary = tempdir().unwrap();
    let home = temporary.path().join("home");
    copy_tree(&fixture.join("before/home"), &home);

    if case == "superset/fixtures/mcp-ledger-mismatch" {
        let ledger = home.join(".superset/plugins/mcp-ledger.json");
        let contents = fs::read_to_string(&ledger).unwrap();
        assert!(contents.contains("/synthetic/home"));
        fs::write(
            &ledger,
            contents.replace("/synthetic/home", &home.to_string_lossy()),
        )
        .unwrap();
    }

    let rules = load_rules(&repository.join("rules")).expect("A rules load with fixture dirs");
    let report = discovery::scan(&home, &rules);
    let mut actual = report
        .findings
        .iter()
        .map(|finding| FindingView {
            product_id: finding.product_id.clone(),
            rule_id: finding.rule_id.clone(),
            surface: finding.surface.clone(),
            path: normalize_path(&home, &finding.path),
            locator: finding.locator.clone(),
            attribution: serde_json::to_value(&finding.attribution)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned(),
            integrity: serde_json::to_value(&finding.integrity)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned(),
        })
        .collect::<Vec<_>>();
    actual.sort();

    let mut expected_findings = expected["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| FindingView {
            product_id: string_field(finding, "product_id"),
            rule_id: string_field(finding, "rule_id"),
            surface: string_field(finding, "surface"),
            path: string_field(finding, "path"),
            locator: string_field(finding, "locator"),
            attribution: string_field(finding, "attribution"),
            integrity: string_field(finding, "integrity"),
        })
        .collect::<Vec<_>>();
    expected_findings.sort();
    assert_eq!(actual, expected_findings, "finding mismatch for {case}");

    for absent in expected["not_find"].as_array().unwrap() {
        let surface = string_field(absent, "surface");
        let path = string_field(absent, "path");
        let locator = absent.get("locator").and_then(Value::as_str);
        assert!(
            !report.findings.iter().any(|finding| {
                finding.surface == surface
                    && normalize_path(&home, &finding.path) == path
                    && locator.is_none_or(|locator| finding.locator == locator)
            }),
            "unexpected finding for negative case in {case}: {absent}"
        );
    }

    let mut actual_diagnostics = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    actual_diagnostics.sort();
    let mut expected_diagnostics = expected["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| string_field(diagnostic, "code"))
        .collect::<Vec<_>>();
    expected_diagnostics.sort();
    assert_eq!(
        actual_diagnostics, expected_diagnostics,
        "diagnostics for {case}"
    );

    if let Some(expected_artifacts) = expected.get("artifacts") {
        let surfaces = vec!["claude.hooks.user".to_owned()];
        let inventory = hosts::discover(&home, &surfaces);
        let mut actual_artifacts = inventory
            .artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.surface.clone(),
                    normalize_path(&home, &artifact.path),
                    artifact.locator.clone(),
                )
            })
            .collect::<Vec<_>>();
        actual_artifacts.sort();
        let mut expected_artifacts = expected_artifacts
            .as_array()
            .unwrap()
            .iter()
            .map(|artifact| {
                (
                    string_field(artifact, "surface"),
                    string_field(artifact, "path"),
                    string_field(artifact, "locator"),
                )
            })
            .collect::<Vec<_>>();
        expected_artifacts.sort();
        assert_eq!(
            actual_artifacts, expected_artifacts,
            "host artifacts for {case}"
        );
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path).unwrap();
        assert!(
            !metadata.file_type().is_symlink(),
            "fixture tree contains symlink"
        );
        if metadata.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(source_path, destination_path).unwrap();
        }
    }
}

fn normalize_path(home: &Path, path: &Path) -> String {
    let relative = path
        .strip_prefix(home)
        .expect("finding remains under fixture home");
    format!("HOME/{}", relative.to_string_lossy().replace('\\', "/"))
}

fn string_field(value: &Value, field: &str) -> String {
    value[field].as_str().unwrap().to_owned()
}

#[test]
fn hooks_mixed_fixture_matches_expected() {
    assert_fixture("fixtures/hooks-mixed");
}

#[test]
fn json_pointer_special_fixture_matches_expected() {
    assert_fixture("fixtures/json-pointer-special");
}

#[test]
fn skill_marker_fixture_matches_expected() {
    assert_fixture("superset/fixtures/skill-marker");
}

#[test]
fn mcp_ledger_mismatch_fixture_matches_expected() {
    assert_fixture("superset/fixtures/mcp-ledger-mismatch");
}

#[test]
fn paseo_manifest_unchanged_fixture_matches_expected() {
    assert_fixture("paseo/fixtures/manifest-unchanged");
}

#[test]
fn paseo_manifest_modified_fixture_matches_expected() {
    assert_fixture("paseo/fixtures/manifest-modified");
}

#[test]
fn paseo_manifest_traversal_fixture_matches_expected() {
    assert_fixture("paseo/fixtures/manifest-traversal");
}
