# Synthetic fixture guide

These are read-only scan fixtures. `before/` is a synthetic home tree; `expected.json` states expected discoveries, non-discoveries, and diagnostics. A finding is not a proposed deletion or an after-state. Paths in expectations use `HOME/` as a placeholder for the temporary fixture root.

The cases are deliberately small and independent. The shared hook case checks both product candidates in one hook group beside a user action. The pointer case checks RFC 6901 escaping without claiming the synthetic user action for a product. Paseo cases exercise its per-skill manifest separately. Superset cases exercise the SKILL.md marker and Claude MCP ledger separately.

## Materializing and checking

Copy a case's `before/home/` tree into a newly created temporary directory and pass that directory as the scan home. For `superset/fixtures/mcp-ledger-mismatch`, replace the literal `/synthetic/home` ledger key with the absolute path to the temporary home before scanning; it is a fixture token, not a machine path. Normalize that root back to `HOME/` when comparing paths to `expected.json`.

Check each listed finding's product/rule, surface, normalized path, locator, attribution, and integrity. Check each `not_find` entry is absent; for the pointer case, check the host artifact locator exactly. Check diagnostics by code. Current expected hashes refer to the exact UTF-8 bytes shown in `before/`.

There is no fixture runner in this Agent A delivery. Agent C can consume these trees in tests without changing the shared rule schema. Every file and value here is synthetic and contains no real user configuration, path, credential, or secret.
