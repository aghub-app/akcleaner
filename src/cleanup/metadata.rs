use std::{collections::BTreeSet, path::PathBuf};

use super::{
    filesystem, json,
    model::{CleanupPlan, MetadataUpdate},
    planner::snapshot_matches,
};

pub(super) fn execute_metadata_update(
    update: &MetadataUpdate,
    plan: &CleanupPlan,
    successful_removals: &BTreeSet<PathBuf>,
    successful_replacements: &BTreeSet<PathBuf>,
) -> Result<bool, PathBuf> {
    match update {
        MetadataUpdate::Manifest {
            relative,
            mode,
            selected,
        } => {
            let successful = selected
                .iter()
                .filter(|(_, target)| successful_removals.contains(target))
                .collect::<Vec<_>>();
            if successful.is_empty() {
                return Ok(false);
            }
            let Some(snapshot) = plan.snapshots.get(relative) else {
                return Err(plan.home.join(relative));
            };
            if !snapshot_matches(&plan.home, relative, snapshot) {
                return Err(plan.home.join(relative));
            }
            let names = successful
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let (edited, empty) = json::edit_manifest(&snapshot.bytes, &names)
                .map_err(|_| plan.home.join(relative))?;
            if empty {
                filesystem::remove_file(&plan.home, relative)
                    .map_err(|_| plan.home.join(relative))?;
            } else {
                filesystem::replace_file(&plan.home, relative, &edited, *mode)
                    .map_err(|_| plan.home.join(relative))?;
            }
            Ok(true)
        }
        MetadataUpdate::Ledger {
            relative,
            mode,
            config_path,
            server_names,
            config_relative,
        } => {
            if !successful_replacements.contains(config_relative) {
                return Ok(false);
            }
            let Some(snapshot) = plan.snapshots.get(relative) else {
                return Err(plan.home.join(relative));
            };
            if !snapshot_matches(&plan.home, relative, snapshot) {
                return Err(plan.home.join(relative));
            }
            let (edited, empty) = json::edit_ledger(&snapshot.bytes, config_path, server_names)
                .map_err(|_| plan.home.join(relative))?;
            if empty {
                filesystem::remove_file(&plan.home, relative)
                    .map_err(|_| plan.home.join(relative))?;
            } else {
                filesystem::replace_file(&plan.home, relative, &edited, *mode)
                    .map_err(|_| plan.home.join(relative))?;
            }
            Ok(true)
        }
    }
}
