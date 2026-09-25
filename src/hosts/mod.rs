use std::collections::BTreeSet;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};
use thiserror::Error;

const MAX_INPUT_BYTES: u64 = 1024 * 1024;

const CLAUDE_HOOKS: &str = "claude.hooks.user";
const CODEX_HOOKS: &str = "codex.hooks.user";
const CLAUDE_MCP: &str = "claude.mcp.user";
const SHARED_SKILLS: &str = "shared.skills.user";
const CLAUDE_SKILLS: &str = "claude.skills.user";
const CODEX_SKILLS: &str = "codex.skills.user";

#[derive(Debug)]
pub struct HostInventory {
    pub artifacts: Vec<HostArtifact>,
    pub skill_directories: Vec<SkillDirectory>,
    pub diagnostics: Vec<HostDiagnostic>,
}

/// An immediate child directory of one of the known user skill roots.
/// This is discovery metadata only; it is not a deletable host artifact.
#[derive(Debug)]
pub struct SkillDirectory {
    pub surface: String,
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct HostArtifact {
    pub surface: String,
    pub path: PathBuf,
    pub locator: String,
    pub value: Value,
}

#[derive(Debug)]
pub struct HostDiagnostic {
    pub surface: String,
    pub path: PathBuf,
    pub code: String,
    pub message: String,
}

/// Discovers read-only host artifacts from the requested default user surfaces.
pub fn discover(home: &Path, surfaces: &[String]) -> HostInventory {
    let surfaces: BTreeSet<&str> = surfaces.iter().map(String::as_str).collect();
    let mut inventory = HostInventory {
        artifacts: Vec::new(),
        skill_directories: Vec::new(),
        diagnostics: Vec::new(),
    };

    let absolute_home = match absolute_path(home) {
        Ok(path) => path,
        Err(()) => {
            for surface in &surfaces {
                if surface_path(surface).is_some() {
                    push_diagnostic(
                        &mut inventory,
                        surface,
                        home,
                        "read_error",
                        "could not resolve the requested home directory",
                    );
                } else {
                    push_diagnostic(
                        &mut inventory,
                        surface,
                        Path::new(""),
                        "unknown_surface",
                        "surface is not supported",
                    );
                }
            }
            return inventory;
        }
    };

    for surface in surfaces {
        match surface_path(surface) {
            Some(SurfacePath::Hooks(relative)) => {
                discover_hooks(&absolute_home, surface, relative, &mut inventory);
            }
            Some(SurfacePath::Mcp(relative)) => {
                discover_mcp(&absolute_home, surface, relative, &mut inventory);
            }
            Some(SurfacePath::Skills(relative)) => {
                discover_skills(&absolute_home, surface, relative, &mut inventory);
            }
            None => push_diagnostic(
                &mut inventory,
                surface,
                Path::new(""),
                "unknown_surface",
                "surface is not supported",
            ),
        }
    }

    inventory.artifacts.sort_by(|left, right| {
        (&left.surface, &left.path, &left.locator).cmp(&(
            &right.surface,
            &right.path,
            &right.locator,
        ))
    });
    inventory
        .skill_directories
        .sort_by(|left, right| (&left.surface, &left.path).cmp(&(&right.surface, &right.path)));
    inventory.diagnostics.sort_by(|left, right| {
        (&left.surface, &left.path, &left.code, &left.message).cmp(&(
            &right.surface,
            &right.path,
            &right.code,
            &right.message,
        ))
    });
    inventory
}

#[derive(Clone, Copy)]
enum SurfacePath {
    Hooks(&'static Path),
    Mcp(&'static Path),
    Skills(&'static Path),
}

fn surface_path(surface: &str) -> Option<SurfacePath> {
    match surface {
        CLAUDE_HOOKS => Some(SurfacePath::Hooks(Path::new(".claude/settings.json"))),
        CODEX_HOOKS => Some(SurfacePath::Hooks(Path::new(".codex/hooks.json"))),
        CLAUDE_MCP => Some(SurfacePath::Mcp(Path::new(".claude.json"))),
        SHARED_SKILLS => Some(SurfacePath::Skills(Path::new(".agents/skills"))),
        CLAUDE_SKILLS => Some(SurfacePath::Skills(Path::new(".claude/skills"))),
        CODEX_SKILLS => Some(SurfacePath::Skills(Path::new(".codex/skills"))),
        _ => None,
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, ()> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .map_err(|_| ())
    }
}

#[derive(Debug, Error)]
enum PathInspectionError {
    #[error("symbolic link found")]
    Symlink,
    #[error("non-directory path component found")]
    NotDirectory,
    #[error("invalid relative path")]
    InvalidRelativePath,
    #[error("filesystem inspection failed")]
    Io(#[source] io::Error),
}

#[derive(Debug, Error)]
enum ConfigReadError {
    #[error(transparent)]
    Path(#[from] PathInspectionError),
    #[error("host configuration is not a regular file")]
    NotRegularFile,
    #[error("host configuration exceeds the size limit")]
    TooLarge,
    #[error("host configuration could not be read")]
    Read,
    #[error("host configuration is not valid JSON")]
    InvalidJson,
    #[error("host configuration must be a JSON object")]
    InvalidRoot,
}

fn inspect_path(home: &Path, relative: &Path) -> Result<Option<Metadata>, PathInspectionError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PathInspectionError::InvalidRelativePath);
    }

    let components: Vec<_> = relative.components().collect();
    let mut current = home.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(PathInspectionError::Io(error)),
        };

        if metadata.file_type().is_symlink() {
            return Err(PathInspectionError::Symlink);
        }
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(PathInspectionError::NotDirectory);
        }
        if index + 1 == components.len() {
            return Ok(Some(metadata));
        }
    }

