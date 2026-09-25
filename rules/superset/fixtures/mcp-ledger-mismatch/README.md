# Superset MCP ledger mismatch

The ledger tracks `linear` at the synthetic absolute config path, but its hash is for an earlier value. The current `linear` entry is expected as confirmed historical management with modified integrity. `notion` has the same MCP map location and a plausible server name but is absent from the ledger, so it must not be found. Before scanning, materialize `/synthetic/home` in the ledger key as the temporary fixture home as described in `rules/fixtures/README.md`.
