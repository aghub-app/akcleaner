use std::{fs, path::PathBuf, process::ExitCode};

use serde_json::json;
use tempfile::tempdir;

use super::{entries::applications, prompt::PromptDriver, run_clean_with_prompts};
use crate::{discovery, rules::load_bundled_rules};

#[derive(Default)]
struct Prompts {
    selection: Option<usize>,
    delete: bool,
    app_labels: Vec<String>,
    app_calls: usize,
    review_calls: usize,
    confirmed_app: Option<String>,
    untouched: Option<(PathBuf, Vec<u8>)>,
}

impl Prompts {
    fn assert_no_writes(&self) {
        if let Some((home, original)) = &self.untouched {
            assert_eq!(
                fs::read(home.join(".claude/settings.json")).unwrap(),
                *original
            );
            assert!(!home.join(".local/state/akcleaner/backups").exists());
        }
    }
}

impl PromptDriver for Prompts {
    fn application(&mut self, names: &[String]) -> anyhow::Result<Option<usize>> {
        self.assert_no_writes();
        self.app_labels = names.to_vec();
        self.app_calls += 1;
        Ok(self.selection.take())
    }
    fn confirm(&mut self, name: &str) -> anyhow::Result<bool> {
        self.assert_no_writes();
        self.review_calls += 1;
        self.confirmed_app = Some(name.to_owned());
        Ok(self.delete)
    }
}

