use std::collections::BTreeMap;

use crate::{
    discovery::{Attribution, Finding, Integrity, ScanReport},
    rules::{Evidence, RuleSet},
};

use super::presentation::safe_text;

/// One choice per installed product, regardless of its number of artifacts.
pub(super) struct Application {
    pub name: String,
    pub removable: Vec<usize>,
}

pub(super) fn applications(rules: &[RuleSet], report: &ScanReport) -> Vec<Application> {
    let mut groups = BTreeMap::<&str, Application>::new();
    for (index, finding) in report.findings.iter().enumerate() {
        let ruleset = rules
            .iter()
            .find(|rule| rule.product.id == finding.product_id);
        let evidence = ruleset.and_then(|ruleset| {
            ruleset
                .rules
                .iter()
                .find(|rule| rule.id == finding.rule_id)
                .map(|rule| &rule.evidence)
        });
        let group = groups
            .entry(&finding.product_id)
            .or_insert_with(|| Application {
                name: safe_text(
                    ruleset.map_or(finding.product_id.as_str(), |ruleset| &ruleset.product.name),
                ),
                removable: Vec::new(),
            });
        if can_remove(finding, evidence) {
            group.removable.push(index);
        }
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    groups
}

fn can_remove(finding: &Finding, evidence: Option<&Evidence>) -> bool {
    match (&finding.attribution, &finding.integrity, evidence) {
        (Attribution::Candidate, Integrity::Unknown, Some(Evidence::CommandMarker { .. })) => {
            matches!(
                finding.surface.as_str(),
                "claude.hooks.user" | "codex.hooks.user"
            )
        }
        (Attribution::Confirmed, Integrity::Unknown, Some(Evidence::SkillMarker { .. })) => {
            finding.surface.ends_with(".skills.user")
                && finding
                    .path
                    .file_name()
                    .is_some_and(|name| name == "SKILL.md")
        }
        (
            Attribution::Confirmed,
            Integrity::Unchanged,
            Some(Evidence::SkillManifest { .. } | Evidence::McpLedger { .. }),
        ) => true,
        _ => false,
    }
}
