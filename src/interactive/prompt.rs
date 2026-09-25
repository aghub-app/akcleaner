use std::io;

use anyhow::Result;

pub(super) trait PromptDriver {
    fn application(&mut self, names: &[String]) -> Result<Option<usize>>;
    fn confirm(&mut self, name: &str) -> Result<bool>;
}

pub(super) struct TerminalPrompts;

impl PromptDriver for TerminalPrompts {
    fn application(&mut self, names: &[String]) -> Result<Option<usize>> {
        let mut prompt = cliclack::select("选择 App（输入搜索）")
            .filter_mode()
            .max_rows(page_size());
        for (index, name) in names.iter().enumerate() {
            prompt = prompt.item(index, name, "");
        }
        cancelled(prompt.interact())
    }

    fn confirm(&mut self, name: &str) -> Result<bool> {
        let answer = cancelled(
            cliclack::confirm(format!("删除 {name} 的全部集成？"))
                .initial_value(false)
                .interact(),
        )?;
        if answer == Some(false) {
            cliclack::outro("")?;
        }
        Ok(answer.unwrap_or(false))
    }
}

fn cancelled<T>(result: io::Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn page_size() -> usize {
    usize::from(console::Term::stderr().size().0.saturating_sub(8)).clamp(4, 8)
}
