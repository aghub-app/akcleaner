use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{
    filesystem::{self, BackupFile},
    metadata,
    model::{
        ChangeAction, CleanupError, CleanupPlan, CleanupResult, MetadataUpdate, Operation,
        SkippedItem,
    },
    planner::{hex_lower, operation_relative, sha256, skip, snapshot_matches},
};

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptStatus {
    Planned,
    Started,
    Succeeded,
    Failed,
    NotRun,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptRunStatus {
    Planned,
    Running,
    Complete,
    Partial,
}

#[derive(Serialize)]
struct ReceiptOperation {
    path: String,
    role: &'static str,
    action: &'static str,
    item_count: usize,
    status: ReceiptStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_kind: Option<String>,
}

#[derive(Serialize)]
struct ExecutionReceipt {
    version: u8,
    status: ReceiptRunStatus,
    operations: Vec<ReceiptOperation>,
}

/// Execute an opaque plan. Every snapshot is checked before backup creation,
/// and all backups are complete before any primary or metadata operation runs.
pub(super) fn execute(plan: CleanupPlan) -> Result<CleanupResult, CleanupError> {
    execute_with(plan, apply_operation)
}

fn execute_with(
    plan: CleanupPlan,
    mut apply_primary: impl FnMut(&Path, &Operation) -> io::Result<()>,
) -> Result<CleanupResult, CleanupError> {
    if plan.backup_root != plan.home.join(&plan.backup_relative) {
        return Err(CleanupError::PreflightChanged);
    }
    if plan.operations.is_empty() {
        return Ok(CleanupResult {
            removed_items: 0,
            changed_files: 0,
            backup_path: None,
            failures: Vec::new(),
        });
    }

    for (relative, expected) in &plan.snapshots {
        let current = filesystem::read_file(&plan.home, relative, MAX_FILE_BYTES)
            .map_err(|_| CleanupError::PreflightChanged)?;
        if current.mode != expected.mode || sha256(&current.bytes) != expected.hash {
            return Err(CleanupError::PreflightChanged);
        }
    }

    let mut receipt = planned_receipt(&plan);
    filesystem::create_backup(&plan.home, &plan.backup_relative).map_err(CleanupError::Backup)?;
    persist_receipt(&plan, &receipt, true).map_err(CleanupError::Backup)?;
    let backup_files = plan
        .snapshots
        .iter()
        .map(|(relative, snapshot)| BackupFile {
            relative: relative.clone(),
            bytes: snapshot.bytes.clone(),
            original_mode: snapshot.mode,
            sha256: hex_lower(&sha256(&snapshot.bytes)),
        })
        .collect::<Vec<_>>();
    filesystem::write_backup_files(&plan.home, &plan.backup_relative, &backup_files)
        .map_err(CleanupError::Backup)?;

    let mut failures = Vec::new();
    let mut changed_files = 0;
    let mut removed_items = 0;
    let mut successful_removals = BTreeSet::<PathBuf>::new();
    let mut successful_replacements = BTreeSet::<PathBuf>::new();
    let mut blocked = false;

    receipt.status = ReceiptRunStatus::Running;
    if persist_receipt(&plan, &receipt, false).is_err() {
        failures.push(receipt_failure(&plan));
        blocked = true;
    }

    for (index, operation) in plan.operations.iter().enumerate() {
        if blocked {
            break;
        }
        receipt.operations[index].status = ReceiptStatus::Started;
        receipt.operations[index].error_kind = None;
        if persist_receipt(&plan, &receipt, false).is_err() {
            receipt.operations[index].status = ReceiptStatus::Failed;
            receipt.operations[index].error_kind = Some("receipt_write_failed".to_owned());
            failures.push(receipt_failure(&plan));
            blocked = true;
            break;
        }

        let relative = operation_relative(operation);
        let snapshot_current = plan
            .snapshots
            .get(relative)
            .is_some_and(|snapshot| snapshot_matches(&plan.home, relative, snapshot));
        let result = if snapshot_current {
            apply_primary(&plan.home, operation)
        } else {
            Err(io::Error::other("snapshot changed after backup"))
        };

        match result {
            Ok(()) => {
                receipt.operations[index].status = ReceiptStatus::Succeeded;
                changed_files += 1;
                match operation {
                    Operation::Remove {
                        relative,
                        item_count,
                        ..
                    } => {
                        removed_items += item_count;
                        successful_removals.insert(relative.clone());
                    }
                    Operation::Replace {
                        relative,
                        item_count,
                        ..
                    } => {
                        removed_items += item_count;
                        successful_replacements.insert(relative.clone());
                    }
                }
            }
            Err(error) => {
                receipt.operations[index].status = ReceiptStatus::Failed;
                receipt.operations[index].error_kind = Some(if snapshot_current {
                    format!("{:?}", error.kind())
                } else {
                    "snapshot_changed".to_owned()
                });
                let reason = if !snapshot_current {
                    "File changed after backup and was left untouched."
                } else {
                    "Could not apply the planned change; the backup remains available."
                };
                failures.push(skip(plan.home.join(relative), reason));
            }
        }
        if persist_receipt(&plan, &receipt, false).is_err() {
            failures.push(receipt_failure(&plan));
            blocked = true;
        }
    }

    let metadata_start = plan.operations.len();
    if !blocked {
        for (offset, update) in plan.metadata_updates.iter().enumerate() {
            let index = metadata_start + offset;
            receipt.operations[index].status = ReceiptStatus::Started;
            receipt.operations[index].error_kind = None;
            if persist_receipt(&plan, &receipt, false).is_err() {
                receipt.operations[index].status = ReceiptStatus::Failed;
                receipt.operations[index].error_kind = Some("receipt_write_failed".to_owned());
                failures.push(receipt_failure(&plan));
                break;
            }

            match metadata::execute_metadata_update(
                update,
                &plan,
                &successful_removals,
                &successful_replacements,
            ) {
                Ok(false) => receipt.operations[index].status = ReceiptStatus::NotRun,
                Ok(true) => {
                    receipt.operations[index].status = ReceiptStatus::Succeeded;
                    changed_files += 1;
                }
                Err(path) => {
                    receipt.operations[index].status = ReceiptStatus::Failed;
                    receipt.operations[index].error_kind =
                        Some("metadata_update_failed".to_owned());
                    failures.push(skip(
                        path,
                        "Could not update associated ownership metadata; the backup remains available.",
                    ));
                }
            }
            if persist_receipt(&plan, &receipt, false).is_err() {
                failures.push(receipt_failure(&plan));
                break;
            }
        }
    }

    receipt.status = if failures.is_empty() {
        ReceiptRunStatus::Complete
    } else {
        ReceiptRunStatus::Partial
    };
    if persist_receipt(&plan, &receipt, false).is_err() {
        failures.push(receipt_failure(&plan));
    }

    debug_assert!(removed_items <= plan.selected_item_count);
    Ok(CleanupResult {
        removed_items,
        changed_files,
        backup_path: Some(plan.home.join(plan.backup_relative)),
        failures,
    })
}

fn apply_operation(home: &Path, operation: &Operation) -> io::Result<()> {
    match operation {
        Operation::Remove { relative, .. } => filesystem::remove_file(home, relative),
        Operation::Replace {
            relative,
            bytes,
            mode,
            ..
        } => filesystem::replace_file(home, relative, bytes, *mode),
    }
}

fn planned_receipt(plan: &CleanupPlan) -> ExecutionReceipt {
    let mut operations = plan
        .operations
        .iter()
        .map(|operation| {
            let relative = operation_relative(operation);
            let (action, item_count) = match operation {
                Operation::Remove { item_count, .. } => ("remove_file", *item_count),
                Operation::Replace { item_count, .. } => ("edit_json", *item_count),
            };
            receipt_operation(&plan.home.join(relative), "primary", action, item_count)
        })
        .collect::<Vec<_>>();
    operations.extend(plan.metadata_updates.iter().map(|update| {
        let (relative, item_count) = match update {
            MetadataUpdate::Manifest {
                relative, selected, ..
            } => (relative, selected.len()),
            MetadataUpdate::Ledger {
                relative,
                server_names,
                ..
            } => (relative, server_names.len()),
        };
        let action = plan
            .changes
            .iter()
            .find(|change| change.path == plan.home.join(relative))
            .map(|change| match change.action {
                ChangeAction::RemoveFile => "remove_file",
                ChangeAction::EditJson => "edit_json",
            })
            .unwrap_or("edit_json");
        receipt_operation(&plan.home.join(relative), "metadata", action, item_count)
    }));
    ExecutionReceipt {
        version: 1,
        status: ReceiptRunStatus::Planned,
        operations,
    }
}

fn receipt_operation(
    path: &Path,
    role: &'static str,
    action: &'static str,
    item_count: usize,
) -> ReceiptOperation {
    ReceiptOperation {
        path: path.to_string_lossy().into_owned(),
        role,
        action,
        item_count,
        status: ReceiptStatus::Planned,
        error_kind: None,
    }
}

fn persist_receipt(plan: &CleanupPlan, receipt: &ExecutionReceipt, create: bool) -> io::Result<()> {
    let bytes =
        serde_json::to_vec_pretty(receipt).map_err(|error| io::Error::other(error.to_string()))?;
    filesystem::write_execution_receipt(&plan.home, &plan.backup_relative, &bytes, create)
}

fn receipt_failure(plan: &CleanupPlan) -> SkippedItem {
    let mut relative = plan.backup_relative.clone();
    relative.push("execution-receipt.json");
    skip(
        plan.home.join(relative),
        "Execution receipt could not be updated; later file changes were stopped.",
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::ErrorKind,
        path::{Path, PathBuf},
    };

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use crate::{
        discovery,
        rules::{RuleSet, load_rules},
        superset_canonicalization,
    };

    use super::{apply_operation, execute_with, operation_relative};
    use crate::cleanup::{model::Operation, planner};

    fn product_rules(id: &str) -> Vec<RuleSet> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rules");
        load_rules(&root)
            .unwrap()
            .into_iter()
            .filter(|ruleset| ruleset.product.id == id)
            .collect()
    }

    fn sha256(bytes: &[u8]) -> String {
        let digest = planner::sha256(bytes);
        planner::hex_lower(&digest)
    }

    fn manifest(home: &Path) -> (String, String) {
        let skill = home.join(".agents/skills/managed");
        fs::create_dir_all(&skill).unwrap();
        let first = b"first managed file";
        let second = b"second managed file";
        fs::write(skill.join("a.txt"), first).unwrap();
        fs::write(skill.join("b.txt"), second).unwrap();
        fs::write(
            skill.join(".paseo-managed-files.json"),
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "files": { "a.txt": sha256(first), "b.txt": sha256(second) }
            }))
            .unwrap(),
        )
        .unwrap();
        (sha256(first), sha256(second))
    }

    #[test]
    fn failed_manifest_file_operation_is_persisted_and_keeps_its_manifest_entry() {
        let temporary = tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let (first_hash, _) = manifest(&home);
        let rules = product_rules("paseo");
        let selected = discovery::scan(&home, &rules).findings;
        assert_eq!(selected.len(), 2);
        let failing_relative = PathBuf::from(".agents/skills/managed/a.txt");
        let plan = planner::prepare(&home, &rules, &selected).unwrap();

        let result = execute_with(plan, |home, operation| {
            if operation_relative(operation) == &failing_relative {
                Err(std::io::Error::new(ErrorKind::PermissionDenied, "injected"))
            } else {
                apply_operation(home, operation)
            }
        })
        .unwrap();

        assert_eq!(result.removed_items, 1);
        assert!(
            !result.failures.is_empty(),
            "caller must return a nonzero exit"
        );
        assert!(home.join(&failing_relative).exists());
        assert!(!home.join(".agents/skills/managed/b.txt").exists());
        let manifest: Value = serde_json::from_slice(
            &fs::read(home.join(".agents/skills/managed/.paseo-managed-files.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["files"]["a.txt"], first_hash);
        assert!(manifest["files"].get("b.txt").is_none());

        let backup = result.backup_path.unwrap();
        let receipt: Value =
            serde_json::from_slice(&fs::read(backup.join("execution-receipt.json")).unwrap())
                .unwrap();
        let failed = receipt["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|operation| {
                operation["path"] == home.join(&failing_relative).to_string_lossy().as_ref()
            })
            .unwrap();
        assert_eq!(failed["role"], "primary");
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["error_kind"], "PermissionDenied");
        let manifest_receipt = receipt["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|operation| operation["role"] == "metadata")
            .unwrap();
        assert_eq!(manifest_receipt["status"], "succeeded");
        let receipt_text = serde_json::to_string(&receipt).unwrap();
        assert!(!receipt_text.contains("first managed file"));
    }

    #[test]
    fn failed_mcp_config_operation_keeps_its_ledger_entry() {
        let temporary = tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let config_path = home.join(".claude.json");
        let server = json!({ "command": "private-command", "args": ["--mode", "safe"] });
        let hash = superset_canonicalization::canonical_server_sha256(&server).unwrap();
        fs::write(
            &config_path,
            serde_json::to_vec(&json!({ "mcpServers": { "demo": server } })).unwrap(),
        )
        .unwrap();
        let config_key = config_path.to_string_lossy().into_owned();
        let ledger_path = home.join(".superset/plugins/mcp-ledger.json");
        fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
        fs::write(
            &ledger_path,
            serde_json::to_vec(&json!({
                "version": 1,
                "files": { config_key.clone(): { "demo": hash } }
            }))
            .unwrap(),
        )
        .unwrap();

        let rules = product_rules("superset");
        let selected = discovery::scan(&home, &rules).findings;
        assert_eq!(selected.len(), 1);
        let plan = planner::prepare(&home, &rules, &selected).unwrap();
        let result = execute_with(plan, |home, operation| {
            if matches!(operation, Operation::Replace { relative, .. } if relative == Path::new(".claude.json")) {
                Err(std::io::Error::new(ErrorKind::PermissionDenied, "injected"))
            } else {
                apply_operation(home, operation)
            }
        })
        .unwrap();

        assert_eq!(result.removed_items, 0);
        assert!(
            !result.failures.is_empty(),
            "caller must return a nonzero exit"
        );
        assert!(config_path.exists());
        let ledger: Value = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        assert_eq!(ledger["files"][config_key]["demo"], hash);
        let backup = result.backup_path.unwrap();
        let receipt: Value =
            serde_json::from_slice(&fs::read(backup.join("execution-receipt.json")).unwrap())
                .unwrap();
        assert_eq!(receipt["operations"][0]["status"], "failed");
        assert_eq!(receipt["operations"][1]["role"], "metadata");
        assert_eq!(receipt["operations"][1]["status"], "not_run");
        assert!(
            !serde_json::to_string(&receipt)
                .unwrap()
                .contains("private-command")
        );
    }
}
