use std::path::PathBuf;

use serde::Serialize;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    Confirmed,
    Candidate,
    Conflicting,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    Unchanged,
    Modified,
    Unknown,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Finding {
    pub product_id: String,
    pub rule_id: String,
    pub surface: String,
    pub path: PathBuf,
    pub locator: String,
    pub attribution: Attribution,
    pub integrity: Integrity,
    pub evidence: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Diagnostic {
    pub surface: String,
    pub path: PathBuf,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Coverage {
    pub scope: String,
    pub surfaces: Vec<String>,
    pub project_directories_scanned: bool,
    pub environment_overrides_scanned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScanReport {
    pub findings: Vec<Finding>,
    pub diagnostics: Vec<Diagnostic>,
    pub coverage: Coverage,
}

impl ScanReport {
    pub fn is_complete(&self) -> bool {
        self.diagnostics.is_empty()
    }
}
