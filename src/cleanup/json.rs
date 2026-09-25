use std::collections::{BTreeMap, BTreeSet};

use jsonc_parser::{ParseOptions, cst::CstRootNode};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub(super) enum JsonError {
    #[error("document is not UTF-8 or strict JSON")]
    Invalid,
    #[error("document contains duplicate object keys")]
    DuplicateKeys,
    #[error("document has an unsupported cleanup schema")]
    UnsupportedSchema,
    #[error("selected artifact no longer matches its rule evidence")]
    EvidenceChanged,
}

pub(super) struct Document {
    pub root: CstRootNode,
    pub value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub version: u32,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ledger {
    pub version: u32,
    pub files: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug)]
pub(super) struct HookTarget {
    pub locator: String,
    pub marker: String,
}

pub(super) fn parse(bytes: &[u8]) -> Result<Document, JsonError> {
    let text = std::str::from_utf8(bytes).map_err(|_| JsonError::Invalid)?;
    let options = ParseOptions {
        allow_comments: false,
        allow_loose_object_property_names: false,
        allow_trailing_commas: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    };
    let root = CstRootNode::parse(text, &options).map_err(|_| JsonError::Invalid)?;
    let node = root.value().ok_or(JsonError::Invalid)?;
    reject_duplicate_keys(&node)?;
    let value = serde_json::from_str(text).map_err(|_| JsonError::Invalid)?;
    Ok(Document { root, value })
}

pub(super) fn parse_manifest(bytes: &[u8]) -> Result<Manifest, JsonError> {
    let document = parse(bytes)?;
    if !document.value.is_object() {
        return Err(JsonError::UnsupportedSchema);
    }
    let manifest: Manifest =
        serde_json::from_value(document.value).map_err(|_| JsonError::UnsupportedSchema)?;
    if manifest.version != 1 {
        return Err(JsonError::UnsupportedSchema);
    }
    Ok(manifest)
}

pub(super) fn parse_ledger(bytes: &[u8]) -> Result<Ledger, JsonError> {
    let document = parse(bytes)?;
    if !document.value.is_object() {
        return Err(JsonError::UnsupportedSchema);
    }
    let ledger: Ledger =
        serde_json::from_value(document.value).map_err(|_| JsonError::UnsupportedSchema)?;
    if ledger.version != 1 {
        return Err(JsonError::UnsupportedSchema);
    }
    Ok(ledger)
}

pub(super) fn edit_hooks(bytes: &[u8], targets: &[HookTarget]) -> Result<Vec<u8>, JsonError> {
    let document = parse(bytes)?;
    let root_value = document
        .value
        .as_object()
        .ok_or(JsonError::UnsupportedSchema)?;
    let hooks_value = root_value
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or(JsonError::EvidenceChanged)?;

    let mut grouped = BTreeMap::<String, BTreeMap<usize, BTreeSet<usize>>>::new();
    for target in targets {
        let (event, group, action) = parse_hook_locator(&target.locator)?;
        let selected_action = hooks_value
            .get(&event)
            .and_then(Value::as_array)
            .and_then(|groups| groups.get(group))
            .and_then(Value::as_object)
            .and_then(|group| group.get("hooks"))
            .and_then(Value::as_array)
            .and_then(|actions| actions.get(action))
            .ok_or(JsonError::EvidenceChanged)?;
        let matches = selected_action
            .as_object()
            .and_then(|action| action.get("type"))
            .and_then(Value::as_str)
            == Some("command")
            && selected_action
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains(&target.marker));
        if !matches {
            return Err(JsonError::EvidenceChanged);
        }
        grouped
            .entry(event)
            .or_default()
            .entry(group)
            .or_default()
            .insert(action);
    }

    let root_object = document
        .root
        .value()
        .and_then(|node| node.as_object())
        .ok_or(JsonError::UnsupportedSchema)?;
    let hooks_object = root_object
        .object_value("hooks")
        .ok_or(JsonError::EvidenceChanged)?;

    for (event, groups) in grouped {
        let event_property = hooks_object.get(&event).ok_or(JsonError::EvidenceChanged)?;
        let event_array = match event_property.value() {
            Some(node) => node.as_array().ok_or(JsonError::EvidenceChanged)?,
            None => return Err(JsonError::EvidenceChanged),
        };
        let original_groups = event_array.elements();
        let mut groups_to_remove = Vec::new();
        let mut remove_event = !original_groups.is_empty();

        for (group_index, action_indexes) in groups {
            let group_node = original_groups
                .get(group_index)
                .cloned()
                .ok_or(JsonError::EvidenceChanged)?;
            let group_value = hooks_value
                .get(&event)
                .and_then(Value::as_array)
                .and_then(|groups| groups.get(group_index))
                .and_then(Value::as_object)
                .ok_or(JsonError::EvidenceChanged)?;
            let action_count = group_value
                .get("hooks")
                .and_then(Value::as_array)
                .map(Vec::len)
                .ok_or(JsonError::EvidenceChanged)?;
            if action_indexes.iter().any(|index| *index >= action_count) {
                return Err(JsonError::EvidenceChanged);
            }
            if action_indexes.len() == action_count {
                groups_to_remove.push(group_node);
            } else {
                remove_event = false;
                let group_object = group_node.as_object().ok_or(JsonError::EvidenceChanged)?;
                let action_array = group_object
                    .array_value("hooks")
                    .ok_or(JsonError::EvidenceChanged)?;
                let actions = action_array.elements();
                for index in action_indexes {
                    actions
                        .get(index)
                        .cloned()
                        .ok_or(JsonError::EvidenceChanged)?
                        .remove();
                }
            }
        }

        if !groups_to_remove.is_empty() {
            for group in groups_to_remove {
                group.remove();
            }
        }
        if remove_event && grouped_event_was_fully_selected(&original_groups, &event_array) {
            event_property.remove();
        }
    }

    Ok(document.root.to_string().into_bytes())
}

