#!/usr/bin/env bash
set -euo pipefail

skill_name="${1:-${SKILL_NAME:-evaluator-guidance}}"

claude_dir="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}/${skill_name}"
codex_dir="${CODEX_SKILLS_DIR:-$HOME/.codex/skills}/${skill_name}"

realpath_py() {
  python3 - "$1" <<'PY'
import os, sys
print(os.path.realpath(sys.argv[1]))
PY
}

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

dir_digest() {
  local dir="$1"

  # Stable manifest of file hashes (relative paths + sha256), then hash the manifest.
  (
    cd "$dir"
    python3 - <<'PY'
import hashlib
import os
import sys

def sha256_file(p: str) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()

rows = []
for root, dirs, files in os.walk(".", followlinks=True):
    # Deterministic traversal
    dirs.sort()
    files.sort()
    for name in files:
        if name == ".DS_Store":
            continue
        full = os.path.join(root, name)
        rel = os.path.relpath(full, ".")
        rows.append((rel, sha256_file(full)))

manifest = "\n".join([f"{rel}\t{h}" for rel, h in rows]) + "\n"
print(hashlib.sha256(manifest.encode("utf-8")).hexdigest())
PY
  )
}

dir_manifest() {
  local dir="$1"
  (
    cd "$dir"
    python3 - <<'PY'
import hashlib
import os

def sha256_file(p: str) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()

rows = []
for root, dirs, files in os.walk(".", followlinks=True):
    dirs.sort()
    files.sort()
    for name in files:
        if name == ".DS_Store":
            continue
        full = os.path.join(root, name)
        rel = os.path.relpath(full, ".")
        rows.append((rel, sha256_file(full)))

for rel, h in rows:
    print(f"{rel}\t{h}")
PY
  )
}

[[ -e "$claude_dir" ]] || fail "Claude skill missing: ${claude_dir}"
[[ -e "$codex_dir" ]] || fail "Codex skill missing:  ${codex_dir}"

claude_real="$(realpath_py "$claude_dir")"
codex_real="$(realpath_py "$codex_dir")"

if [[ "$claude_real" == "$codex_real" ]]; then
  echo "OK: '${skill_name}' points to the same real path:"
  echo "  ${claude_real}"
  exit 0
fi

claude_digest="$(dir_digest "$claude_dir")"
codex_digest="$(dir_digest "$codex_dir")"

if [[ "$claude_digest" == "$codex_digest" ]]; then
  echo "OK: '${skill_name}' content matches (different paths, same bytes)."
  echo "  Claude: ${claude_real}"
  echo "  Codex:  ${codex_real}"
  exit 0
fi

echo "Mismatch: '${skill_name}' differs between Claude and Codex." >&2
echo "  Claude: ${claude_real}" >&2
echo "  Codex:  ${codex_real}" >&2
echo >&2
echo "Claude manifest:" >&2
dir_manifest "$claude_dir" >&2 || true
echo >&2
echo "Codex manifest:" >&2
dir_manifest "$codex_dir" >&2 || true
echo >&2
fail "Skills are not identical. Re-link one to the other (symlink) or copy to match."

