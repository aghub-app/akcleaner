use std::io::IsTerminal;

use cliclack::{Theme, ThemeState};

pub(super) fn init_theme() {
    let color = std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal();
    console::set_colors_enabled(color);
    console::set_colors_enabled_stderr(color);
    cliclack::set_theme(ChineseTheme);
}

struct ChineseTheme;

impl Theme for ChineseTheme {
    fn format_confirm(&self, state: &ThemeState, confirm: bool) -> String {
        let yes = self.radio_item(state, confirm, "删除", "");
        let no = self.radio_item(state, !confirm, "不删", "");
        let separator = if matches!(state, ThemeState::Active) {
            " / "
        } else {
            ""
        };
        format!(
            "{}  {yes}{separator}{no}\n",
            self.bar_color(state).apply_to("│")
        )
    }

    fn format_footer_with_message(&self, state: &ThemeState, message: &str) -> String {
        let color = self.bar_color(state);
        match state {
            ThemeState::Active => format!(
                "{}  {}{}\n",
                color.apply_to("└"),
                console::style("↑↓ 选择 · Enter 确认").dim(),
                if message.is_empty() {
                    String::new()
                } else {
                    format!(" · {message}")
                },
            ),
            ThemeState::Cancel => format!("{}  已取消\n", color.apply_to("└")),
            _ => format!("{}\n", color.apply_to("│")),
        }
    }
}

pub(super) fn safe_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                format!("\\u{{{:x}}}", u32::from(character))
            } else {
                character.to_string()
            }
        })
        .collect()
}
