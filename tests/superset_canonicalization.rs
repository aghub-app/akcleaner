use std::{fs, path::PathBuf};

use akcleaner::{discovery, rules::load_rules};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

#[test]
fn node_upstream_golden_defines_supported_and_unknown_integrity_cases() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/superset-canonicalization.json")).unwrap();
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let rules = load_rules(&repository.join("rules"))
        .unwrap()
        .into_iter()
        .filter(|ruleset| ruleset.product.id == "superset")
        .collect::<Vec<_>>();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);

    for case in cases {
        let canonical = case["upstream_canonical_json"].as_str().unwrap();
        assert_eq!(sha256(canonical.as_bytes()), case["upstream_sha256"]);

        let temporary = tempdir().unwrap();
        let home = temporary.path();
        let config_path = home.join(".claude.json");
        fs::write(
            &config_path,
            serde_json::to_vec(&json!({"mcpServers":{"demo":case["value"]}})).unwrap(),
        )
        .unwrap();
        let mut entry = Map::new();
        entry.insert("demo".to_owned(), case["upstream_sha256"].clone());
        let mut files = Map::new();
        files.insert(
            config_path.to_string_lossy().into_owned(),
            Value::Object(entry),
        );
        let ledger_path = home.join(".superset/plugins/mcp-ledger.json");
        fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
        fs::write(
            &ledger_path,
            serde_json::to_vec(&json!({"version":1,"files":files})).unwrap(),
        )
        .unwrap();

        let report = discovery::scan(home, &rules);
        assert_eq!(report.findings.len(), 1, "{}", case["id"]);
        let finding = &report.findings[0];
        assert_eq!(finding.product_id, "superset", "{}", case["id"]);
        assert_eq!(finding.attribution, discovery::Attribution::Confirmed);
        if case["supported"] == true {
            assert_eq!(finding.integrity, discovery::Integrity::Unchanged);
            assert!(report.diagnostics.is_empty());
        } else {
            assert_eq!(
                finding.integrity,
                discovery::Integrity::Unknown,
                "{}",
                case["id"]
            );
            assert_eq!(report.diagnostics.len(), 1, "{}", case["id"]);
            assert_eq!(
                report.diagnostics[0].code, "ledger_canonicalization_unsupported",
                "{}",
                case["id"]
            );
        }
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(
            !serialized.contains(canonical),
            "canonical config data leaked for {}",
            case["id"]
        );
    }
}

fn sha256(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
