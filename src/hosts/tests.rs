use std::fs;
use std::path::Path;

use serde_json::json;
use tempfile::TempDir;

use super::{HostArtifact, HostInventory, discover};

fn surface(name: &str) -> Vec<String> {
    vec![name.to_owned()]
}

fn write(path: &Path, contents: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().expect("parent directory")).expect("create parent directory");
    fs::write(path, contents).expect("write fixture");
}

fn artifact<'a>(inventory: &'a HostInventory, locator: &str) -> &'a HostArtifact {
    inventory
        .artifacts
        .iter()
        .find(|artifact| artifact.locator == locator)
        .expect("artifact with locator")
}

#[test]
fn missing_host_files_are_empty() {
    let home = TempDir::new().expect("temporary home");
    let inventory = discover(
        home.path(),
        &[
            "claude.hooks.user".to_owned(),
            "codex.hooks.user".to_owned(),
            "claude.mcp.user".to_owned(),
            "shared.skills.user".to_owned(),
            "claude.skills.user".to_owned(),
            "codex.skills.user".to_owned(),
        ],
    );
    assert!(inventory.artifacts.is_empty());
    assert!(inventory.diagnostics.is_empty());
}

#[test]
fn hooks_return_innermost_command_actions_with_escaped_pointer() {
    let home = TempDir::new().expect("temporary home");
    write(
        &home.path().join(".claude/settings.json"),
        serde_json::to_vec(&json!({
            "unrelated": { "token": "do-not-return" },
            "hooks": {
                "Stop/~": [{
                    "matcher": "*",
                    "hooks": [
                        { "type": "prompt", "prompt": "ignored" },
                        { "type": "command", "command": "private command", "env": { "KEY": "secret" } }
                    ]
                }]
            }
        }))
        .expect("serialize fixture"),
    );

    let inventory = discover(home.path(), &surface("claude.hooks.user"));
    assert!(inventory.diagnostics.is_empty());
    assert_eq!(inventory.artifacts.len(), 1);
    let action = artifact(&inventory, "/hooks/Stop~1~0/0/hooks/1");
    assert_eq!(action.value["command"], "private command");
    assert!(action.value.get("env").is_some());
    assert!(
        inventory
            .artifacts
            .iter()
            .all(|item| item.value.get("token").is_none())
    );
}

#[test]
fn hooks_ignore_non_command_actions_and_diagnose_bad_commands() {
    let home = TempDir::new().expect("temporary home");
    write(
        &home.path().join(".codex/hooks.json"),
        br#"{"hooks":{"SessionStart":[{"hooks":[{"type":"prompt"},{"type":"command","command":false}]}]}}"#,
    );

    let inventory = discover(home.path(), &surface("codex.hooks.user"));
    assert!(inventory.artifacts.is_empty());
    assert_eq!(inventory.diagnostics.len(), 1);
    assert_eq!(inventory.diagnostics[0].code, "invalid_schema");
    assert!(!inventory.diagnostics[0].message.contains("false"));
}

#[test]
fn invalid_json_and_top_level_schema_are_distinct_and_sanitized() {
    let home = TempDir::new().expect("temporary home");
    let path = home.path().join(".claude/settings.json");
    write(&path, b"private-value: not-json");
    let invalid_json = discover(home.path(), &surface("claude.hooks.user"));
    assert_eq!(invalid_json.diagnostics[0].code, "invalid_json");
    assert!(
        !invalid_json.diagnostics[0]
            .message
            .contains("private-value")
    );

    write(&path, b"[]");
    let invalid_schema = discover(home.path(), &surface("claude.hooks.user"));
    assert_eq!(invalid_schema.diagnostics[0].code, "invalid_schema");
}

#[test]
fn mcp_reads_only_top_level_entries_and_escapes_names() {
    let home = TempDir::new().expect("temporary home");
    write(
        &home.path().join(".claude.json"),
        serde_json::to_vec(&json!({
            "mcpServers": {
                "server/~name": { "command": "private", "args": ["secret"] }
            },
            "projects": {
                "/tmp/project": { "mcpServers": { "nested": { "command": "ignored" } } }
            }
        }))
        .expect("serialize fixture"),
    );

    let inventory = discover(home.path(), &surface("claude.mcp.user"));
    assert!(inventory.diagnostics.is_empty());
    assert_eq!(inventory.artifacts.len(), 1);
    assert_eq!(
        artifact(&inventory, "/mcpServers/server~1~0name").value["command"],
        "private"
    );
}