    Ok(None)
}

fn read_json_config(home: &Path, relative: &Path) -> Result<Option<Value>, ConfigReadError> {
    let Some(metadata) = inspect_path(home, relative)? else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Err(ConfigReadError::NotRegularFile);
    }
    let bytes = match read_bounded(&home.join(relative), metadata.len()) {
        Ok(bytes) => bytes,
        Err(ReadError::TooLarge) => return Err(ConfigReadError::TooLarge),
        Err(ReadError::Io) => return Err(ConfigReadError::Read),
    };
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| ConfigReadError::InvalidJson)?;
    if !document.is_object() {
        return Err(ConfigReadError::InvalidRoot);
    }
    Ok(Some(document))
}

fn report_config_error(
    inventory: &mut HostInventory,
    surface: &str,
    path: &Path,
    error: ConfigReadError,
) {
    match error {
        ConfigReadError::Path(error) => report_path_error(inventory, surface, path, error),
        ConfigReadError::NotRegularFile => push_diagnostic(
            inventory,
            surface,
            path,
            "invalid_schema",
            "host configuration is not a regular file",
        ),
        ConfigReadError::TooLarge => push_diagnostic(
            inventory,
            surface,
            path,
            "file_too_large",
            "host configuration exceeds the size limit",
        ),
        ConfigReadError::Read => push_diagnostic(
            inventory,
            surface,
            path,
            "read_error",
            "could not read host configuration",
        ),
        ConfigReadError::InvalidJson => push_diagnostic(
            inventory,
            surface,
            path,
            "invalid_json",
            "host configuration is not valid JSON",
        ),
        ConfigReadError::InvalidRoot => push_diagnostic(
            inventory,
            surface,
            path,
            "invalid_schema",
            "host configuration must be a JSON object",
        ),
    }
}

