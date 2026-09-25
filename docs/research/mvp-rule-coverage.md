# MVP product rule coverage

Rules are limited to the three product snapshots and evidence forms reviewed for Agent A. Product names and directory names are never used alone as attribution evidence. This is source-backed coverage for a read-only scan, not a complete installation inventory or a cleanup plan.

## Rule sets

| Product | Rule | Surface(s) | Evidence and expected attribution |
| --- | --- | --- | --- |
| Muxy | `notification-hooks` | Claude hooks, Codex hooks | `muxy-notification-hook` command substring; candidate, integrity unknown |
| Paseo | `managed-skill-files` | shared, Claude, and Codex skills | `.paseo-managed-files.json` v1 per-file SHA-256; matching files confirmed/unchanged, changed files confirmed/modified |
| Superset | `notification-hooks` | Claude hooks, Codex hooks | Current `$SUPERSET_HOME_DIR/hooks/notify.sh` command form; candidate, integrity unknown |
| Superset | `managed-skill-document` | shared skills | Exact marker in `SKILL.md`; confirmed for that document only, integrity unknown |
| Superset | `claude-mcp-ledger` | Claude MCP | `.superset/plugins/mcp-ledger.json` v1, scoped to the current home `.claude.json`; tracked entries confirmed historically, with current hash state shown separately |

The fixed source commits, cited files/lines, and the evidence boundary for each rule are in the adjacent product `evidence.md` files. The local research copies were checked against the task-book path and each checkout's `HEAD` matched its fixed commit.

## Synthetic fixture coverage

All paths and values are synthetic. Fixture paths use `HOME/` in expected results; see [the fixture guide](../../rules/fixtures/README.md) for temporary-home materialization and the absolute ledger-key substitution.

| Required case | Fixture | Expected result |
| --- | --- | --- |
| Two products and a user hook in one group | `rules/fixtures/hooks-mixed` | Muxy and Superset candidates at distinct JSON Pointers; user action is not found |
| Same-named skill with no marker | `rules/superset/fixtures/skill-marker` | `superset/SKILL.md` is not found by name; a different marked document is confirmed; extra file is not claimed |
| Modified manifest-listed file | `rules/paseo/fixtures/manifest-modified` | Listed `SKILL.md` is confirmed/modified; extra unlisted file is not claimed |
| Manifest parent traversal | `rules/paseo/fixtures/manifest-traversal` | Valid listed file remains confirmed/unchanged; `../outside.txt` produces an invalid-path diagnostic and is not read or found |
| Same-name MCP entry with ledger hash mismatch | `rules/superset/fixtures/mcp-ledger-mismatch` | Tracked `linear` is confirmed historical management/modified; untracked `notion` is not found |
| JSON Pointer key containing `/` and `~` | `rules/fixtures/json-pointer-special` | Artifact locator is `/hooks/Before~1Tool~0Use/0/hooks/0`; the user artifact is not attributed |
| Clean Paseo manifest | `rules/paseo/fixtures/manifest-unchanged` | Each matching listed file is confirmed/unchanged; extra unlisted note is not claimed |

The Paseo manifest cases and Superset MCP ledger case each use their own minimal manifest/ledger sample. Hook command markers intentionally produce candidates: they are not exact parser-based ownership proofs. The Superset skill case shows that its marker applies to one `SKILL.md`, not the full directory.

## Not covered

- Products other than Muxy, Paseo, and Superset.
- Runtime-only injection and whether an integration is active in a running app or agent session.
- Product settings that stop reinjection or reinstall; scanning and removing persisted artifacts does not change app intent.
- Codex MCP TOML, OpenCode plugins, Muxy's Pi extension/legacy registration, Superset's Claude plugin sentinel layout, and other source-supported but protocol-incompatible surfaces.
- Project roots, non-default profiles, secondary accounts, and environment-selected `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, or `SUPERSET_HOME_DIR` overrides. The shared first-round scanner covers the default user roots or an explicitly supplied test home only.
- Superset absolute/stale notification-hook command forms. Its source predicate handles them, but this ruleset's literal marker only describes the current guarded command form.
- Apply, deletion, quarantine, restore, reinjection prevention, and proof that the remaining app configuration is inactive. This round's fixtures specify read-only findings only.
