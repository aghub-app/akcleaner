use std::path::{Component, Path};

use garde::Validate;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct RuleSet {
    #[garde(skip)]
    pub schema_version: u32,
    #[garde(dive)]
    pub product: Product,
    #[garde(dive)]
    pub verified: Verified,
    #[garde(length(min = 1), dive)]
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct Product {
    #[garde(length(min = 1))]
    pub id: String,
    #[garde(length(min = 1))]
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    #[garde(length(min = 1))]
    pub repository: String,
    #[garde(length(equal = 40))]
    pub commit: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[garde(length(min = 1))]
    pub id: String,
    #[garde(length(min = 1), inner(length(min = 1)))]
    pub surfaces: Vec<String>,
    #[garde(skip)]
    pub evidence: Evidence,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
    CommandMarker { marker: String },
    SkillMarker { marker: String },
    SkillManifest { filename: String },
    McpLedger { path: String },
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Surface {
    ClaudeHooksUser,
    CodexHooksUser,
    ClaudeMcpUser,
    SharedSkillsUser,
    ClaudeSkillsUser,
    CodexSkillsUser,
}

impl Surface {
    pub const ALL: [&'static str; 6] = [
        "claude.hooks.user",
        "codex.hooks.user",
        "claude.mcp.user",
        "shared.skills.user",
        "claude.skills.user",
        "codex.skills.user",
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "claude.hooks.user" => Self::ClaudeHooksUser,
            "codex.hooks.user" => Self::CodexHooksUser,
            "claude.mcp.user" => Self::ClaudeMcpUser,
            "shared.skills.user" => Self::SharedSkillsUser,
            "claude.skills.user" => Self::ClaudeSkillsUser,
            "codex.skills.user" => Self::CodexSkillsUser,
            _ => return None,
        })
    }

    fn is_hook(self) -> bool {
        matches!(self, Self::ClaudeHooksUser | Self::CodexHooksUser)
    }

    fn is_skill(self) -> bool {
        matches!(
            self,
            Self::SharedSkillsUser | Self::ClaudeSkillsUser | Self::CodexSkillsUser
        )
    }

    fn is_mcp(self) -> bool {
        self == Self::ClaudeMcpUser
    }
}

pub(crate) fn validate_rule_set(rule_set: &RuleSet) -> Result<(), String> {
    rule_set.validate().map_err(|error| error.to_string())?;
    if rule_set.schema_version != 1 {
        return Err(format!(
            "unsupported schema_version {}; expected 1",
            rule_set.schema_version
        ));
    }
    validate_id(&rule_set.product.id).map_err(|reason| format!("product.id {reason}"))?;
    if !rule_set.verified.repository.starts_with("https://") {
        return Err("verified.repository must be an https URL".to_owned());
    }
    if !rule_set
        .verified
        .commit
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("verified.commit must be a full hexadecimal SHA".to_owned());
    }

    let mut rule_ids = std::collections::BTreeSet::new();
    for rule in &rule_set.rules {
        validate_id(&rule.id).map_err(|reason| format!("rule.id {} {reason}", rule.id))?;
        if !rule_ids.insert(&rule.id) {
            return Err(format!("duplicate rule id {}", rule.id));
        }
        let mut surfaces = std::collections::BTreeSet::new();
        let parsed_surfaces = rule
            .surfaces
            .iter()
            .map(|surface| {
                Surface::parse(surface)
                    .ok_or_else(|| format!("rule {} has unsupported surface {surface:?}", rule.id))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for surface in &rule.surfaces {
            if !surfaces.insert(surface) {
                return Err(format!("rule {} repeats surface {surface:?}", rule.id));
            }
        }
        validate_evidence(&rule.evidence)?;
        let compatible = parsed_surfaces.iter().all(|surface| match &rule.evidence {
            Evidence::CommandMarker { .. } => surface.is_hook(),
            Evidence::SkillMarker { .. } | Evidence::SkillManifest { .. } => surface.is_skill(),
            Evidence::McpLedger { .. } => surface.is_mcp(),
        });
        if !compatible {
            return Err(format!(
                "rule {} has surfaces incompatible with its evidence type",
                rule.id
            ));
        }
    }
    Ok(())
}

fn validate_evidence(evidence: &Evidence) -> Result<(), String> {
    match evidence {
        Evidence::CommandMarker { marker } | Evidence::SkillMarker { marker } => {
            if marker.trim().is_empty() {
                return Err("evidence marker must not be empty".to_owned());
            }
        }
        Evidence::SkillManifest { filename } => {
            validate_relative_path(filename, "manifest filename")?
        }
        Evidence::McpLedger { path } => validate_relative_path(path, "ledger path")?,
    }
    Ok(())
}

pub(crate) fn validate_relative_path(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.starts_with(['/', '\\']) {
        return Err(format!("{label} must be a non-empty relative path"));
    }
    if value.as_bytes().get(1) == Some(&b':') && value.as_bytes()[0].is_ascii_alphabetic() {
        return Err(format!("{label} must not be absolute"));
    }
    let portable = value.replace('\\', "/");
    if portable.split('/').any(|part| part == "..") {
        return Err(format!("{label} must not contain parent traversal"));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("{label} must be a relative path without traversal"));
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), &'static str> {
    let valid = !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err("must be lowercase alphanumeric segments separated by single hyphens")
    }
}
