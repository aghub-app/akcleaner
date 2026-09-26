#!/bin/sh
# Recreate a synthetic home with removable Muxy / Superset / Paseo integrations,
# then launch the interactive cleanup against it. Never touches $HOME.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
home="$root/.demo-home"

rm -rf "$home"
mkdir -p "$home/.claude" "$home/.agents/skills/superset" "$home/.agents/skills/paseo-demo"

cat > "$home/.claude/settings.json" <<'EOF'
{
  "hooks": {
    "Stop": [{"hooks": [
      {"type": "command", "command": "runner muxy-notification-hook --private-token=x"},
      {"type": "command", "command": "runner $SUPERSET_HOME_DIR/hooks/notify.sh"},
      {"type": "command", "command": "user-hook"}
    ]}],
    "SessionStart": [{"hooks": [
      {"type": "command", "command": "run muxy-notification-hook"}
    ]}]
  }
}
EOF

cat > "$home/.agents/skills/superset/SKILL.md" <<'EOF'
# Superset helper
<!-- superset-managed-skill v1 -->
EOF

printf '# Paseo demo skill\n' > "$home/.agents/skills/paseo-demo/SKILL.md"

hash_for() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

skill_hash="$(hash_for "$home/.agents/skills/paseo-demo/SKILL.md")"
cat > "$home/.agents/skills/paseo-demo/.paseo-managed-files.json" <<EOF
{"version": 1, "files": {"SKILL.md": "$skill_hash"}}
EOF

cargo build --quiet --manifest-path "$root/Cargo.toml" --bin akcleaner
exec "$root/target/debug/akcleaner" clean --home "$home"