fn fixture(home: &std::path::Path) -> Vec<u8> {
    let config = home.join(".claude/settings.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let action = |command: &str| json!({"type":"command", "command":command});
    let content = serde_json::to_vec(&json!({"hooks": {
        "Stop": [{"hooks": [action("run muxy-notification-hook --private-token=x"), action("run $SUPERSET_HOME_DIR/hooks/notify.sh"), action("user-hook")]}],
        "SessionStart": [{"hooks": [action("run muxy-notification-hook")]}]
    }})).unwrap();
    fs::write(config, &content).unwrap();
    content
}

#[test]
fn application_selection_expands_to_all_its_artifacts_without_touching_other_apps() {
    let temp = tempdir().unwrap();
    let original = fixture(temp.path());
    let rules = load_bundled_rules().unwrap();
    let report = discovery::scan(temp.path(), &rules);
    let mut prompts = Prompts {
        selection: Some(0),
        delete: true,
        untouched: Some((temp.path().to_owned(), original.clone())),
        ..Default::default()
    };
    assert_eq!(
        run_clean_with_prompts(temp.path(), &rules, &report, &mut prompts).unwrap(),
        ExitCode::SUCCESS
    );
    assert_eq!(prompts.app_labels, ["Muxy", "Superset"]);
    assert_eq!(prompts.app_calls, 1);
    assert_eq!(prompts.review_calls, 1);
    assert_eq!(prompts.confirmed_app.as_deref(), Some("Muxy"));
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(config["hooks"].get("SessionStart").is_none());
    let actions = config["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(
        actions[0]["command"],
        "run $SUPERSET_HOME_DIR/hooks/notify.sh"
    );
    assert_eq!(actions[1]["command"], "user-hook");
    let backups = fs::read_dir(temp.path().join(".local/state/akcleaner/backups"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read(backups[0].path().join("files/.claude/settings.json")).unwrap(),
        original
    );
}

#[test]
fn choosing_another_app_only_deletes_that_apps_integrations() {
    let temp = tempdir().unwrap();
    let original = fixture(temp.path());
    let rules = load_bundled_rules().unwrap();
    let report = discovery::scan(temp.path(), &rules);
    let mut prompts = Prompts {
        selection: Some(1),
        delete: true,
        untouched: Some((temp.path().to_owned(), original)),
        ..Default::default()
    };
    assert_eq!(
        run_clean_with_prompts(temp.path(), &rules, &report, &mut prompts).unwrap(),
        ExitCode::SUCCESS
    );
    assert_eq!(prompts.app_calls, 1);
    assert_eq!(prompts.app_labels, ["Muxy", "Superset"]);
    assert_eq!(prompts.review_calls, 1);
    assert_eq!(prompts.confirmed_app.as_deref(), Some("Superset"));
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        config["hooks"]["Stop"][0]["hooks"],
        json!([
            {"type":"command", "command":"run muxy-notification-hook --private-token=x"},
            {"type":"command", "command":"user-hook"}
        ])
    );
}

#[test]
fn cancelling_or_choosing_not_to_delete_never_writes_or_reopens_the_picker() {
    for mut prompts in [
        Prompts::default(),
        Prompts {
            selection: Some(0),
            delete: false,
            ..Default::default()
        },
    ] {
        let temp = tempdir().unwrap();
        let original = fixture(temp.path());
        prompts.untouched = Some((temp.path().to_owned(), original));
        let rules = load_bundled_rules().unwrap();
        let report = discovery::scan(temp.path(), &rules);
        assert_eq!(
            run_clean_with_prompts(temp.path(), &rules, &report, &mut prompts).unwrap(),
            ExitCode::SUCCESS
        );
        prompts.assert_no_writes();
        assert_eq!(prompts.app_calls, 1);
    }
}

#[test]
fn hundreds_of_artifacts_still_produce_one_choice_per_present_application() {
    let temp = tempdir().unwrap();
    fixture(temp.path());
    let rules = load_bundled_rules().unwrap();
    let mut report = discovery::scan(temp.path(), &rules);
    let example = report
        .findings
        .iter()
        .find(|finding| finding.product_id == "muxy")
        .unwrap()
        .clone();
    report.findings.extend((0..500).map(|i| {
        let mut item = example.clone();
        item.locator = format!("/hooks/Stop/{i}/hooks/0");
        item
    }));
    let apps = applications(&rules, &report);
    assert_eq!(
        apps.iter().map(|app| app.name.as_str()).collect::<Vec<_>>(),
        ["Muxy", "Superset"]
    );
    assert_eq!(apps[0].removable.len(), 502);
    assert!(!apps.iter().any(|app| app.name == "Paseo"));
}

#[test]
fn cleanup_keeps_modified_skill_files() {
    let temp = tempdir().unwrap();
    fixture(temp.path());
    let skill = temp.path().join(".agents/skills/paseo/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, "user edit").unwrap();
    fs::write(
        skill.with_file_name(".paseo-managed-files.json"),
        format!(
            r#"{{"version":1,"files":{{"SKILL.md":"{}"}}}}"#,
            "0".repeat(64)
        ),
    )
    .unwrap();
    let rules = load_bundled_rules().unwrap();
    let report = discovery::scan(temp.path(), &rules);
    let mut prompts = Prompts {
        selection: Some(0),
        delete: true,
        ..Default::default()
    };
    run_clean_with_prompts(temp.path(), &rules, &report, &mut prompts).unwrap();
    assert_eq!(fs::read_to_string(skill).unwrap(), "user edit");
}

#[test]
fn empty_scan_never_opens_a_menu_and_failures_remain_nonzero() {
    let temp = tempdir().unwrap();
    let rules = load_bundled_rules().unwrap();
    let report = discovery::scan(temp.path(), &rules);
    let mut prompts = Prompts::default();
    assert_eq!(
        run_clean_with_prompts(temp.path(), &rules, &report, &mut prompts).unwrap(),
        ExitCode::SUCCESS
    );
    assert_eq!(prompts.app_calls + prompts.review_calls, 0);
    let report = discovery::scan(&temp.path().join("missing"), &rules);
    assert_eq!(
        run_clean_with_prompts(temp.path(), &rules, &report, &mut prompts).unwrap(),
        ExitCode::FAILURE
    );
    assert_eq!(prompts.app_calls + prompts.review_calls, 0);
}
