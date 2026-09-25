use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    discovery::{self, Attribution, Finding, Integrity},
    rules::{Evidence, RuleSet},
    superset_canonicalization,
};

use super::{
    filesystem, json,
    model::{
        ChangeAction, CleanupError, CleanupPlan, MetadataUpdate, Operation, PlannedChange,
        SkippedItem, Snapshot,
    },
};

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
static BACKUP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
struct SelectedFinding {
    finding: Finding,
    evidence: Evidence,
}

struct Planner<'a> {
    home: &'a Path,
    snapshots: BTreeMap<PathBuf, Snapshot>,
    operations: Vec<Operation>,
    metadata_updates: Vec<MetadataUpdate>,
    changes: BTreeMap<PathBuf, PlannedChange>,
    skipped: Vec<SkippedItem>,
    selected_item_count: usize,
}

/// Build an opaque, read-only cleanup plan from current scan findings.
pub(super) fn prepare(
    home: &Path,
    rules: &[RuleSet],
    selected: &[Finding],
) -> Result<CleanupPlan, CleanupError> {
    let home = absolute_home(home)?;
    filesystem::ensure_home(&home).map_err(|_| CleanupError::HomeUnavailable)?;
    let report = discovery::scan(&home, rules);
    if report.diagnostics.iter().any(|diagnostic| {
        diagnostic.surface == "home"
            && matches!(
                diagnostic.code.as_str(),
                "home_missing" | "home_not_directory" | "home_unavailable" | "home_resolve_failed"
            )
    }) {
        return Err(CleanupError::HomeUnavailable);
    }

    let current: BTreeSet<_> = report.findings.iter().cloned().collect();
    let mut skipped = Vec::new();
    let mut eligible = Vec::new();
    let mut seen_selection = BTreeSet::new();
    for finding in selected {
        if !seen_selection.insert(finding.clone()) {
            continue;
        }
        if !current.contains(finding) {
            skipped.push(skip(
                finding.path.clone(),
                "Finding is stale or does not match the current scan.",
            ));
            continue;
        }
        let Some(evidence) = rule_evidence(rules, finding) else {
            skipped.push(skip(
                finding.path.clone(),
                "Finding has no matching rule in the supplied rule sets.",
            ));
            continue;
        };
        if !is_selectable(finding, &evidence) {
            skipped.push(skip(
                finding.path.clone(),
                "This finding is modified, conflicting, unknown, or unsupported for removal.",
            ));
            continue;
        }
        eligible.push(SelectedFinding {
            finding: finding.clone(),
            evidence,
        });
    }

    eligible.sort_by(|left, right| left.finding.cmp(&right.finding));
    let mut planner = Planner {
        home: &home,
        snapshots: BTreeMap::new(),
        operations: Vec::new(),
        metadata_updates: Vec::new(),
        changes: BTreeMap::new(),
        skipped,
        selected_item_count: 0,
    };

    let hooks = eligible
        .iter()
        .filter(|item| matches!(item.evidence, Evidence::CommandMarker { .. }))
        .cloned()
        .collect::<Vec<_>>();
    planner.plan_hooks(&hooks);

    let markers = eligible
        .iter()
        .filter(|item| matches!(item.evidence, Evidence::SkillMarker { .. }))
        .cloned()
        .collect::<Vec<_>>();
    planner.plan_skill_markers(&markers);

    let manifests = eligible
        .iter()
        .filter(|item| matches!(item.evidence, Evidence::SkillManifest { .. }))
        .cloned()
        .collect::<Vec<_>>();
    planner.plan_manifests(&manifests);

    let mcps = eligible
        .iter()
        .filter(|item| matches!(item.evidence, Evidence::McpLedger { .. }))
        .cloned()
        .collect::<Vec<_>>();
    planner.plan_mcp(&mcps);

    let required_snapshots = planner
        .operations
        .iter()
        .map(|operation| operation_relative(operation).clone())
        .chain(
            planner
                .metadata_updates
                .iter()
                .map(|update| metadata_relative(update).clone()),
        )
        .collect::<BTreeSet<_>>();
    planner
        .snapshots
        .retain(|relative, _| required_snapshots.contains(relative));

    planner
        .operations
        .sort_by(|left, right| operation_relative(left).cmp(operation_relative(right)));
    planner
        .metadata_updates
        .sort_by(|left, right| metadata_relative(left).cmp(metadata_relative(right)));
    planner
        .skipped
        .sort_by(|left, right| (&left.path, &left.reason).cmp(&(&right.path, &right.reason)));
    planner.skipped.dedup();

    let backup_relative = backup_relative_path();
    let backup_root = home.join(&backup_relative);
    Ok(CleanupPlan {
        changes: planner.changes.into_values().collect(),
        skipped: planner.skipped,
        backup_root,
        backup_relative,
        home: home.clone(),
        snapshots: planner.snapshots,
        operations: planner.operations,
        metadata_updates: planner.metadata_updates,
        selected_item_count: planner.selected_item_count,
    })
}