#[test]
fn skills_read_immediate_child_skill_files_only() {
    let home = TempDir::new().expect("temporary home");
    write(
        &home.path().join(".agents/skills/alpha/SKILL.md"),
        "alpha instructions",
    );
    write(
        &home.path().join(".agents/skills/alpha/nested/SKILL.md"),
        "not enumerated recursively",
    );
    write(
        &home.path().join(".agents/skills/beta/SKILL.md"),
        "beta instructions",
    );
    fs::create_dir_all(home.path().join(".agents/skills/no-skill"))
        .expect("create empty child directory");

    let inventory = discover(home.path(), &surface("shared.skills.user"));
    assert!(inventory.diagnostics.is_empty());
    assert_eq!(inventory.artifacts.len(), 2);
    assert_eq!(inventory.artifacts[0].locator, "");
    assert!(
        inventory
            .artifacts
            .iter()
            .any(|item| item.value["content"] == "alpha instructions")
    );
    assert!(
        inventory
            .artifacts
            .iter()
            .any(|item| item.value["content"] == "beta instructions")
    );
    let skills_root = home.path().join(".agents/skills");
    assert_eq!(inventory.skill_directories.len(), 3);
    assert!(
        inventory
            .skill_directories
            .iter()
            .any(|directory| { directory.path == home.path().join(".agents/skills/no-skill") })
    );
    assert!(inventory.skill_directories.iter().all(|directory| {
        directory.surface == "shared.skills.user"
            && directory.path.parent() == Some(skills_root.as_path())
    }));
    assert!(inventory.artifacts.iter().all(|item| {
        Path::new(item.value["skill_dir"].as_str().expect("skill directory")).is_absolute()
    }));
}

#[test]
fn oversized_configuration_and_skill_files_are_diagnosed() {
    let home = TempDir::new().expect("temporary home");
    let large = vec![b'x'; 1024 * 1024 + 1];
    write(&home.path().join(".claude.json"), &large);
    write(&home.path().join(".codex/skills/large/SKILL.md"), &large);

    let inventory = discover(
        home.path(),
        &["claude.mcp.user".to_owned(), "codex.skills.user".to_owned()],
    );
    assert!(inventory.artifacts.is_empty());
    assert_eq!(inventory.diagnostics.len(), 2);
    assert!(
        inventory
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "file_too_large")
    );
}

#[test]
fn malformed_skill_file_types_and_utf8_are_diagnosed() {
    let home = TempDir::new().expect("temporary home");
    fs::create_dir_all(home.path().join(".claude/skills/directory-file/SKILL.md"))
        .expect("create directory at skill file path");
    write(
        &home.path().join(".claude/skills/invalid-utf8/SKILL.md"),
        [0xff, 0xfe],
    );

    let inventory = discover(home.path(), &surface("claude.skills.user"));
    assert!(inventory.artifacts.is_empty());
    assert_eq!(inventory.diagnostics.len(), 2);
    assert!(
        inventory
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "invalid_utf8")
    );
    assert!(
        inventory
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "invalid_schema")
    );
}

#[cfg(unix)]
#[test]
fn symlinked_configuration_components_and_skill_entries_are_skipped() {
    use std::os::unix::fs::symlink;

    let home = TempDir::new().expect("temporary home");
    let outside = TempDir::new().expect("outside fixture");
    write(
        &outside.path().join("settings.json"),
        br#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"outside"}]}]}}"#,
    );
    fs::create_dir_all(outside.path().join("linked-skill")).expect("create outside skill");
    write(
        &outside.path().join("linked-skill/SKILL.md"),
        "outside skill",
    );

    symlink(
        outside.path().join("settings.json"),
        home.path().join(".claude.json"),
    )
    .expect("symlink config file");
    symlink(outside.path(), home.path().join(".codex"))
        .expect("symlink config directory component");
    fs::create_dir_all(home.path().join(".agents/skills/file-link"))
        .expect("create skill directory");
    symlink(
        outside.path().join("linked-skill/SKILL.md"),
        home.path().join(".agents/skills/file-link/SKILL.md"),
    )
    .expect("symlink skill file");
    symlink(
        outside.path().join("linked-skill"),
        home.path().join(".agents/skills/directory-link"),
    )
    .expect("symlink skill directory");

    let inventory = discover(
        home.path(),
        &[
            "claude.mcp.user".to_owned(),
            "codex.hooks.user".to_owned(),
            "shared.skills.user".to_owned(),
        ],
    );
    assert!(inventory.artifacts.is_empty());
    assert_eq!(inventory.diagnostics.len(), 4);
    assert!(
        inventory
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "skipped_symlink")
    );
}

#[cfg(unix)]
#[test]
fn symlinked_skill_root_component_is_skipped() {
    use std::os::unix::fs::symlink;

    let home = TempDir::new().expect("temporary home");
    let outside = TempDir::new().expect("outside fixture");
    write(&outside.path().join("skill/SKILL.md"), "outside");
    symlink(outside.path(), home.path().join(".agents")).expect("symlink root component");

    let inventory = discover(home.path(), &surface("shared.skills.user"));
    assert!(inventory.artifacts.is_empty());
    assert_eq!(inventory.diagnostics.len(), 1);
    assert_eq!(inventory.diagnostics[0].code, "skipped_symlink");
}

#[test]
fn unknown_surfaces_are_reported_without_touching_home() {
    let home = TempDir::new().expect("temporary home");
    let inventory = discover(home.path(), &surface("claude.skills.project"));
    assert!(inventory.artifacts.is_empty());
    assert_eq!(inventory.diagnostics.len(), 1);
    assert_eq!(inventory.diagnostics[0].code, "unknown_surface");
}
