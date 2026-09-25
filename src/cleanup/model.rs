use std::{collections::BTreeMap, fmt, io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CleanupError {
    #[error("could not resolve the supplied home path")]
    HomeResolve,
    #[error("supplied home is not an accessible directory")]
    HomeUnavailable,
    #[error("one or more planned files changed after preview; nothing was written")]
    PreflightChanged,
    #[error("cleanup backup could not be completed; originals were not modified")]
    Backup(#[source] io::Error),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ChangeAction {
    RemoveFile,
    EditJson,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PlannedChange {
    pub path: PathBuf,
    pub action: ChangeAction,
    pub item_count: usize,
    pub locators: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SkippedItem {
    pub path: PathBuf,
    pub reason: String,
}

pub struct CleanupPlan {
    pub changes: Vec<PlannedChange>,
    pub skipped: Vec<SkippedItem>,
    pub backup_root: PathBuf,
    pub(super) backup_relative: PathBuf,
    pub(super) home: PathBuf,
    pub(super) snapshots: BTreeMap<PathBuf, Snapshot>,
    pub(super) operations: Vec<Operation>,
    pub(super) metadata_updates: Vec<MetadataUpdate>,
    pub(super) selected_item_count: usize,
}

#[derive(Debug)]
pub struct CleanupResult {
    pub removed_items: usize,
    pub changed_files: usize,
    pub backup_path: Option<PathBuf>,
    pub failures: Vec<SkippedItem>,
}

#[derive(Clone)]
pub(super) struct Snapshot {
    pub(super) bytes: Vec<u8>,
    pub(super) hash: [u8; 32],
    pub(super) mode: u32,
}

pub(super) enum Operation {
    Remove {
        relative: PathBuf,
        item_count: usize,
    },
    Replace {
        relative: PathBuf,
        bytes: Vec<u8>,
        mode: u32,
        item_count: usize,
    },
}

pub(super) enum MetadataUpdate {
    Manifest {
        relative: PathBuf,
        mode: u32,
        selected: Vec<(String, PathBuf)>,
    },
    Ledger {
        relative: PathBuf,
        mode: u32,
        config_path: String,
        server_names: Vec<String>,
        config_relative: PathBuf,
    },
}

impl fmt::Debug for CleanupPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CleanupPlan")
            .field("changes", &self.changes)
            .field("skipped", &self.skipped)
            .field("backup_root", &self.backup_root)
            .field("snapshot_count", &self.snapshots.len())
            .field("operation_count", &self.operations.len())
            .finish_non_exhaustive()
    }
}