fn discover_hooks(home: &Path, surface: &str, relative: &Path, inventory: &mut HostInventory) {
    let path = home.join(relative);
    let document = match read_json_config(home, relative) {
        Ok(Some(document)) => document,
        Ok(None) => return,
        Err(error) => {
            report_config_error(inventory, surface, &path, error);
            return;
        }
    };
    let root = document
        .as_object()
        .expect("read_json_config validates root");
    let Some(hooks) = root.get("hooks") else {
        return;
    };
    let Some(hooks) = hooks.as_object() else {
        push_diagnostic(
            inventory,
            surface,
            &path,
            "invalid_schema",
            "hooks must be a JSON object",
        );
        return;
    };

    let mut events: Vec<_> = hooks.iter().collect();
    events.sort_by_key(|(key, _)| *key);
    for (event, groups) in events {
        let Some(groups) = groups.as_array() else {
            push_diagnostic(
                inventory,
                surface,
                &path,
                "invalid_schema",
                "hook events must contain arrays of groups",
            );
            continue;
        };
        for (group_index, group) in groups.iter().enumerate() {
            let Some(group) = group.as_object() else {
                push_diagnostic(
                    inventory,
                    surface,
                    &path,
                    "invalid_schema",
                    "hook groups must be JSON objects",
                );
                continue;
            };
            let Some(actions) = group.get("hooks").and_then(Value::as_array) else {
                push_diagnostic(
                    inventory,
                    surface,
                    &path,
                    "invalid_schema",
                    "hook groups must contain an actions array",
                );
                continue;
            };
            for (action_index, action) in actions.iter().enumerate() {
                let Some(action_object) = action.as_object() else {
                    push_diagnostic(
                        inventory,
                        surface,
                        &path,
                        "invalid_schema",
                        "hook actions must be JSON objects",
                    );
                    continue;
                };
                if action_object.get("type").and_then(Value::as_str) != Some("command") {
                    continue;
                }
                if !action_object.get("command").is_some_and(Value::is_string) {
                    push_diagnostic(
                        inventory,
                        surface,
                        &path,
                        "invalid_schema",
                        "command hook actions must contain a string command",
                    );
                    continue;
                }

                inventory.artifacts.push(HostArtifact {
                    surface: surface.to_owned(),
                    path: path.clone(),
                    locator: format!(
                        "/hooks/{}/{group_index}/hooks/{action_index}",
                        escape_pointer_segment(event)
                    ),
                    value: Value::Object(action_object.clone()),
                });
            }
        }
    }
}

fn discover_mcp(home: &Path, surface: &str, relative: &Path, inventory: &mut HostInventory) {
    let path = home.join(relative);
    let document = match read_json_config(home, relative) {
        Ok(Some(document)) => document,
        Ok(None) => return,
        Err(error) => {
            report_config_error(inventory, surface, &path, error);
            return;
        }
    };
    let root = document
        .as_object()
        .expect("read_json_config validates root");
    let Some(servers) = root.get("mcpServers") else {
        return;
    };
    let Some(servers) = servers.as_object() else {
        push_diagnostic(
            inventory,
            surface,
            &path,
            "invalid_schema",
            "mcpServers must be a JSON object",
        );
        return;
    };

    let mut entries: Vec<_> = servers.iter().collect();
    entries.sort_by_key(|(key, _)| *key);
    for (name, server) in entries {
        if !server.is_object() {
            push_diagnostic(
                inventory,
                surface,
                &path,
                "invalid_schema",
                "MCP server entries must be JSON objects",
            );
            continue;
        }
        inventory.artifacts.push(HostArtifact {
            surface: surface.to_owned(),
            path: path.clone(),
            locator: format!("/mcpServers/{}", escape_pointer_segment(name)),
            value: server.clone(),
        });
    }
}

