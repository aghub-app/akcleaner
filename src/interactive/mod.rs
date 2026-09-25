mod entries;
mod presentation;
mod prompt;

use std::{path::Path, process::ExitCode};

use anyhow::Result;

use self::{
    entries::applications,
    presentation::init_theme,
    prompt::{PromptDriver, TerminalPrompts},
};
use crate::{cleanup, discovery::ScanReport, rules::RuleSet};

pub(crate) fn run_clean(home: &Path, rules: &[RuleSet], report: &ScanReport) -> Result<ExitCode> {
    init_theme();
    run_clean_with_prompts(home, rules, report, &mut TerminalPrompts)
}

fn run_clean_with_prompts(
    home: &Path,
    rules: &[RuleSet],
    report: &ScanReport,
    prompts: &mut impl PromptDriver,
) -> Result<ExitCode> {
    let apps = applications(rules, report);
    let available: Vec<_> = apps
        .iter()
        .filter(|app| !app.removable.is_empty())
        .collect();

    cliclack::intro("AKCleaner")?;

    if available.is_empty() {
        cliclack::outro("现在没有任何东西可以卸载。")?;
        if report.is_complete() {
            return Ok(ExitCode::SUCCESS);
        }
        return Ok(ExitCode::FAILURE);
    }

    let names = available
        .iter()
        .map(|app| app.name.clone())
        .collect::<Vec<_>>();
    let Some(index) = prompts.application(&names)? else {
        return Ok(ExitCode::SUCCESS);
    };
    let Some(app) = available.get(index) else {
        return Ok(ExitCode::FAILURE);
    };
    if !prompts.confirm(&app.name)? {
        return Ok(ExitCode::SUCCESS);
    }

    let selected = app
        .removable
        .iter()
        .map(|&index| report.findings[index].clone())
        .collect::<Vec<_>>();
    let plan = cleanup::prepare(home, rules, &selected)?;
    if plan.changes.is_empty() {
        return Ok(ExitCode::FAILURE);
    }
    let skipped = plan.skipped.len();
    let result = cleanup::execute(plan)?;
    if result.failures.is_empty() && skipped == 0 && report.is_complete() {
        cliclack::outro("已删除。")?;
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

#[cfg(test)]
mod tests;
