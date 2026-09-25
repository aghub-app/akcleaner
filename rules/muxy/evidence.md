# Muxy rule evidence

Verified against the fixed source snapshot at commit `ab008599954afe683a48410b98b8f483adb96b40`. The local research checkout's `HEAD` matches this SHA.

## Notification hooks

- [ClaudeCodeProvider.swift, lines 25–27](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/ClaudeCodeProvider.swift#L25-L27) defines the user's `~/.claude/settings.json` path and the literal `muxy-notification-hook` marker. Lines [41–64](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/ClaudeCodeProvider.swift#L41-L64) show hook event entries being merged into its `hooks` object.
- [ClaudeCodeProvider.swift, lines 167–181](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/ClaudeCodeProvider.swift#L167-L181) builds command actions with that marker.
- [CodexProvider.swift, lines 25–47](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/CodexProvider.swift#L25-L47) defines the same marker and defaults the Codex hook file to `<home>/.codex/hooks.json`. Lines [101–140](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/CodexProvider.swift#L101-L140) merge command actions into the hooks object; lines [163–177](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/CodexProvider.swift#L163-L177) construct commands ending in the marker.
- Claude's removal predicate uses marker substring matching at [ClaudeCodeProvider.swift, lines 199–216](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/ClaudeCodeProvider.swift#L199-L216). This ruleset therefore uses the shared protocol's `command_marker`, whose result is only a **candidate**. It does not infer ownership from a hook name or command path.

The mixed-hook fixture proves that the Muxy marker can coexist inside a hook group with a Superset candidate and an unmarked user action. The Pi extension and legacy settings registration are not represented: no allowed first-round surface or evidence type covers them.