impl Planner<'_> {
    fn plan_hooks(&mut self, selected: &[SelectedFinding]) {
        let grouped = group_by_path(selected, self.home, &mut self.skipped);
        for (relative, items) in grouped {
            let snapshot = match load_snapshot(self.home, &relative, &mut self.snapshots) {
                Ok(snapshot) => snapshot.clone(),
                Err(reason) => {
                    skip_items(&mut self.skipped, &items, reason);
                    continue;
                }
            };
            let targets = items
                .iter()
                .filter_map(|item| match &item.evidence {
                    Evidence::CommandMarker { marker } => Some(json::HookTarget {
                        locator: item.finding.locator.clone(),
                        marker: marker.clone(),
                    }),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let mut unique = BTreeMap::<(String, String), json::HookTarget>::new();
            for target in targets {
                unique
                    .entry((target.locator.clone(), target.marker.clone()))
                    .or_insert(target);
            }
            let targets = unique.into_values().collect::<Vec<_>>();
            let edited = match json::edit_hooks(&snapshot.bytes, &targets) {
                Ok(bytes) => bytes,
                Err(_) => {
                    skip_items(
                        &mut self.skipped,
                        &items,
                        "The hook document is ambiguous or the selected action no longer matches its marker.",
                    );
                    continue;
                }
            };
            let unique_locators = items
                .iter()
                .map(|item| item.finding.locator.clone())
                .collect::<BTreeSet<_>>();
            let item_count = unique_locators.len();
            let locators = unique_locators.into_iter().collect::<Vec<_>>();
            add_change(
                &mut self.changes,
                self.home.join(&relative),
                ChangeAction::EditJson,
                item_count,
                &locators,
            );
            self.operations.push(Operation::Replace {
                relative,
                bytes: edited,
                mode: snapshot.mode,
                item_count,
            });
            self.selected_item_count += item_count;
        }
    }

    fn plan_skill_markers(&mut self, selected: &[SelectedFinding]) {
        let mut grouped = BTreeMap::<PathBuf, Vec<SelectedFinding>>::new();
        for item in selected {
            let Some(relative) = relative_under(self.home, &item.finding.path) else {
                self.skipped.push(skip(
                    item.finding.path.clone(),
                    "Finding path is outside the approved home.",
                ));
                continue;
            };
            grouped.entry(relative).or_default().push(item.clone());
        }
        for (relative, items) in grouped {
            let snapshot = match load_snapshot(self.home, &relative, &mut self.snapshots) {
                Ok(snapshot) => snapshot.clone(),
                Err(reason) => {
                    skip_items(&mut self.skipped, &items, reason);
                    continue;
                }
            };
            let current = String::from_utf8_lossy(&snapshot.bytes);
            let all_markers_match = items.iter().all(|item| match &item.evidence {
                Evidence::SkillMarker { marker } => current.contains(marker),
                _ => false,
            });
            if !all_markers_match {
                skip_items(
                    &mut self.skipped,
                    &items,
                    "The selected SKILL.md no longer contains its managed marker.",
                );
                continue;
            }
            let locators = items
                .iter()
                .map(|item| item.finding.locator.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let item_count = 1;
            add_change(
                &mut self.changes,
                self.home.join(&relative),
                ChangeAction::RemoveFile,
                item_count,
                &locators,
            );
            self.operations.push(Operation::Remove {
                relative,
                item_count,
            });
            self.selected_item_count += item_count;
        }
    }

    fn plan_manifests(&mut self, selected: &[SelectedFinding]) {
        let mut by_manifest = BTreeMap::<PathBuf, Vec<(SelectedFinding, String, PathBuf)>>::new();
        for item in selected {
            let Evidence::SkillManifest { filename } = &item.evidence else {
                continue;
            };
            let Some(relative_file) = relative_under(self.home, &item.finding.path) else {
                self.skipped.push(skip(
                    item.finding.path.clone(),
                    "Manifest-listed file is outside the approved home.",
                ));
                continue;
            };
            let Some(skill_directory) = skill_directory_for(&item.finding.surface, &relative_file)
            else {
                self.skipped.push(skip(
                    item.finding.path.clone(),
                    "Manifest-listed file has no skill directory.",
                ));
                continue;
            };
            let Some(file_name) = relative_file
                .strip_prefix(&skill_directory)
                .ok()
                .filter(|path| is_normal_relative(path))
                .and_then(Path::to_str)
            else {
                self.skipped.push(skip(
                    item.finding.path.clone(),
                    "Manifest-listed file name is unsupported.",
                ));
                continue;
            };
            let manifest_relative = skill_directory.join(filename);
            if !is_normal_relative(&manifest_relative) {
                self.skipped.push(skip(
                    item.finding.path.clone(),
                    "Skill manifest path is not a safe relative path.",
                ));
                continue;
            }
            by_manifest.entry(manifest_relative).or_default().push((
                item.clone(),
                file_name.to_owned(),
                relative_file,
            ));
        }

        for (manifest_relative, items) in by_manifest {
            let manifest_snapshot =
                match load_snapshot(self.home, &manifest_relative, &mut self.snapshots) {
                    Ok(snapshot) => snapshot.clone(),
                    Err(reason) => {
                        skip_manifest_items(&mut self.skipped, &items, reason);
                        continue;
                    }
                };
            let manifest = match json::parse_manifest(&manifest_snapshot.bytes) {
                Ok(manifest) => manifest,
                Err(_) => {
                    skip_manifest_items(
                        &mut self.skipped,
                        &items,
                        "Skill manifest is malformed, ambiguous, or has an unsupported schema.",
                    );
                    continue;
                }
            };
            let mut valid = Vec::new();
            for (item, filename, relative_file) in &items {
                let Some(expected) = manifest.files.get(filename) else {
                    self.skipped.push(skip(
                        item.finding.path.clone(),
                        "Selected file is no longer listed by its skill manifest.",
                    ));
                    continue;
                };
                if !is_sha256(expected) {
                    self.skipped.push(skip(
                        item.finding.path.clone(),
                        "Skill manifest contains an invalid file hash.",
                    ));
                    continue;
                }
                let file_snapshot =
                    match load_snapshot(self.home, relative_file, &mut self.snapshots) {
                        Ok(snapshot) => snapshot,
                        Err(reason) => {
                            self.skipped.push(skip(item.finding.path.clone(), reason));
                            continue;
                        }
                    };
                if !hex_lower(&sha256(&file_snapshot.bytes)).eq_ignore_ascii_case(expected) {
                    self.skipped.push(skip(
                        item.finding.path.clone(),
                        "Selected manifest file has changed since the scan.",
                    ));
                    continue;
                }
                valid.push((item.clone(), filename.clone(), relative_file.clone()));
            }
            if valid.is_empty() {
                continue;
            }

            let filenames = valid
                .iter()
                .map(|(_, filename, _)| filename.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let (_edited_manifest, _) = match json::edit_manifest(
                &manifest_snapshot.bytes,
                &filenames,
            ) {
                Ok(result) => result,
                Err(_) => {
                    skip_manifest_items(
                        &mut self.skipped,
                        &valid,
                        "Skill manifest became ambiguous while preparing the selected removals.",
                    );
                    continue;
                }
            };
            let empty_after_selection = filenames.len() == manifest.files.len();
            let locators = valid
                .iter()
                .map(|(item, _, _)| item.finding.locator.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let successful_targets = valid
                .iter()
                .map(|(_, filename, target)| (filename.clone(), target.clone()))
                .collect::<Vec<_>>();

            for (_, _, target) in &valid {
                if self
                    .operations
                    .iter()
                    .any(|operation| operation_relative(operation) == target)
                {
                    continue;
                }
                add_change(
                    &mut self.changes,
                    self.home.join(target),
                    ChangeAction::RemoveFile,
                    1,
                    &locators,
                );
                self.operations.push(Operation::Remove {
                    relative: target.clone(),
                    item_count: 1,
                });
                self.selected_item_count += 1;
            }

            let expected_action = if empty_after_selection {
                ChangeAction::RemoveFile
            } else {
                ChangeAction::EditJson
            };
            add_change(
                &mut self.changes,
                self.home.join(&manifest_relative),
                expected_action,
                0,
                &locators,
            );
            self.metadata_updates.push(MetadataUpdate::Manifest {
                relative: manifest_relative,
                mode: manifest_snapshot.mode,
                selected: successful_targets,
            });
        }
    }

    fn plan_mcp(&mut self, selected: &[SelectedFinding]) {
        let relative_config = PathBuf::from(".claude.json");
        let grouped = group_by_path(selected, self.home, &mut self.skipped);
        for (relative, items) in grouped {
            if relative != relative_config {
                skip_items(
                    &mut self.skipped,
                    &items,
                    "MCP finding is outside the approved Claude user configuration.",
                );
                continue;
            }
            let Some((_, ledger_path)) = items.iter().find_map(|item| match &item.evidence {
                Evidence::McpLedger { path } => Some((item, PathBuf::from(path))),
                _ => None,
            }) else {
                skip_items(
                    &mut self.skipped,
                    &items,
                    "MCP ownership ledger is unavailable.",
                );
                continue;
            };
            if !is_normal_relative(&ledger_path) {
                skip_items(
                    &mut self.skipped,
                    &items,
                    "MCP ownership ledger path is unsafe.",
                );
                continue;
            }
            if items.iter().any(|item| {
            !matches!(&item.evidence, Evidence::McpLedger { path } if Path::new(path) == ledger_path)
        }) {
            skip_items(&mut self.skipped, &items, "Selected MCP findings use inconsistent ledgers.");
            continue;
        }

            let config_snapshot = match load_snapshot(self.home, &relative, &mut self.snapshots) {
                Ok(snapshot) => snapshot.clone(),
                Err(reason) => {
                    skip_items(&mut self.skipped, &items, reason);
                    continue;
                }
            };
            let config = match json::parse(&config_snapshot.bytes) {
                Ok(document) => document.value,
                Err(_) => {
                    skip_items(
                        &mut self.skipped,
                        &items,
                        "Claude MCP configuration is malformed or ambiguous.",
                    );
                    continue;
                }
            };
            let Some(servers) = config.get("mcpServers").and_then(Value::as_object) else {
                skip_items(
                    &mut self.skipped,
                    &items,
                    "Claude MCP server map is unavailable.",
                );
                continue;
            };
            let ledger_snapshot = match load_snapshot(self.home, &ledger_path, &mut self.snapshots)
            {
                Ok(snapshot) => snapshot.clone(),
                Err(reason) => {
                    skip_items(&mut self.skipped, &items, reason);
                    continue;
                }
            };
            let ledger = match json::parse_ledger(&ledger_snapshot.bytes) {
                Ok(ledger) if valid_ledger(&ledger) => ledger,
                _ => {
                    skip_items(
                        &mut self.skipped,
                        &items,
                        "MCP ownership ledger is malformed or ambiguous.",
                    );
                    continue;
                }
            };
            let config_path = self.home.join(&relative).to_string_lossy().into_owned();
            let Some(tracked) = ledger.files.get(&config_path) else {
                skip_items(
                    &mut self.skipped,
                    &items,
                    "MCP ownership ledger no longer tracks this configuration.",
                );
                continue;
            };

            let mut names = BTreeSet::new();
            let mut valid_items = Vec::new();
            for item in &items {
                let name = match json::parse_mcp_locator(&item.finding.locator) {
                    Ok(name) => name,
                    Err(_) => {
                        self.skipped
                            .push(skip(item.finding.path.clone(), "MCP locator is invalid."));
                        continue;
                    }
                };
                let Some(server) = servers.get(&name) else {
                    self.skipped.push(skip(
                        item.finding.path.clone(),
                        "Selected MCP server no longer exists.",
                    ));
                    continue;
                };
                let Some(expected_hash) = tracked.get(&name) else {
                    self.skipped.push(skip(
                        item.finding.path.clone(),
                        "Selected MCP server is no longer in the ledger.",
                    ));
                    continue;
                };
                let hash_matches = superset_canonicalization::canonical_server_sha256(server)
                    .is_ok_and(|hash| hash.eq_ignore_ascii_case(expected_hash));
                if !hash_matches {
                    self.skipped.push(skip(
                        item.finding.path.clone(),
                        "Selected MCP server is modified or outside the verified hash subset.",
                    ));
                    continue;
                }
                names.insert(name);
                valid_items.push(item.clone());
            }
            if names.is_empty() {
                continue;
            }
            let names = names.into_iter().collect::<Vec<_>>();
            let locators = valid_items
                .iter()
                .map(|item| item.finding.locator.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let edited = match json::edit_mcp_config(&config_snapshot.bytes, &names) {
                Ok(edited) => edited,
                Err(_) => {
                    skip_items(
                        &mut self.skipped,
                        &valid_items,
                        "MCP configuration is ambiguous or changed during planning.",
                    );
                    continue;
                }
            };
            let (_edited_ledger, ledger_empty) =
                match json::edit_ledger(&ledger_snapshot.bytes, &config_path, &names) {
                    Ok(edited) => edited,
                    Err(_) => {
                        skip_items(
                            &mut self.skipped,
                            &valid_items,
                            "MCP ownership ledger is ambiguous or changed during planning.",
                        );
                        continue;
                    }
                };
            let item_count = names.len();
            add_change(
                &mut self.changes,
                self.home.join(&relative),
                ChangeAction::EditJson,
                item_count,
                &locators,
            );
            self.operations.push(Operation::Replace {
                relative: relative.clone(),
                bytes: edited,
                mode: config_snapshot.mode,
                item_count,
            });
            let ledger_action = if ledger_empty {
                ChangeAction::RemoveFile
            } else {
                ChangeAction::EditJson
            };
            add_change(
                &mut self.changes,
                self.home.join(&ledger_path),
                ledger_action,
                0,
                &locators,
            );
            self.metadata_updates.push(MetadataUpdate::Ledger {
                relative: ledger_path,
                mode: ledger_snapshot.mode,
                config_path,
                server_names: names,
                config_relative: relative,
            });
            self.selected_item_count += item_count;
        }
    }
}

fn load_snapshot<'a>(
    home: &Path,
    relative: &Path,
    snapshots: &'a mut BTreeMap<PathBuf, Snapshot>,
) -> Result<&'a Snapshot, &'static str> {
    if !is_normal_relative(relative) {
        return Err("Path is not a safe relative file path.");
    }
    if !snapshots.contains_key(relative) {
        let read = filesystem::read_file(home, relative, MAX_FILE_BYTES).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                "Selected file no longer exists."
            } else {
                "Selected path is a symlink, non-regular file, or unreadable."
            }
        })?;
        snapshots.insert(
            relative.to_owned(),
            Snapshot {
                hash: sha256(&read.bytes),
                bytes: read.bytes,
                mode: read.mode,
            },
        );
    }
    Ok(snapshots.get(relative).expect("snapshot inserted above"))
}

pub(super) fn snapshot_matches(home: &Path, relative: &Path, snapshot: &Snapshot) -> bool {
    filesystem::read_file(home, relative, MAX_FILE_BYTES).is_ok_and(|current| {
        current.mode == snapshot.mode && sha256(&current.bytes) == snapshot.hash
    })
}

fn group_by_path(
    selected: &[SelectedFinding],
    home: &Path,
    skipped: &mut Vec<SkippedItem>,
) -> BTreeMap<PathBuf, Vec<SelectedFinding>> {
    let mut grouped = BTreeMap::new();
    for item in selected {
        let Some(relative) = relative_under(home, &item.finding.path) else {
            skipped.push(skip(
                item.finding.path.clone(),
                "Finding path is outside the approved home.",
            ));
            continue;
        };
        grouped
            .entry(relative)
            .or_insert_with(Vec::new)
            .push(item.clone());
    }
    grouped
}

fn skip_items(skipped: &mut Vec<SkippedItem>, items: &[SelectedFinding], reason: &'static str) {
    skipped.extend(
        items
            .iter()
            .map(|item| skip(item.finding.path.clone(), reason)),
    );
}

fn skip_manifest_items<T>(
    skipped: &mut Vec<SkippedItem>,
    items: &[(SelectedFinding, String, T)],
    reason: &'static str,
) {
    skipped.extend(
        items
            .iter()
            .map(|(item, _, _)| skip(item.finding.path.clone(), reason)),
    );
}

fn rule_evidence(rules: &[RuleSet], finding: &Finding) -> Option<Evidence> {
    rules
        .iter()
        .find(|ruleset| ruleset.product.id == finding.product_id)
        .and_then(|ruleset| ruleset.rules.iter().find(|rule| rule.id == finding.rule_id))
        .filter(|rule| {
            rule.surfaces
                .iter()
                .any(|surface| surface == &finding.surface)
        })
        .map(|rule| rule.evidence.clone())
}

fn is_selectable(finding: &Finding, evidence: &Evidence) -> bool {
    match (&finding.attribution, &finding.integrity, evidence) {
        (Attribution::Candidate, Integrity::Unknown, Evidence::CommandMarker { .. }) => {
            finding.surface == "claude.hooks.user" || finding.surface == "codex.hooks.user"
        }
        (Attribution::Confirmed, Integrity::Unknown, Evidence::SkillMarker { .. }) => {
            finding
                .path
                .file_name()
                .is_some_and(|name| name == "SKILL.md")
                && finding.surface.ends_with(".skills.user")
        }
        (
            Attribution::Confirmed,
            Integrity::Unchanged,
            Evidence::SkillManifest { .. } | Evidence::McpLedger { .. },
        ) => true,
        _ => false,
    }
}

fn relative_under(home: &Path, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(home).ok()?.to_owned();
    is_normal_relative(&relative).then_some(relative)
}

fn skill_directory_for(surface: &str, file: &Path) -> Option<PathBuf> {
    let skills_root = match surface {
        "shared.skills.user" => Path::new(".agents/skills"),
        "claude.skills.user" => Path::new(".claude/skills"),
        "codex.skills.user" => Path::new(".codex/skills"),
        _ => return None,
    };
    let relative = file.strip_prefix(skills_root).ok()?;
    let skill_name = relative.components().next()?;
    let Component::Normal(_) = skill_name else {
        return None;
    };
    Some(skills_root.join(skill_name.as_os_str()))
}

fn is_normal_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn add_change(
    changes: &mut BTreeMap<PathBuf, PlannedChange>,
    path: PathBuf,
    action: ChangeAction,
    item_count: usize,
    locators: &[String],
) {
    let change = changes
        .entry(path.clone())
        .or_insert_with(|| PlannedChange {
            path,
            action,
            item_count: 0,
            locators: Vec::new(),
        });
    if change.action == ChangeAction::EditJson || action == ChangeAction::EditJson {
        change.action = ChangeAction::EditJson;
    }
    change.item_count += item_count;
    change.locators.extend(locators.iter().cloned());
    change.locators.sort();
    change.locators.dedup();
}

pub(super) fn operation_relative(operation: &Operation) -> &PathBuf {
    match operation {
        Operation::Remove { relative, .. } | Operation::Replace { relative, .. } => relative,
    }
}

pub(super) fn metadata_relative(update: &MetadataUpdate) -> &PathBuf {
    match update {
        MetadataUpdate::Manifest { relative, .. } | MetadataUpdate::Ledger { relative, .. } => {
            relative
        }
    }
}

pub(super) fn absolute_home(home: &Path) -> Result<PathBuf, CleanupError> {
    if home.is_absolute() {
        return Ok(home.to_owned());
    }
    std::env::current_dir()
        .map(|current| current.join(home))
        .map_err(|_| CleanupError::HomeResolve)
}

pub(super) fn backup_relative_path() -> PathBuf {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let count = BACKUP_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(
        ".local/state/akcleaner/backups/{time}-{}-{count}",
        std::process::id()
    ))
}

pub(super) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(super) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_ledger(ledger: &json::Ledger) -> bool {
    ledger.files.iter().all(|(config, entries)| {
        let absolute = Path::new(config).is_absolute()
            || config.starts_with(['/', '\\'])
            || (config.as_bytes().get(1) == Some(&b':')
                && config.as_bytes()[0].is_ascii_alphabetic());
        absolute && entries.values().all(|hash| is_sha256(hash))
    })
}

pub(super) fn skip(path: PathBuf, reason: impl Into<String>) -> SkippedItem {
    SkippedItem {
        path,
        reason: reason.into(),
    }
}
