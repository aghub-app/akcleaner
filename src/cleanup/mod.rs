mod executor;
mod filesystem;
mod json;
mod metadata;
mod model;
mod planner;

use std::path::Path;

use crate::{discovery::Finding, rules::RuleSet};

pub use model::{
    ChangeAction, CleanupError, CleanupPlan, CleanupResult, PlannedChange, SkippedItem,
};

/// Build an opaque, read-only cleanup plan from current scan findings.
pub fn prepare(
    home: &Path,
    rules: &[RuleSet],
    selected: &[Finding],
) -> Result<CleanupPlan, CleanupError> {
    planner::prepare(home, rules, selected)
}

/// Execute a previously prepared plan after rechecking every snapshot and backing it up.
pub fn execute(plan: CleanupPlan) -> Result<CleanupResult, CleanupError> {
    executor::execute(plan)
}