fn grouped_event_was_fully_selected(
    original_groups: &[jsonc_parser::cst::CstNode],
    event_array: &jsonc_parser::cst::CstArray,
) -> bool {
    event_array.elements().is_empty() && !original_groups.is_empty()
}

pub(super) fn edit_mcp_config(bytes: &[u8], names: &[String]) -> Result<Vec<u8>, JsonError> {
    let document = parse(bytes)?;
    let servers = document
        .value
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or(JsonError::EvidenceChanged)?;
    if names.iter().any(|name| !servers.contains_key(name)) {
        return Err(JsonError::EvidenceChanged);
    }
    let root = document
        .root
        .value()
        .and_then(|node| node.as_object())
        .ok_or(JsonError::UnsupportedSchema)?;
    let servers_object = root
        .object_value("mcpServers")
        .ok_or(JsonError::EvidenceChanged)?;
    for name in names {
        servers_object
            .get(name)
            .ok_or(JsonError::EvidenceChanged)?
            .remove();
    }
    Ok(document.root.to_string().into_bytes())
}

pub(super) fn edit_manifest(
    bytes: &[u8],
    filenames: &[String],
) -> Result<(Vec<u8>, bool), JsonError> {
    let document = parse(bytes)?;
    let manifest: Manifest =
        serde_json::from_value(document.value.clone()).map_err(|_| JsonError::UnsupportedSchema)?;
    if manifest.version != 1
        || filenames
            .iter()
            .any(|name| !manifest.files.contains_key(name))
    {
        return Err(JsonError::EvidenceChanged);
    }
    let root = document
        .root
        .value()
        .and_then(|node| node.as_object())
        .ok_or(JsonError::UnsupportedSchema)?;
    let files = root
        .object_value("files")
        .ok_or(JsonError::UnsupportedSchema)?;
    for filename in filenames {
        files
            .get(filename)
            .ok_or(JsonError::EvidenceChanged)?
            .remove();
    }
    let empty = files.properties().is_empty();
    Ok((document.root.to_string().into_bytes(), empty))
}

pub(super) fn edit_ledger(
    bytes: &[u8],
    config_path: &str,
    names: &[String],
) -> Result<(Vec<u8>, bool), JsonError> {
    let document = parse(bytes)?;
    let ledger: Ledger =
        serde_json::from_value(document.value.clone()).map_err(|_| JsonError::UnsupportedSchema)?;
    if ledger.version != 1 {
        return Err(JsonError::UnsupportedSchema);
    }
    let tracked = ledger
        .files
        .get(config_path)
        .ok_or(JsonError::EvidenceChanged)?;
    if names.iter().any(|name| !tracked.contains_key(name)) {
        return Err(JsonError::EvidenceChanged);
    }

    let root = document
        .root
        .value()
        .and_then(|node| node.as_object())
        .ok_or(JsonError::UnsupportedSchema)?;
    let files = root
        .object_value("files")
        .ok_or(JsonError::UnsupportedSchema)?;
    let config = files
        .object_value(config_path)
        .ok_or(JsonError::EvidenceChanged)?;
    for name in names {
        config.get(name).ok_or(JsonError::EvidenceChanged)?.remove();
    }
    if config.properties().is_empty() {
        files
            .get(config_path)
            .ok_or(JsonError::EvidenceChanged)?
            .remove();
    }
    let empty = files.properties().is_empty();
    Ok((document.root.to_string().into_bytes(), empty))
}

fn parse_hook_locator(locator: &str) -> Result<(String, usize, usize), JsonError> {
    let parts = locator.split('/').skip(1).collect::<Vec<_>>();
    if parts.len() != 5 || parts[0] != "hooks" || parts[3] != "hooks" {
        return Err(JsonError::EvidenceChanged);
    }
    let event = unescape_pointer(parts[1])?;
    let group = parse_index(parts[2])?;
    let action = parse_index(parts[4])?;
    Ok((event, group, action))
}

pub(super) fn parse_mcp_locator(locator: &str) -> Result<String, JsonError> {
    let Some(name) = locator.strip_prefix("/mcpServers/") else {
        return Err(JsonError::EvidenceChanged);
    };
    unescape_pointer(name)
}

fn parse_index(value: &str) -> Result<usize, JsonError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(JsonError::EvidenceChanged);
    }
    value.parse().map_err(|_| JsonError::EvidenceChanged)
}

fn unescape_pointer(value: &str) -> Result<String, JsonError> {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '~' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('0') => output.push('~'),
            Some('1') => output.push('/'),
            _ => return Err(JsonError::EvidenceChanged),
        }
    }
    Ok(output)
}

fn reject_duplicate_keys(node: &jsonc_parser::cst::CstNode) -> Result<(), JsonError> {
    if let Some(object) = node.as_object() {
        let mut names = BTreeSet::new();
        for property in object.properties() {
            let name = property.decoded_name().ok_or(JsonError::Invalid)?;
            if !names.insert(name) {
                return Err(JsonError::DuplicateKeys);
            }
            if let Some(value) = property.value() {
                reject_duplicate_keys(&value)?;
            }
        }
    } else if let Some(array) = node.as_array() {
        for element in array.elements() {
            reject_duplicate_keys(&element)?;
        }
    }
    Ok(())
}
