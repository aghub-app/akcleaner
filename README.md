# AKCleaner

AKCleaner finds agent integrations left by applications and lets you clean them by application. Cleanup backs up every planned file before changing selected hooks, MCP entries, skill marker documents, and unchanged manifest-listed files. Modified, conflicting, or unverifiable findings remain available and are not removed. AKCleaner does not uninstall applications or inspect project directories, alternate profiles, or environment overrides.

The filesystem implementation supports macOS and Linux (Unix). Windows is not supported.

## Commands

```sh
cargo run -- clean --home /path/to/synthetic-home
cargo run -- scan --home /path/to/synthetic-home --json
cargo run -- rules check --rules-dir ./rules
```

Running `akcleaner` without a subcommand opens the same interactive flow as `akcleaner clean`:

1. **选择 App** — choose one application with the arrow keys or type to search, then press Enter. Only applications with removable findings appear, with at most eight visible rows.
2. **删除 [App] 的全部集成？** — choose **删除** or **不删**. The default is **不删**, which exits. Choosing **删除** immediately backs up and removes all currently removable findings for that application.

Selecting an application includes all its currently removable findings; modified or unverified content is retained. Clean requires terminal stdin, stdout, and stderr, and has no `--yes` shortcut.

The terminal UI uses Clack-style prompts through `cliclack`: a left-hand guide connects the application picker, a compact delete/don't-delete question, and a short success message. With nothing to remove, it displays **现在没有任何东西可以卸载。** and exits. It does not show counts, file paths, previews, internal states, errors, or diagnostics. Unsuccessful operations still return a nonzero exit status.

Before confirmation, planning is read-only and no backup is created. Execution stores private backups in `home/.local/state/akcleaner/backups/<id>`, including a path map with original modes and SHA-256 hashes and an operation receipt without command or configuration values. A partial result or failed operation returns a nonzero status. Escape and Ctrl-C cancel interactive prompts without executing cleanup.

`scan` and `clean` use the three bundled rulesets when `--rules-dir` is omitted. An explicit `--rules-dir` loads that directory and reports errors normally. `rules check` defaults to `./rules` and accepts an explicit directory. Tests and examples should always pass a synthetic `--home`.

Text output escapes terminal control characters in paths and keys. `NO_COLOR` disables terminal styling. Reports omit raw commands and configuration values.

## Scan behavior

The host adapter reads `claude.hooks.user`, `codex.hooks.user`, `claude.mcp.user`, `shared.skills.user`, `claude.skills.user`, and `codex.skills.user`. It reads only the default user roots under the supplied home, skips symlinks, and reports malformed host files as diagnostics. The report separates findings from diagnostics. A complete scan with no findings exits successfully; diagnostics make it incomplete and return a nonzero exit status.

- Command markers are candidates with unknown integrity.
- A skill marker confirms only the `SKILL.md` document and has unknown integrity.
- Paseo manifests are read from immediate child directories of the known skill roots, whether or not `SKILL.md` exists; listed files are checked independently against SHA-256. Unlisted files are outside the product claim.
- Superset MCP ledger entries apply only to the current scan home's `.claude.json`. Hash comparison is supported only for the verified canonicalization subset; other shapes remain unknown and are never reported as modified from an unverified hash.
- Coverage includes only the default user roots for the supplied home. Project directories, profile roots, and environment overrides are not scanned.

## Checks

```sh
cargo fmt -- --check
cargo check --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --bin akcleaner
python3 tests/interactive_pty.py
```
