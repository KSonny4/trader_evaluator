#!/usr/bin/env bash
set -euo pipefail

# ------------------------------------------------------------------
# Check that all evaluator-* skills are in sync across every agent
# directory.  Claude (~/.claude/skills/) is the source of truth.
#
# Directories checked:
#   1. Claude   ~/.claude/skills/           (source of truth)
#   2. Codex    ~/.codex/skills/
#   3. OpenCode ~/.config/opencode/skills/
#   4. Agents   ~/.agents/skills/
#
# For each evaluator-* skill found in Claude, we verify every other
# directory either symlinks to the same real path or has byte-identical
# content (sha256 of file tree).
# ------------------------------------------------------------------

CLAUDE_DIR="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}"
DIRS=(
  "${CODEX_SKILLS_DIR:-$HOME/.codex/skills}"
  "${OPENCODE_SKILLS_DIR:-$HOME/.config/opencode/skills}"
  "${AGENTS_SKILLS_DIR:-$HOME/.agents/skills}"
)
DIR_NAMES=("Codex" "OpenCode" "Agents")

realpath_py() {
  python3 - "$1" <<'PY'
import os, sys
print(os.path.realpath(sys.argv[1]))
PY
}

dir_digest() {
  local dir="$1"
  (
    cd "$dir"
    python3 - <<'PY'
import hashlib, os

def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()

rows = []
for root, dirs, files in os.walk(".", followlinks=True):
    dirs.sort(); files.sort()
    for name in files:
        if name == ".DS_Store":
            continue
        full = os.path.join(root, name)
        rel = os.path.relpath(full, ".")
        rows.append(f"{rel}\t{sha256_file(full)}")

manifest = "\n".join(rows) + "\n"
print(hashlib.sha256(manifest.encode("utf-8")).hexdigest())
PY
  )
}

errors=0

# Discover evaluator-* skills in Claude (source of truth)
skills=()
for d in "$CLAUDE_DIR"/evaluator-*; do
  [[ -d "$d" ]] || continue
  skills+=("$(basename "$d")")
done

if [[ ${#skills[@]} -eq 0 ]]; then
  echo "WARN: No evaluator-* skills found in $CLAUDE_DIR"
  exit 0
fi

for skill in "${skills[@]}"; do
  claude_path="$CLAUDE_DIR/$skill"
  claude_real="$(realpath_py "$claude_path")"
  claude_digest="$(dir_digest "$claude_path")"

  for i in "${!DIRS[@]}"; do
    target_dir="${DIRS[$i]}"
    target_name="${DIR_NAMES[$i]}"
    target_path="$target_dir/$skill"

    if [[ ! -e "$target_path" ]]; then
      echo "FAIL: '$skill' missing in $target_name ($target_path)" >&2
      errors=$((errors + 1))
      continue
    fi

    target_real="$(realpath_py "$target_path")"

    if [[ "$claude_real" == "$target_real" ]]; then
      echo "OK: '$skill' ${target_name} -> same path"
      continue
    fi

    target_digest="$(dir_digest "$target_path")"
    if [[ "$claude_digest" == "$target_digest" ]]; then
      echo "OK: '$skill' ${target_name} -> same content (different path)"
      continue
    fi

    echo "FAIL: '$skill' differs in $target_name" >&2
    echo "  Claude: $claude_real" >&2
    echo "  ${target_name}: $target_real" >&2
    errors=$((errors + 1))
  done
done

if [[ $errors -gt 0 ]]; then
  echo >&2
  echo "FAIL: $errors skill sync error(s). Fix with symlinks:" >&2
  echo "  ln -sf ~/.claude/skills/<skill> <target_dir>/<skill>" >&2
  exit 1
fi

echo "OK: all ${#skills[@]} evaluator skills in sync across ${#DIRS[@]} agent dirs"
