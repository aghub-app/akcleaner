use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    hosts::{self, HostArtifact, HostInventory, SkillDirectory},
    rules::{Evidence, Rule, RuleSet, Surface, validate_relative_path},
    superset_canonicalization,
};

use super::{
    Attribution, Coverage, Diagnostic, Finding, Integrity, ScanReport,
    safe_read::{
        MAX_MANAGED_FILE_BYTES, MAX_MANIFEST_BYTES, SafeReadError, read_under,
        verify_directory_under,
    },
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileManifest {
    version: u32,
    files: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpLedger {
    version: u32,
    files: BTreeMap<String, BTreeMap<String, String>>,
}

pub fn scan(home: &Path, rulesets: &[RuleSet]) -> ScanReport {
    let surfaces = requested_surfaces(rulesets);
    let absolute_home = match prepare_home(home, std::env::current_dir()) {
        Ok(path) => path,
        Err(diagnostic) => return failed_home_report(surfaces, diagnostic),
    };
    let inventory = hosts::discover(&absolute_home, &surfaces);
    scan_with_inventory(&absolute_home, rulesets, inventory)
}

fn prepare_home(home: &Path, current_dir: io::Result<PathBuf>) -> Result<PathBuf, Diagnostic> {
    let absolute_home = if home.is_absolute() {
        home.to_owned()
    } else {
        current_dir
            .map(|current| current.join(home))
            .map_err(|_| Diagnostic {
                surface: "home".to_owned(),
                path: home.to_owned(),
                code: "home_resolve_failed".to_owned(),
                message: "Could not resolve the supplied relative home path.".to_owned(),
            })?
    };
    let metadata = fs::metadata(&absolute_home).map_err(|error| Diagnostic {
        surface: "home".to_owned(),
        path: absolute_home.clone(),
        code: if error.kind() == io::ErrorKind::NotFound {
            "home_missing"
        } else {
            "home_unavailable"
        }
        .to_owned(),
        message: "The supplied home directory could not be accessed.".to_owned(),
    })?;
    if !metadata.is_dir() {
        return Err(Diagnostic {
            surface: "home".to_owned(),
            path: absolute_home,
            code: "home_not_directory".to_owned(),
            message: "The supplied home path is not a directory.".to_owned(),
        });
    }
    Ok(absolute_home)
}

fn failed_home_report(surfaces: Vec<String>, diagnostic: Diagnostic) -> ScanReport {
    ScanReport {
        findings: Vec::new(),
        diagnostics: vec![diagnostic],
        coverage: coverage(surfaces),
    }
}

fn coverage(surfaces: Vec<String>) -> Coverage {
    Coverage {
        scope: "Default user roots for the supplied home; project directories and profile/environment overrides are not scanned.".to_owned(),
        surfaces,
        project_directories_scanned: false,
        environment_overrides_scanned: false,
    }
}

pub(crate) fn scan_with_inventory(
    home: &Path,
    rulesets: &[RuleSet],
    inventory: HostInventory,
) -> ScanReport {
    let mut findings = Vec::new();
    let HostInventory {
        artifacts,
        skill_directories,
        diagnostics: host_diagnostics,
    } = inventory;
    let mut diagnostics = host_diagnostics
        .into_iter()
        .map(|diagnostic| Diagnostic {
            surface: diagnostic.surface,
            path: diagnostic.path,
            code: diagnostic.code,
            message: diagnostic.message,
        })
        .collect::<Vec<_>>();
    let mut processed_manifests = BTreeSet::new();
    let supported_surfaces = requested_surfaces(rulesets);

    for artifact in &artifacts {
        if Surface::parse(&artifact.surface).is_none() {
            diagnostics.push(Diagnostic {
                surface: artifact.surface.clone(),
                path: artifact.path.clone(),
                code: "host_unknown_surface".to_owned(),
                message: "Host adapter returned an unsupported surface.".to_owned(),
            });
            continue;
        }
        for ruleset in rulesets {
            for rule in &ruleset.rules {
                if !rule
                    .surfaces
                    .iter()
                    .any(|surface| surface == &artifact.surface)
                {
                    continue;
                }
                match &rule.evidence {
                    Evidence::CommandMarker { marker } => {
                        if command_marker_matches(artifact, marker) {
                            findings.push(finding(
                                ruleset,
                                rule,
                                artifact,
                                Attribution::Candidate,
                                Integrity::Unknown,
                                "Command marker matched; candidate attribution only.",
                            ));
                        }
                    }
                    Evidence::SkillMarker { marker } => {
                        if skill_marker_matches(artifact, marker) {
                            findings.push(finding(
                                ruleset,
                                rule,
                                artifact,
                                Attribution::Confirmed,
                                Integrity::Unknown,
                                "Managed marker found in SKILL.md; attribution covers this document only.",
                            ));
                        }
                    }
                    Evidence::SkillManifest { .. } => {}
                    Evidence::McpLedger { path } => {
                        scan_ledger(
                            home,
                            ruleset,
                            rule,
                            artifact,
                            path,
                            &mut findings,
                            &mut diagnostics,
                        );
                    }
                }
            }
        }
    }

    for directory in &skill_directories {
        if Surface::parse(&directory.surface).is_none() {
            diagnostics.push(Diagnostic {
                surface: directory.surface.clone(),
                path: directory.path.clone(),
                code: "host_unknown_surface".to_owned(),
                message: "Host adapter returned an unsupported skill surface.".to_owned(),
            });
            continue;
        }
        if !skill_root_in_scope(home, directory) {
            diagnostics.push(Diagnostic {
                surface: directory.surface.clone(),
                path: directory.path.clone(),
                code: "skill_inventory_invalid".to_owned(),
                message: "Host skill directory is outside the immediate approved skill roots."
                    .to_owned(),
            });
            continue;
        }
        if let Err(error) = verify_directory_under(home, &directory.path) {
            diagnostics.push(read_diagnostic(
                &directory.surface,
                &directory.path,
                "manifest_root",
                error,
            ));
            continue;
        }
        for ruleset in rulesets {
            for rule in &ruleset.rules {
                if !rule
                    .surfaces
                    .iter()
                    .any(|surface| surface == &directory.surface)
                {
                    continue;
                }
                let Evidence::SkillManifest { filename } = &rule.evidence else {
                    continue;
                };
                let cache_key = (
                    ruleset.product.id.clone(),
                    rule.id.clone(),
                    directory.surface.clone(),
                    directory.path.clone(),
                    filename.clone(),
                );
                if processed_manifests.insert(cache_key) {
                    scan_manifest(
                        ruleset,
                        rule,
                        &directory.surface,
                        &ManifestLocation {
                            approved_home: home,
                            skill_dir: &directory.path,
                        },
                        filename,
                        &mut findings,
                        &mut diagnostics,
                    );
                }
            }
        }
    }

    findings.sort();
    findings.dedup();
    mark_conflicts(&mut findings);
    findings.sort();
    diagnostics.sort();
    diagnostics.dedup();
    ScanReport {
        findings,
        diagnostics,
        coverage: coverage(supported_surfaces),
    }
}

fn requested_surfaces(rulesets: &[RuleSet]) -> Vec<String> {
    rulesets
        .iter()
        .flat_map(|ruleset| ruleset.rules.iter())
        .flat_map(|rule| rule.surfaces.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn command_marker_matches(artifact: &HostArtifact, marker: &str) -> bool {
    artifact
        .value
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(marker))
}

fn skill_marker_matches(artifact: &HostArtifact, marker: &str) -> bool {
    artifact
        .value
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(|content| content.contains(marker))
}

fn finding(
    ruleset: &RuleSet,
    rule: &Rule,
    artifact: &HostArtifact,
    attribution: Attribution,
    integrity: Integrity,
    evidence: &str,
) -> Finding {
    Finding {
        product_id: ruleset.product.id.clone(),
        rule_id: rule.id.clone(),
        surface: artifact.surface.clone(),
        path: artifact.path.clone(),
        locator: artifact.locator.clone(),
        attribution,
        integrity,
        evidence: evidence.to_owned(),
    }
}

fn skill_root_in_scope(home: &Path, directory: &SkillDirectory) -> bool {
    let root = match directory.surface.as_str() {
        "shared.skills.user" => Path::new(".agents/skills"),
        "claude.skills.user" => Path::new(".claude/skills"),
        "codex.skills.user" => Path::new(".codex/skills"),
        _ => return false,
    };
    let Some(relative) = directory.path.strip_prefix(home).ok() else {
        return false;
    };
    let Some(child) = relative.strip_prefix(root).ok() else {
        return false;
    };
    let mut components = child.components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

struct ManifestLocation<'a> {
    approved_home: &'a Path,
    skill_dir: &'a Path,
}

fn scan_manifest(
    ruleset: &RuleSet,
    rule: &Rule,
    surface: &str,
    location: &ManifestLocation<'_>,
    filename: &str,
    findings: &mut Vec<Finding>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let loaded = match read_under(
        location.approved_home,
        location.skill_dir,
        filename,
        MAX_MANIFEST_BYTES,
    ) {
        Ok(Some(value)) => value,
        Ok(None) => return,
        Err(error) => {
            diagnostics.push(read_diagnostic(
                surface,
                &location.skill_dir.join(filename),
                "manifest",
                error,
            ));
            return;
        }
    };
    let (manifest_path, bytes) = loaded;
    let manifest: FileManifest = match serde_json::from_slice::<FileManifest>(&bytes) {
        Ok(manifest) if manifest.version == 1 => manifest,
        Ok(_) => {
            diagnostics.push(Diagnostic {
                surface: surface.to_owned(),
                path: manifest_path,
                code: "manifest_schema_unsupported".to_owned(),
                message: "Only version 1 skill manifests are supported.".to_owned(),
            });
            return;
        }
        Err(_) => {
            diagnostics.push(Diagnostic {
                surface: surface.to_owned(),
                path: manifest_path,
                code: "manifest_invalid".to_owned(),
                message: "Skill manifest is invalid JSON or has an unsupported schema.".to_owned(),
            });
            return;
        }
    };

    for (relative, expected_hash) in manifest.files {
        if let Err(reason) = validate_relative_path(&relative, "manifest filename") {
            diagnostics.push(Diagnostic {
                surface: surface.to_owned(),
                path: location.skill_dir.join(&relative),
                code: "invalid_manifest_path".to_owned(),
                message: reason,
            });
            continue;
        }
        if !is_sha256(&expected_hash) {
            diagnostics.push(Diagnostic {
                surface: surface.to_owned(),
                path: location.skill_dir.join(&relative),
                code: "manifest_hash_invalid".to_owned(),
                message: "Manifest contains a hash that is not 64 hexadecimal characters."
                    .to_owned(),
            });
            continue;
        }
        let file = match read_under(
            location.approved_home,
            location.skill_dir,
            &relative,
            MAX_MANAGED_FILE_BYTES,
        ) {
            Ok(Some(file)) => file,
            Ok(None) => {
                diagnostics.push(Diagnostic {
                    surface: surface.to_owned(),
                    path: location.skill_dir.join(&relative),
                    code: "manifest_file_missing".to_owned(),
                    message: "Manifest lists a file that is missing.".to_owned(),
                });
                continue;
            }
            Err(error) => {
                diagnostics.push(read_diagnostic(
                    surface,
                    &location.skill_dir.join(&relative),
                    "manifest_file",
                    error,
                ));
                continue;
            }
        };
        let (path, contents) = file;
        let hash = sha256_hex(&contents);
        let unchanged = hash.eq_ignore_ascii_case(&expected_hash);
        findings.push(Finding {
            product_id: ruleset.product.id.clone(),
            rule_id: rule.id.clone(),
            surface: surface.to_owned(),
            path,
            locator: String::new(),
            attribution: Attribution::Confirmed,
            integrity: if unchanged {
                Integrity::Unchanged
            } else {
                Integrity::Modified
            },
            evidence: if unchanged {
                "File SHA-256 matches the product manifest.".to_owned()
            } else {
                "File SHA-256 differs from the product manifest.".to_owned()
            },
        });
    }
}

fn scan_ledger(
    home: &Path,
    ruleset: &RuleSet,
    rule: &Rule,
    artifact: &HostArtifact,
    relative_path: &str,
    findings: &mut Vec<Finding>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let config_path = home.join(".claude.json");
    if artifact.surface != "claude.mcp.user" || artifact.path != config_path {
        return;
    }
    let loaded = match read_under(home, home, relative_path, MAX_MANIFEST_BYTES) {
        Ok(Some(value)) => value,
        Ok(None) => return,
        Err(error) => {
            diagnostics.push(read_diagnostic(
                &artifact.surface,
                &home.join(relative_path),
                "ledger",
                error,
            ));
            return;
        }
    };
    let (ledger_path, bytes) = loaded;
    let ledger: McpLedger = match serde_json::from_slice::<McpLedger>(&bytes) {
        Ok(ledger) if ledger.version == 1 => ledger,
        Ok(_) => {
            diagnostics.push(Diagnostic {
                surface: artifact.surface.clone(),
                path: ledger_path,
                code: "ledger_schema_unsupported".to_owned(),
                message: "Only version 1 MCP ledgers are supported.".to_owned(),
            });
            return;
        }
        Err(_) => {
            diagnostics.push(Diagnostic {
                surface: artifact.surface.clone(),
                path: ledger_path,
                code: "ledger_invalid".to_owned(),
                message: "MCP ledger is invalid JSON or has an unsupported schema.".to_owned(),
            });
            return;
        }
    };
    for (config, entries) in &ledger.files {
        if !is_portably_absolute(config) {
            diagnostics.push(Diagnostic {
                surface: artifact.surface.clone(),
                path: ledger_path.clone(),
                code: "ledger_config_path_invalid".to_owned(),
                message: "MCP ledger configuration keys must be absolute paths.".to_owned(),
            });
            return;
        }
        for hash in entries.values() {
            if !is_sha256(hash) {
                diagnostics.push(Diagnostic {
                    surface: artifact.surface.clone(),
                    path: ledger_path.clone(),
                    code: "ledger_hash_invalid".to_owned(),
                    message: "MCP ledger contains a hash that is not 64 hexadecimal characters."
                        .to_owned(),
                });
                return;
            }
        }
    }
    let expected_config = config_path.to_string_lossy();
    let Some(entries) = ledger.files.get(expected_config.as_ref()) else {
        return;
    };
    for (entry_name, expected_hash) in entries {
        if artifact.locator != mcp_locator(entry_name) {
            continue;
        }
        match superset_canonicalization::canonical_server_sha256(&artifact.value) {
            Ok(actual_hash) => {
                let unchanged = actual_hash.eq_ignore_ascii_case(expected_hash);
                findings.push(finding(
                    ruleset,
                    rule,
                    artifact,
                    Attribution::Confirmed,
                    if unchanged {
                        Integrity::Unchanged
                    } else {
                        Integrity::Modified
                    },
                    if unchanged {
                        "Superset MCP ledger hash matches; historical management association confirmed."
                    } else {
                        "Superset MCP ledger hash differs; historical management association only."
                    },
                ));
            }
            Err(superset_canonicalization::CanonicalizationError::UnsupportedShape) => {
                findings.push(finding(
                    ruleset,
                    rule,
                    artifact,
                    Attribution::Confirmed,
                    Integrity::Unknown,
                    "Superset ledger tracks this entry; canonicalization is unsupported, so integrity is unknown.",
                ));
                diagnostics.push(Diagnostic {
                    surface: artifact.surface.clone(),
                    path: artifact.path.clone(),
                    code: "ledger_canonicalization_unsupported".to_owned(),
                    message: "MCP value is outside the verified canonicalization subset; integrity is unknown.".to_owned(),
                });
            }
        }
    }
}

fn read_diagnostic(surface: &str, path: &Path, kind: &str, error: SafeReadError) -> Diagnostic {
    let (suffix, message) = match error {
        SafeReadError::Symlink => ("symlink_skipped", "Path contains a symbolic link; skipped."),
        SafeReadError::Missing => ("root_missing", "Expected scan root is missing."),
        SafeReadError::NotDirectory => ("not_directory", "Expected a directory; skipped."),
        SafeReadError::NotRegularFile => ("not_regular_file", "Expected a regular file; skipped."),
        SafeReadError::TooLarge => ("too_large", "Input exceeds the supported size limit."),
        SafeReadError::InvalidPath => ("path_invalid", "Path is not a safe relative path."),
        SafeReadError::Io => ("read_failed", "Input could not be read."),
    };
    Diagnostic {
        surface: surface.to_owned(),
        path: path.to_owned(),
        code: format!("{kind}_{suffix}"),
        message: message.to_owned(),
    }
}

fn mark_conflicts(findings: &mut [Finding]) {
    let mut products_by_artifact = BTreeMap::<(String, PathBuf, String), BTreeSet<String>>::new();
    for finding in findings.iter() {
        if finding.attribution != Attribution::Candidate {
            products_by_artifact
                .entry((
                    finding.surface.clone(),
                    finding.path.clone(),
                    finding.locator.clone(),
                ))
                .or_default()
                .insert(finding.product_id.clone());
        }
    }
    for finding in findings {
        let key = (
            finding.surface.clone(),
            finding.path.clone(),
            finding.locator.clone(),
        );
        if finding.attribution != Attribution::Candidate
            && products_by_artifact
                .get(&key)
                .is_some_and(|products| products.len() > 1)
        {
            finding.attribution = Attribution::Conflicting;
            finding
                .evidence
                .push_str(" Multiple products claim this artifact.");
        }
    }
}

fn mcp_locator(entry_name: &str) -> String {
    format!(
        "/mcpServers/{}",
        entry_name.replace('~', "~0").replace('/', "~1")
    )
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_portably_absolute(value: &str) -> bool {
    Path::new(value).is_absolute()
        || value.starts_with(['/', '\\'])
        || (value.as_bytes().get(1) == Some(&b':') && value.as_bytes()[0].is_ascii_alphabetic())
}

fn sha256_hex(value: &[u8]) -> String {
    hex(&Sha256::digest(value))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
pub(super) fn canonical_hash_for_test(value: &Value) -> String {
    superset_canonicalization::canonical_server_sha256(value).expect("supported MCP value")
}

#[cfg(test)]
mod preparation_tests {
    use super::prepare_home;
    use std::{io, path::Path};

    #[test]
    fn relative_home_resolution_failure_is_reported() {
        let error = prepare_home(
            Path::new("synthetic-home"),
            Err(io::Error::other("synthetic current directory failure")),
        )
        .unwrap_err();
        assert_eq!(error.code, "home_resolve_failed");
    }
}