fn discover_skills(home: &Path, surface: &str, relative: &Path, inventory: &mut HostInventory) {
    let root_path = home.join(relative);
    let metadata = match inspect_path(home, relative) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return,
        Err(error) => {
            report_path_error(inventory, surface, &root_path, error);
            return;
        }
    };
    if !metadata.is_dir() {
        push_diagnostic(
            inventory,
            surface,
            &root_path,
            "invalid_schema",
            "skill root is not a directory",
        );
        return;
    }

    let entries = match fs::read_dir(&root_path) {
        Ok(entries) => entries,
        Err(_) => {
            push_diagnostic(
                inventory,
                surface,
                &root_path,
                "read_error",
                "could not read skill directory",
            );
            return;
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => names.push(entry.file_name()),
            Err(_) => push_diagnostic(
                inventory,
                surface,
                &root_path,
                "read_error",
                "could not read skill directory entry",
            ),
        }
    }
    names.sort();

    for name in names {
        let child_relative = relative.join(&name);
        let child_path = home.join(&child_relative);
        let child_metadata = match inspect_path(home, &child_relative) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => continue,
            Err(error) => {
                report_path_error(inventory, surface, &child_path, error);
                continue;
            }
        };
        if !child_metadata.is_dir() {
            continue;
        }

        inventory.skill_directories.push(SkillDirectory {
            surface: surface.to_owned(),
            path: child_path.clone(),
        });

        let skill_relative = child_relative.join("SKILL.md");
        let skill_path = home.join(&skill_relative);
        let metadata = match inspect_path(home, &skill_relative) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => continue,
            Err(error) => {
                report_path_error(inventory, surface, &skill_path, error);
                continue;
            }
        };
        if !metadata.is_file() {
            push_diagnostic(
                inventory,
                surface,
                &skill_path,
                "invalid_schema",
                "SKILL.md is not a regular file",
            );
            continue;
        }
        let bytes = match read_bounded(&skill_path, metadata.len()) {
            Ok(bytes) => bytes,
            Err(ReadError::TooLarge) => {
                push_diagnostic(
                    inventory,
                    surface,
                    &skill_path,
                    "file_too_large",
                    "SKILL.md exceeds the size limit",
                );
                continue;
            }
            Err(ReadError::Io) => {
                push_diagnostic(
                    inventory,
                    surface,
                    &skill_path,
                    "read_error",
                    "could not read SKILL.md",
                );
                continue;
            }
        };
        let content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(_) => {
                push_diagnostic(
                    inventory,
                    surface,
                    &skill_path,
                    "invalid_utf8",
                    "SKILL.md is not valid UTF-8",
                );
                continue;
            }
        };
        let mut value = Map::new();
        value.insert(
            "skill_dir".to_owned(),
            Value::String(child_path.to_string_lossy().into_owned()),
        );
        value.insert("content".to_owned(), Value::String(content));
        inventory.artifacts.push(HostArtifact {
            surface: surface.to_owned(),
            path: skill_path,
            locator: String::new(),
            value: Value::Object(value),
        });
    }
}

#[derive(Debug)]
enum ReadError {
    TooLarge,
    Io,
}

fn read_bounded(path: &Path, initial_size: u64) -> Result<Vec<u8>, ReadError> {
    if initial_size > MAX_INPUT_BYTES {
        return Err(ReadError::TooLarge);
    }
    let file = File::open(path).map_err(|_| ReadError::Io)?;
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadError::Io)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(ReadError::TooLarge);
    }
    Ok(bytes)
}

fn report_path_error(
    inventory: &mut HostInventory,
    surface: &str,
    path: &Path,
    error: PathInspectionError,
) {
    match error {
        PathInspectionError::Symlink => push_diagnostic(
            inventory,
            surface,
            path,
            "skipped_symlink",
            "path contains a symbolic link and was skipped",
        ),
        PathInspectionError::NotDirectory | PathInspectionError::InvalidRelativePath => {
            push_diagnostic(
                inventory,
                surface,
                path,
                "invalid_schema",
                "host path has an unexpected structure",
            );
        }
        PathInspectionError::Io(_) => push_diagnostic(
            inventory,
            surface,
            path,
            "read_error",
            "could not inspect host path",
        ),
    }
}

fn push_diagnostic(
    inventory: &mut HostInventory,
    surface: &str,
    path: &Path,
    code: &str,
    message: &str,
) {
    inventory.diagnostics.push(HostDiagnostic {
        surface: surface.to_owned(),
        path: path.to_path_buf(),
        code: code.to_owned(),
        message: message.to_owned(),
    });
}

fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests;
