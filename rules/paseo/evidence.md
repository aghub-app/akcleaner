# Paseo rule evidence

Verified against the fixed source snapshot at commit `8cd989529e2d86bb6d1c8a775bf695fa5970c997`. The local research checkout's `HEAD` matches this SHA.

## Manifest-managed skill files

- [paths.ts, lines 17–23](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/server/orchestration-skills/internal/paths.ts#L17-L23) maps the user's home to `.agents/skills`, `.claude/skills`, and `.codex/skills` roots.
- [sync.ts, lines 19–24](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/server/orchestration-skills/internal/sync.ts#L19-L24) defines `.paseo-managed-files.json` with `version: 1` and a relative-path-to-hash `files` map. Lines [54–76](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/server/orchestration-skills/internal/sync.ts#L54-L76) read that manifest and compute SHA-256 over file bytes; lines [79–90](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/server/orchestration-skills/internal/sync.ts#L79-L90) write the same versioned shape.
- [sync.ts, lines 135–168](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/server/orchestration-skills/internal/sync.ts#L135-L168) hashes listed files, checks previous hashes before removing obsolete files, and writes the current manifest. Lines [188–212](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/server/orchestration-skills/internal/sync.ts#L188-L212) call the same sync for each of the three roots.

The rule uses `skill_manifest` only. A listed ordinary file whose current SHA matches the manifest can be reported as confirmed and unchanged; a mismatch is confirmed and modified. A directory name, `SKILL.md` name, or unlisted extra file is not ownership evidence. Each manifest entry is checked separately, and an unsafe relative path must be rejected rather than followed. The three synthetic manifest fixtures cover a clean hash, an edited listed file, and a `../` entry.

This evidence concerns app-managed synchronization. It does not prove whether a selected skill is currently enabled in a running Paseo session, nor does it cover runtime injection or profile overrides.
